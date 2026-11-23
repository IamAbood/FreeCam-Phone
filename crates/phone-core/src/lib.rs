//! Phone-side core: TCP server, mDNS advertisement and session logic.
//! Kotlin only captures/encodes and calls in through the JNI layer in `jni_api`.

mod jni_api;

use anyhow::Context;
use protocol::{Control, Packet, Stats, AUDIO_S16LE, CODEC_H264, FLAG_KEYFRAME, VERSION};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// Outbound queue depth per client. Video is dropped (until the next keyframe)
/// when the network can't keep up, so latency stays bounded.
const QUEUE: usize = 32;

pub struct Core {
    rt: tokio::runtime::Runtime,
    mdns: Option<mdns_sd::ServiceDaemon>,
    shared: Arc<Shared>,
}

struct Shared {
    /// Monotonic clock origin shared by pts and Pong timestamps.
    epoch: Instant,
    state: Mutex<State>,
    controls_tx: Mutex<Option<std_mpsc::Sender<String>>>,
    controls_rx: Mutex<Option<std_mpsc::Receiver<String>>>,
}

#[derive(Default)]
struct State {
    /// SPS/PPS, prepended to each keyframe so any keyframe is decodable alone.
    csd: Vec<u8>,
    video_config: Option<Packet>,
    stats: Option<Stats>,
    client: Option<mpsc::Sender<Packet>>,
    client_id: u64,
    peer: Option<String>,
    need_keyframe: bool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicked holder can't leave this simple state inconsistent; keep going.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn now_us(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    fn push_control(&self, c: String) {
        if let Some(tx) = lock(&self.controls_tx).as_ref() {
            let _ = tx.send(c);
        }
    }

    fn send(&self, st: &mut State, p: Packet) -> bool {
        let Some(tx) = &st.client else { return false };
        match tx.try_send(p) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => false,
            Err(mpsc::error::TrySendError::Closed(_)) => {
                st.client = None;
                st.peer = None;
                false
            }
        }
    }
}

impl Core {
    pub fn start(name: &str, port: u16) -> anyhow::Result<Core> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let (ctx, crx) = std_mpsc::channel();
        let shared = Arc::new(Shared {
            controls_tx: Mutex::new(Some(ctx)),
            controls_rx: Mutex::new(Some(crx)),
            epoch: Instant::now(),
            state: Mutex::default(),
        });
        let listener = rt.block_on(TcpListener::bind(("0.0.0.0", port)))?;
        rt.spawn(accept_loop(listener, shared.clone(), name.to_string()));
        // mDNS failure (e.g. no Wi-Fi) must not stop USB use.
        let mdns = advertise(name, port).ok();
        Ok(Core { rt, mdns, shared })
    }

    pub fn video_config(&self, width: u16, height: u16, fps: u8, csd: &[u8]) {
        let p = Packet::VideoConfig {
            codec: CODEC_H264,
            width,
            height,
            fps,
            csd: csd.to_vec(),
        };
        let mut st = lock(&self.shared.state);
        st.csd = csd.to_vec();
        st.video_config = Some(p.clone());
        st.need_keyframe = true;
        self.shared.send(&mut st, p);
    }

    /// `age_us`: how long ago the frame was captured, measured by the caller.
    pub fn video_frame(&self, data: &[u8], age_us: u64, key: bool) {
        let pts_us = self.shared.now_us().saturating_sub(age_us);
        let mut st = lock(&self.shared.state);
        if st.client.is_none() {
            return;
        }
        if st.need_keyframe && !key {
            return;
        }
        let mut buf = Vec::with_capacity(data.len() + st.csd.len());
        if key {
            buf.extend_from_slice(&st.csd);
        }
        buf.extend_from_slice(data);
        let p = Packet::VideoFrame {
            pts_us,
            flags: if key { FLAG_KEYFRAME } else { 0 },
            data: buf,
        };
        if self.shared.send(&mut st, p) {
            st.need_keyframe = false;
        } else if st.client.is_some() && !st.need_keyframe {
            // Congested: drop until a fresh keyframe.
            st.need_keyframe = true;
            drop(st);
            self.shared.push_control("key".into());
        }
    }

    pub fn audio_frame(&self, data: &[u8]) {
        let p = Packet::AudioFrame {
            pts_us: self.shared.now_us(),
            data: data.to_vec(),
        };
        let mut st = lock(&self.shared.state);
        self.shared.send(&mut st, p);
    }

    pub fn stats(&self, s: Stats) {
        let mut st = lock(&self.shared.state);
        st.stats = Some(s.clone());
        self.shared.send(&mut st, Packet::Stats(s));
    }

    /// Next control command for Kotlin, as a short string (see `control_string`).
    pub fn next_control(&self, timeout: Duration) -> Option<String> {
        let rx = lock(&self.shared.controls_rx);
        rx.as_ref()?.recv_timeout(timeout).ok()
    }

    pub fn status(&self) -> String {
        match &lock(&self.shared.state).peer {
            Some(p) => format!("Connected to {p}"),
            None => "Waiting for PC…".into(),
        }
    }

    pub fn stop(self) {
        if let Some(m) = &self.mdns {
            let _ = m.shutdown();
        }
        lock(&self.shared.controls_tx).take();
        self.rt.shutdown_timeout(Duration::from_millis(500));
    }
}

fn advertise(name: &str, port: u16) -> anyhow::Result<mdns_sd::ServiceDaemon> {
    let d = mdns_sd::ServiceDaemon::new()?;
    let host: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let info = mdns_sd::ServiceInfo::new(
        protocol::MDNS_SERVICE,
        name,
        &format!("freecam-{host}.local."),
        "",
        port,
        &[("v", "1")][..],
    )?
    .enable_addr_auto();
    d.register(info)?;
    Ok(d)
}

