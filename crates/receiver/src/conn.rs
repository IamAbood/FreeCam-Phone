//! Discovery (mDNS + adb), connection manager with auto-reconnect, and the
//! per-session read/decode loop.

use crate::decode::{self, Decoder};
use crate::settings::Settings;
use crate::{adb, audio, vcam};
use anyhow::Result;
use framebus::Frame;
use protocol::{read_packet, write_packet, Control, Packet, Stats, FLAG_KEYFRAME, VERSION};
use std::collections::{BTreeMap, VecDeque};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// Local port used for `adb forward`.
const USB_PORT: u16 = 47801;

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn now_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

#[derive(Default, Clone)]
pub struct State {
    /// False after the user pressed Disconnect.
    pub enabled: bool,
    pub status: String,
    /// "Pixel 8 via USB" while a session is up.
    pub connected: Option<String>,
    pub latency_ms: f32,
    pub fps: f32,
    pub stats: Option<Stats>,
    /// mDNS-discovered phones: name → address.
    pub wifi: BTreeMap<String, SocketAddr>,
    pub usb: Vec<String>,
    pub video_size: (usize, usize),
}

pub struct Shared {
    pub state: Mutex<State>,
    pub settings: Mutex<Settings>,
    pub preview: Mutex<Option<(u64, Arc<Frame>)>>,
    pub controls: Mutex<Option<mpsc::Sender<Control>>>,
    /// Socket of the live session, so the UI can drop it.
    pub socket: Mutex<Option<TcpStream>>,
    pub repaint: Mutex<Option<eframe::egui::Context>>,
    pub outputs: Mutex<vcam::Outputs>,
    pub audio: Mutex<audio::Sink>,
    last_frame: Mutex<Option<Instant>>,
}

impl Shared {
    pub fn new(settings: Settings) -> Arc<Shared> {
        let outputs = vcam::Outputs::new(settings.backend);
        Arc::new(Shared {
            state: Mutex::new(State {
                enabled: true,
                status: "Searching for phones…".into(),
                ..Default::default()
            }),
            audio: Mutex::new(audio::Sink::new(&settings.audio_device)),
            settings: Mutex::new(settings),
            preview: Mutex::new(None),
            controls: Mutex::new(None),
            socket: Mutex::new(None),
            repaint: Mutex::new(None),
            outputs: Mutex::new(outputs),
            last_frame: Mutex::new(None),
        })
    }

    pub fn send(&self, c: Control) {
        if let Some(tx) = lock(&self.controls).as_ref() {
            let _ = tx.send(c);
        }
    }

    /// Ends the current session (the manager reconnects if still enabled).
    pub fn drop_session(&self) {
        if let Some(s) = lock(&self.socket).take() {
            let _ = s.shutdown(Shutdown::Both);
        }
    }

    fn repaint(&self) {
        if let Some(c) = lock(&self.repaint).as_ref() {
            c.request_repaint();
        }
    }

    fn publish(&self, mut f: Frame, front: bool) {
        // Front sensors are mounted 180° from back ones in landscape.
        if front {
            f.rotate180();
        }
        if lock(&self.settings).mirror {
            f.mirror();
        }
        let f = Arc::new(f);
        lock(&self.outputs).send(&f);
        let mut p = lock(&self.preview);
        let seq = p.as_ref().map_or(0, |(s, _)| s + 1);
        *p = Some((seq, f));
        drop(p);
        *lock(&self.last_frame) = Some(Instant::now());
        self.repaint();
    }
}

pub fn spawn(shared: Arc<Shared>) {
    let s = shared.clone();
    thread::spawn(move || discovery(s));
    let s = shared.clone();
    thread::spawn(move || loop {
        let path = adb::find(&lock(&s.settings).adb_path);
        let usb = path.map(|p| adb::devices(&p)).unwrap_or_default();
        lock(&s.state).usb = usb;
        thread::sleep(Duration::from_secs(2));
    });
    let s = shared.clone();
    // Placeholder card whenever no live video is flowing.
    thread::spawn(move || loop {
        let stale = lock(&s.last_frame).is_none_or(|t| t.elapsed() > Duration::from_secs(1));
        if stale {
            lock(&s.outputs).placeholder();
        }
        thread::sleep(Duration::from_millis(100));
    });
    thread::spawn(move || manager(shared));
}

