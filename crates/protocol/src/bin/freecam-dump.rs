//! Debug tool: connects to a phone and writes the H.264 stream to out.h264
//! and the mic audio (48 kHz mono s16le) to out.pcm.
//!
//! Usage: freecam-dump <host> [port] [seconds]

use protocol::{read_packet, write_packet, Packet, DEFAULT_PORT, VERSION};
use std::io::Write;
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let host = args
        .next()
        .ok_or("usage: freecam-dump <host> [port] [seconds]")?;
    let port: u16 = args
        .next()
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(DEFAULT_PORT);
    let secs: u64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(10);

    let mut s = TcpStream::connect((host.as_str(), port))?;
    s.set_nodelay(true)?;
    write_packet(
        &mut s,
        &Packet::Hello {
            version: VERSION,
            name: "freecam-dump".into(),
        },
    )?;
    // The phone drops silent clients after 5 s, so keep pinging.
    let mut pinger = s.try_clone()?;
    std::thread::spawn(move || {
        let t0 = Instant::now();
        while write_packet(
            &mut pinger,
            &Packet::Ping {
                t_us: t0.elapsed().as_micros() as u64,
            },
        )
        .is_ok()
        {
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    let mut video = std::fs::File::create("out.h264")?;
    let mut audio = std::fs::File::create("out.pcm")?;
    let start = Instant::now();
    let (mut frames, mut bytes, mut audio_bytes, mut last_pts) = (0u64, 0u64, 0u64, 0u64);
    while start.elapsed() < Duration::from_secs(secs) {
        match read_packet(&mut s)? {
            Packet::Hello { version, name } => println!("hello from {name:?} v{version}"),
            Packet::VideoConfig {
                width,
                height,
                fps,
                csd,
                ..
            } => {
                println!(
                    "video config {width}x{height}@{fps}, csd {} bytes",
                    csd.len()
                )
            }
            Packet::VideoFrame { data, pts_us, .. } => {
                last_pts = pts_us;
                frames += 1;
                bytes += data.len() as u64;
                video.write_all(&data)?;
            }
            Packet::AudioFrame { data, .. } => {
                audio_bytes += data.len() as u64;
                audio.write_all(&data)?;
            }
            Packet::Pong { peer_us, .. } => {
                // Phone clock now minus the newest frame's capture time.
                println!(
                    "frame age on phone clock: {} ms",
                    (peer_us as i64 - last_pts as i64) / 1000
                )
            }
            Packet::Stats(_) => {}
            other => println!("{other:?}"),
        }
    }
    let t = start.elapsed().as_secs_f64();
    println!(
        "{frames} frames ({:.1} fps, {:.0} kbps), {audio_bytes} audio bytes in {t:.1}s",
        frames as f64 / t,
        bytes as f64 * 8.0 / t / 1000.0
    );
    Ok(())
}