/// Maps a protocol control to the string command Kotlin understands.
fn control_string(c: &Control) -> String {
    match c {
        Control::SwitchCamera => "switch".into(),
        Control::Resolution(h) => format!("res:{h}"),
        Control::Fps(f) => format!("fps:{f}"),
        Control::Torch(on) => format!("torch:{}", *on as u8),
        Control::Focus { x, y } => format!("focus:{x}:{y}"),
        Control::Exposure(e) => format!("ev:{e}"),
        Control::Zoom(z) => format!("zoom:{z}"),
        Control::Mic(on) => format!("mic:{}", *on as u8),
        Control::RequestKeyframe => "key".into(),
    }
}

async fn accept_loop(listener: TcpListener, shared: Arc<Shared>, name: String) {
    loop {
        let Ok((sock, addr)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        };
        let shared = shared.clone();
        let name = name.clone();
        tokio::spawn(async move {
            let _ = serve(sock, addr.to_string(), shared, name).await;
        });
    }
}

async fn read_packet(r: &mut (impl AsyncReadExt + Unpin)) -> anyhow::Result<Packet> {
    let mut h = [0u8; protocol::HEADER_LEN];
    r.read_exact(&mut h).await?;
    let (ty, len) = protocol::parse_header(h)?;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).await?;
    Ok(Packet::decode(ty, &payload)?)
}

async fn serve(
    sock: TcpStream,
    addr: String,
    shared: Arc<Shared>,
    name: String,
) -> anyhow::Result<()> {
    sock.set_nodelay(true)?;
    let (mut rd, mut wr) = sock.into_split();
    let hello = tokio::time::timeout(Duration::from_secs(5), read_packet(&mut rd))
        .await
        .context("hello timeout")??;
    let peer = match hello {
        Packet::Hello { name, .. } => format!("{name} ({addr})"),
        _ => anyhow::bail!("expected hello"),
    };
    let (tx, mut rx) = mpsc::channel::<Packet>(QUEUE);
    let mut initial = vec![
        Packet::Hello {
            version: VERSION,
            name,
        },
        Packet::AudioConfig {
            sample_rate: 48_000,
            channels: 1,
            format: AUDIO_S16LE,
        },
    ];
    let my_id = {
        let mut st = lock(&shared.state);
        initial.extend(st.video_config.clone());
        initial.extend(st.stats.clone().map(Packet::Stats));
        // Newest connection wins; the previous writer ends when its sender drops.
        st.client = Some(tx.clone());
        st.client_id += 1;
        st.peer = Some(peer);
        st.need_keyframe = true;
        st.client_id
    };
    shared.push_control("key".into());
    for p in initial {
        wr.write_all(&p.encode()).await?;
    }

    let writer = tokio::spawn(async move {
        while let Some(p) = rx.recv().await {
            if wr.write_all(&p.encode()).await.is_err() {
                break;
            }
        }
    });

    let result = async {
        loop {
            // The PC pings every second; silence means it's gone.
            let p = tokio::time::timeout(Duration::from_secs(5), read_packet(&mut rd))
                .await
                .context("read timeout")??;
            match p {
                Packet::Control(c) => shared.push_control(control_string(&c)),
                Packet::Ping { t_us } => {
                    let _ = tx.try_send(Packet::Pong {
                        t_us,
                        peer_us: shared.now_us(),
                    });
                }
                _ => {}
            }
        }
    }
    .await;

    let mut st = lock(&shared.state);
    if st.client_id == my_id {
        st.client = None;
        st.peer = None;
    }
    drop(st);
    writer.abort();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpStream;

    #[test]
    fn end_to_end_session() {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let core = Core::start("Test Phone", port).unwrap();
        core.video_config(1280, 720, 30, &[0, 0, 0, 1, 0x67, 0, 0, 0, 1, 0x68]);

        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(
            &Packet::Hello {
                version: VERSION,
                name: "pc".into(),
            }
            .encode(),
        )
        .unwrap();
        assert!(matches!(
            protocol::read_packet(&mut s).unwrap(),
            Packet::Hello { .. }
        ));
        assert!(matches!(
            protocol::read_packet(&mut s).unwrap(),
            Packet::AudioConfig { .. }
        ));
        assert!(matches!(
            protocol::read_packet(&mut s).unwrap(),
            Packet::VideoConfig { width: 1280, .. }
        ));
        assert_eq!(
            core.next_control(Duration::from_secs(2)).as_deref(),
            Some("key")
        );

        // Wait until the server registered the client.
        while !core.status().starts_with("Connected") {
            std::thread::sleep(Duration::from_millis(10));
        }
        core.video_frame(&[0, 0, 0, 1, 0x41, 1], 0, false); // dropped: waiting for keyframe
        core.video_frame(&[0, 0, 0, 1, 0x65, 2], 1000, true);
        match protocol::read_packet(&mut s).unwrap() {
            Packet::VideoFrame { flags, data, .. } => {
                assert_eq!(flags, FLAG_KEYFRAME);
                assert_eq!(data[4], 0x67, "csd prepended to keyframe");
            }
            p => panic!("{p:?}"),
        }
        s.write_all(&Packet::Control(Control::Zoom(2.0)).encode())
            .unwrap();
        assert_eq!(
            core.next_control(Duration::from_secs(2)).as_deref(),
            Some("zoom:2")
        );
        s.write_all(&Packet::Ping { t_us: 5 }.encode()).unwrap();
        assert!(matches!(
            protocol::read_packet(&mut s).unwrap(),
            Packet::Pong { t_us: 5, .. }
        ));
        drop(s);
        core.stop();
    }
}