fn discovery(shared: Arc<Shared>) {
    let Ok(d) = mdns_sd::ServiceDaemon::new() else {
        return;
    };
    let Ok(rx) = d.browse(protocol::MDNS_SERVICE) else {
        return;
    };
    let suffix = format!(".{}", protocol::MDNS_SERVICE);
    while let Ok(ev) = rx.recv() {
        match ev {
            mdns_sd::ServiceEvent::ServiceResolved(info) => {
                let name = info.fullname.trim_end_matches(&suffix).replace("\\.", ".");
                if let Some(ip) = info.get_addresses_v4().into_iter().next() {
                    lock(&shared.state)
                        .wifi
                        .insert(name, SocketAddr::from((ip, info.port)));
                    shared.repaint();
                }
            }
            mdns_sd::ServiceEvent::ServiceRemoved(_, full) => {
                let name = full.trim_end_matches(&suffix).replace("\\.", ".");
                lock(&shared.state).wifi.remove(&name);
                shared.repaint();
            }
            _ => {}
        }
    }
}

enum Target {
    Usb(String),
    Wifi(String, SocketAddr),
}

fn pick(shared: &Shared) -> Option<Target> {
    let want = lock(&shared.settings).device.clone();
    let st = lock(&shared.state);
    if let Some(serial) = want.strip_prefix("usb:") {
        return st
            .usb
            .iter()
            .any(|s| s == serial)
            .then(|| Target::Usb(serial.into()));
    }
    if let Some(name) = want.strip_prefix("wifi:") {
        return st.wifi.get(name).map(|a| Target::Wifi(name.into(), *a));
    }
    // Automatic: USB first.
    if let Some(serial) = st.usb.first() {
        return Some(Target::Usb(serial.clone()));
    }
    st.wifi
        .iter()
        .next()
        .map(|(n, a)| Target::Wifi(n.clone(), *a))
}

fn set_status(shared: &Shared, s: impl Into<String>) {
    lock(&shared.state).status = s.into();
    shared.repaint();
}

fn manager(shared: Arc<Shared>) {
    if let Err(e) = decode::init_thread() {
        set_status(&shared, format!("Media Foundation unavailable: {e}"));
        return;
    }
    let mut backoff = Duration::from_millis(500);
    loop {
        if !lock(&shared.state).enabled {
            thread::sleep(Duration::from_millis(300));
            continue;
        }
        let Some(target) = pick(&shared) else {
            set_status(&shared, "Searching for phones (Wi-Fi / USB)…");
            thread::sleep(Duration::from_millis(500));
            continue;
        };
        let (addr, via) = match &target {
            Target::Usb(serial) => {
                let Some(adb) = adb::find(&lock(&shared.settings).adb_path) else {
                    thread::sleep(backoff);
                    continue;
                };
                if let Err(e) = adb::forward(&adb, serial, USB_PORT) {
                    set_status(&shared, format!("adb forward failed: {e}"));
                    thread::sleep(backoff);
                    continue;
                }
                (
                    SocketAddr::from(([127, 0, 0, 1], USB_PORT)),
                    format!("USB ({serial})"),
                )
            }
            Target::Wifi(name, a) => (*a, format!("Wi-Fi ({name}, {a})")),
        };
        set_status(&shared, format!("Connecting via {via}…"));
        let started = Instant::now();
        let r = session(&shared, addr, &via);
        {
            let mut st = lock(&shared.state);
            st.connected = None;
            st.fps = 0.0;
            st.status = match r {
                Ok(()) => "Disconnected".into(),
                Err(e) => format!("Disconnected: {e}"),
            };
        }
        *lock(&shared.last_frame) = None;
        shared.repaint();
        backoff = if started.elapsed() > Duration::from_secs(5) {
            Duration::from_millis(500)
        } else {
            (backoff * 2).min(Duration::from_secs(5))
        };
        thread::sleep(backoff);
    }
}

fn session(shared: &Arc<Shared>, addr: SocketAddr, via: &str) -> Result<()> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into());
    write_packet(
        &mut s,
        &Packet::Hello {
            version: VERSION,
            name: host,
        },
    )?;
    *lock(&shared.socket) = Some(s.try_clone()?);

    let (tx, rx) = mpsc::channel::<Control>();
    {
        let st = lock(&shared.settings);
        let _ = tx.send(Control::Resolution(st.resolution));
        let _ = tx.send(Control::Fps(st.fps));
    }
    *lock(&shared.controls) = Some(tx);
    let mut w = s.try_clone()?;
    let writer = thread::spawn(move || loop {
        let p = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(c) => Packet::Control(c),
            Err(mpsc::RecvTimeoutError::Timeout) => Packet::Ping { t_us: now_us() },
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if write_packet(&mut w, &p).is_err() {
            break;
        }
    });
    // Ping right away so latency shows up quickly.
    let _ = write_packet(&mut s, &Packet::Ping { t_us: now_us() });

    let r = read_loop(shared, &mut s, via);
    lock(&shared.controls).take();
    lock(&shared.socket).take();
    let _ = s.shutdown(Shutdown::Both);
    let _ = writer.join();
    r
}

fn read_loop(shared: &Arc<Shared>, s: &mut TcpStream, via: &str) -> Result<()> {
    let mut cfg: Option<(usize, usize, u32)> = None;
    let mut dec: Option<Decoder> = None;
    let mut need_key = true;
    // (rtt, offset) samples; the lowest-RTT one gives the best clock offset.
    let mut pongs: VecDeque<(u64, i64)> = VecDeque::new();
    let mut offset: Option<i64> = None;
    let (mut frames, mut fps_t) = (0u32, Instant::now());
    let mut front = false;
    loop {
        match read_packet(s)? {
            Packet::Hello { name, .. } => {
                let mut st = lock(&shared.state);
                st.connected = Some(format!("{name} via {via}"));
                st.status = format!("Connected to {name} via {via}");
                drop(st);
                shared.repaint();
            }
            Packet::VideoConfig {
                width, height, fps, ..
            } => {
                cfg = Some((width as usize, height as usize, fps as u32));
                dec = None;
                need_key = true;
                lock(&shared.state).video_size = (width as usize, height as usize);
            }
            Packet::VideoFrame {
                pts_us,
                flags,
                data,
            } => {
                let key = flags & FLAG_KEYFRAME != 0;
                if need_key && !key {
                    continue;
                }
                if dec.is_none() {
                    if let Some((w, h, f)) = cfg {
                        dec = Decoder::new(w, h, f).ok();
                    }
                }
                let Some(d) = dec.as_mut() else { continue };
                match d.decode(&data, pts_us) {
                    Ok(out) => {
                        need_key = false;
                        for (pts, f) in out {
                            frames += 1;
                            if let Some(off) = offset {
                                let captured = pts as i64 - off;
                                let lat = (now_us() as i64 - captured) as f32 / 1000.0;
                                let mut st = lock(&shared.state);
                                st.latency_ms = if st.latency_ms == 0.0 {
                                    lat
                                } else {
                                    st.latency_ms * 0.9 + lat * 0.1
                                };
                            }
                            shared.publish(f, front);
                        }
                    }
                    Err(_) => {
                        dec = None;
                        need_key = true;
                        shared.send(Control::RequestKeyframe);
                    }
                }
                if fps_t.elapsed() >= Duration::from_secs(1) {
                    lock(&shared.state).fps = frames as f32 / fps_t.elapsed().as_secs_f32();
                    frames = 0;
                    fps_t = Instant::now();
                }
            }
            Packet::AudioFrame { data, .. } => lock(&shared.audio).push(&data),
            Packet::Stats(st) => {
                front = st.facing == 1;
                lock(&shared.state).stats = Some(st);
                shared.repaint();
            }
            Packet::Pong { t_us, peer_us } => {
                let now = now_us();
                let rtt = now.saturating_sub(t_us);
                pongs.push_back((rtt, peer_us as i64 - (t_us + rtt / 2) as i64));
                if pongs.len() > 10 {
                    pongs.pop_front();
                }
                offset = pongs.iter().min_by_key(|p| p.0).map(|p| p.1);
            }
            _ => {}
        }
    }
}
