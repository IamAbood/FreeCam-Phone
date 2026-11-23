//! FreeCam Phone wire protocol.
//!
//! Every packet is framed as `[u8 type][u32 len, little-endian][payload]`.
//! Payload fields are little-endian; byte blobs and strings run to the end of
//! the payload or are prefixed by a u32 length where another field follows.

use std::io::{Read, Write};

pub const VERSION: u16 = 1;
pub const MAGIC: [u8; 4] = *b"FCAM";
pub const HEADER_LEN: usize = 5;
/// Largest payload we accept (a 1080p keyframe is well under 1 MiB).
pub const MAX_PAYLOAD: u32 = 8 * 1024 * 1024;
pub const DEFAULT_PORT: u16 = 47800;
pub const MDNS_SERVICE: &str = "_freecamphone._tcp.local.";

pub const FLAG_KEYFRAME: u8 = 1;
pub const CODEC_H264: u8 = 1;
pub const AUDIO_S16LE: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("payload length {0} exceeds maximum {MAX_PAYLOAD}")]
    TooLarge(u32),
    #[error("unknown packet type {0:#04x}")]
    UnknownType(u8),
    #[error("unknown control tag {0:#04x}")]
    UnknownControl(u8),
    #[error("truncated payload")]
    Truncated,
    #[error("bad magic")]
    BadMagic,
    #[error("invalid utf-8")]
    Utf8,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    SwitchCamera,
    /// Target height: 480, 720 or 1080.
    Resolution(u16),
    /// 30 or 60.
    Fps(u8),
    Torch(bool),
    /// Normalised tap-to-focus point (0..1, 0..1) in sensor-upright image space.
    Focus {
        x: f32,
        y: f32,
    },
    /// Exposure compensation in device steps.
    Exposure(i8),
    /// Zoom ratio, 1.0 = no zoom.
    Zoom(f32),
    Mic(bool),
    RequestKeyframe,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub fps: f32,
    pub bitrate_kbps: u32,
    /// 0 = back, 1 = front.
    pub facing: u8,
    pub zoom_max: f32,
    pub ev_min: i8,
    pub ev_max: i8,
    pub has_torch: bool,
    pub mic_on: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    Hello {
        version: u16,
        name: String,
    },
    VideoConfig {
        codec: u8,
        width: u16,
        height: u16,
        fps: u8,
        /// SPS + PPS, Annex-B.
        csd: Vec<u8>,
    },
    VideoFrame {
        pts_us: u64,
        flags: u8,
        /// Annex-B NAL units.
        data: Vec<u8>,
    },
    AudioConfig {
        sample_rate: u32,
        channels: u8,
        format: u8,
    },
    AudioFrame {
        pts_us: u64,
        data: Vec<u8>,
    },
    Control(Control),
    Stats(Stats),
    Ping {
        t_us: u64,
    },
    Pong {
        t_us: u64,
        peer_us: u64,
    },
}

impl Packet {
    fn type_id(&self) -> u8 {
        match self {
            Packet::Hello { .. } => 1,
            Packet::VideoConfig { .. } => 2,
            Packet::VideoFrame { .. } => 3,
            Packet::AudioConfig { .. } => 4,
            Packet::AudioFrame { .. } => 5,
            Packet::Control(_) => 6,
            Packet::Stats(_) => 7,
            Packet::Ping { .. } => 8,
            Packet::Pong { .. } => 9,
        }
    }

    /// Serialises the packet including its 5-byte header.
    pub fn encode(&self) -> Vec<u8> {
        let mut p = vec![self.type_id(), 0, 0, 0, 0];
        match self {
            Packet::Hello { version, name } => {
                p.extend_from_slice(&MAGIC);
                p.extend_from_slice(&version.to_le_bytes());
                p.extend_from_slice(name.as_bytes());
            }
            Packet::VideoConfig {
                codec,
                width,
                height,
                fps,
                csd,
            } => {
                p.push(*codec);
                p.extend_from_slice(&width.to_le_bytes());
                p.extend_from_slice(&height.to_le_bytes());
                p.push(*fps);
                p.extend_from_slice(csd);
            }
            Packet::VideoFrame {
                pts_us,
                flags,
                data,
            } => {
                p.extend_from_slice(&pts_us.to_le_bytes());
                p.push(*flags);
                p.extend_from_slice(data);
            }
            Packet::AudioConfig {
                sample_rate,
                channels,
                format,
            } => {
                p.extend_from_slice(&sample_rate.to_le_bytes());
                p.push(*channels);
                p.push(*format);
            }
            Packet::AudioFrame { pts_us, data } => {
                p.extend_from_slice(&pts_us.to_le_bytes());
                p.extend_from_slice(data);
            }
            Packet::Control(c) => match c {
                Control::SwitchCamera => p.push(1),
                Control::Resolution(h) => {
                    p.push(2);
                    p.extend_from_slice(&h.to_le_bytes());
                }
                Control::Fps(f) => p.extend_from_slice(&[3, *f]),
                Control::Torch(on) => p.extend_from_slice(&[4, *on as u8]),
                Control::Focus { x, y } => {
                    p.push(5);
                    p.extend_from_slice(&x.to_le_bytes());
                    p.extend_from_slice(&y.to_le_bytes());
                }
                Control::Exposure(ev) => p.extend_from_slice(&[6, *ev as u8]),
                Control::Zoom(z) => {
                    p.push(7);
                    p.extend_from_slice(&z.to_le_bytes());
                }
                Control::Mic(on) => p.extend_from_slice(&[8, *on as u8]),
                Control::RequestKeyframe => p.push(9),
            },
            Packet::Stats(s) => {
                p.extend_from_slice(&s.fps.to_le_bytes());
                p.extend_from_slice(&s.bitrate_kbps.to_le_bytes());
                p.push(s.facing);
                p.extend_from_slice(&s.zoom_max.to_le_bytes());
                p.push(s.ev_min as u8);
                p.push(s.ev_max as u8);
                p.push(s.has_torch as u8);
                p.push(s.mic_on as u8);
            }
            Packet::Ping { t_us } => p.extend_from_slice(&t_us.to_le_bytes()),
            Packet::Pong { t_us, peer_us } => {
                p.extend_from_slice(&t_us.to_le_bytes());
                p.extend_from_slice(&peer_us.to_le_bytes());
            }
        }
        let len = (p.len() - HEADER_LEN) as u32;
        p[1..5].copy_from_slice(&len.to_le_bytes());
        p
    }

    /// Decodes a payload whose header has already been parsed.
    pub fn decode(ty: u8, payload: &[u8]) -> Result<Packet> {
        let mut r = Cursor(payload);
        let pkt = match ty {
            1 => {
                if r.take(4)? != MAGIC {
                    return Err(Error::BadMagic);
                }
                Packet::Hello {
                    version: r.u16()?,
                    name: String::from_utf8(r.rest().to_vec()).map_err(|_| Error::Utf8)?,
                }
            }
            2 => Packet::VideoConfig {
                codec: r.u8()?,
                width: r.u16()?,
                height: r.u16()?,
                fps: r.u8()?,
                csd: r.rest().to_vec(),
            },
            3 => Packet::VideoFrame {
                pts_us: r.u64()?,
                flags: r.u8()?,
                data: r.rest().to_vec(),
            },
            4 => Packet::AudioConfig {
                sample_rate: r.u32()?,
                channels: r.u8()?,
                format: r.u8()?,
            },
            5 => Packet::AudioFrame {
                pts_us: r.u64()?,
                data: r.rest().to_vec(),
            },
            6 => Packet::Control(match r.u8()? {
                1 => Control::SwitchCamera,
                2 => Control::Resolution(r.u16()?),
                3 => Control::Fps(r.u8()?),
                4 => Control::Torch(r.u8()? != 0),
                5 => Control::Focus {
                    x: r.f32()?,
                    y: r.f32()?,
                },
                6 => Control::Exposure(r.u8()? as i8),
                7 => Control::Zoom(r.f32()?),
                8 => Control::Mic(r.u8()? != 0),
                9 => Control::RequestKeyframe,
                t => return Err(Error::UnknownControl(t)),
            }),
            7 => Packet::Stats(Stats {
                fps: r.f32()?,
                bitrate_kbps: r.u32()?,
                facing: r.u8()?,
                zoom_max: r.f32()?,
                ev_min: r.u8()? as i8,
                ev_max: r.u8()? as i8,
                has_torch: r.u8()? != 0,
                mic_on: r.u8()? != 0,
            }),
            8 => Packet::Ping { t_us: r.u64()? },
            9 => Packet::Pong {
                t_us: r.u64()?,
                peer_us: r.u64()?,
            },
            t => return Err(Error::UnknownType(t)),
        };
        Ok(pkt)
    }
}

/// Parses a 5-byte header into `(type, payload_len)`, rejecting oversized lengths.
pub fn parse_header(h: [u8; HEADER_LEN]) -> Result<(u8, usize)> {
    let len = u32::from_le_bytes([h[1], h[2], h[3], h[4]]);
    if len > MAX_PAYLOAD {
        return Err(Error::TooLarge(len));
    }
    Ok((h[0], len as usize))
}

/// Tries to decode one packet from the front of `buf`.
/// Returns `Ok(None)` when more bytes are needed, otherwise the packet and bytes consumed.
pub fn try_decode(buf: &[u8]) -> Result<Option<(Packet, usize)>> {
    let Some(h) = buf.get(..HEADER_LEN) else {
        return Ok(None);
    };
    let (ty, len) = parse_header([h[0], h[1], h[2], h[3], h[4]])?;
    let Some(payload) = buf.get(HEADER_LEN..HEADER_LEN + len) else {
        return Ok(None);
    };
    Ok(Some((Packet::decode(ty, payload)?, HEADER_LEN + len)))
}

/// Blocking read of one packet.
pub fn read_packet(r: &mut impl Read) -> Result<Packet> {
    let mut h = [0u8; HEADER_LEN];
    r.read_exact(&mut h)?;
    let (ty, len) = parse_header(h)?;
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    Packet::decode(ty, &payload)
}

pub fn write_packet(w: &mut impl Write, p: &Packet) -> Result<()> {
    w.write_all(&p.encode())?;
    Ok(())
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(Error::Truncated);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.arr()?))
    }
    fn rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_packets() -> Vec<Packet> {
        let mut v = vec![
            Packet::Hello {
                version: VERSION,
                name: "Pixel 8 — test".into(),
            },
            Packet::VideoConfig {
                codec: CODEC_H264,
                width: 1920,
                height: 1080,
                fps: 30,
                csd: vec![0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3],
            },
            Packet::VideoFrame {
                pts_us: 123_456_789,
                flags: FLAG_KEYFRAME,
                data: vec![0, 0, 0, 1, 0x65, 9, 9, 9],
            },
            Packet::VideoFrame {
                pts_us: 0,
                flags: 0,
                data: vec![],
            },
            Packet::AudioConfig {
                sample_rate: 48_000,
                channels: 1,
                format: AUDIO_S16LE,
            },
            Packet::AudioFrame {
                pts_us: 42,
                data: vec![1, 2, 3, 4],
            },
            Packet::Stats(Stats {
                fps: 29.97,
                bitrate_kbps: 6000,
                facing: 1,
                zoom_max: 8.0,
                ev_min: -12,
                ev_max: 12,
                has_torch: true,
                mic_on: false,
            }),
            Packet::Ping { t_us: 7 },
            Packet::Pong {
                t_us: 7,
                peer_us: u64::MAX,
            },
        ];
        for c in [
            Control::SwitchCamera,
            Control::Resolution(720),
            Control::Fps(60),
            Control::Torch(true),
            Control::Focus { x: 0.25, y: 0.75 },
            Control::Exposure(-3),
            Control::Zoom(2.5),
            Control::Mic(false),
            Control::RequestKeyframe,
        ] {
            v.push(Packet::Control(c));
        }
        v
    }

    #[test]
    fn round_trip_every_type() {
        for p in all_packets() {
            let bytes = p.encode();
            assert_eq!(try_decode(&bytes).unwrap(), Some((p.clone(), bytes.len())));
            assert_eq!(read_packet(&mut &bytes[..]).unwrap(), p);
        }
    }

    #[test]
    fn partial_reads_need_more() {
        let stream: Vec<u8> = all_packets().iter().flat_map(|p| p.encode()).collect();
        // Every strict prefix of the first packet yields None.
        let first = all_packets()[0].encode();
        for n in 0..first.len() {
            assert!(try_decode(&stream[..n]).unwrap().is_none(), "prefix {n}");
        }
        // Feeding a concatenated stream byte-by-byte recovers every packet in order.
        let mut buf = Vec::new();
        let mut got = Vec::new();
        for b in stream {
            buf.push(b);
            while let Some((p, used)) = try_decode(&buf).unwrap() {
                buf.drain(..used);
                got.push(p);
            }
        }
        assert_eq!(got, all_packets());
        assert!(buf.is_empty());
    }

    #[test]
    fn oversized_length_rejected() {
        let mut h = [3u8, 0, 0, 0, 0];
        h[1..].copy_from_slice(&(MAX_PAYLOAD + 1).to_le_bytes());
        assert!(matches!(try_decode(&h), Err(Error::TooLarge(_))));
        assert!(matches!(read_packet(&mut &h[..]), Err(Error::TooLarge(_))));
        h[1..].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(parse_header(h), Err(Error::TooLarge(_))));
    }

    #[test]
    fn malformed_payloads_error() {
        assert!(matches!(
            Packet::decode(0xEE, &[]),
            Err(Error::UnknownType(0xEE))
        ));
        assert!(matches!(
            Packet::decode(6, &[0xEE]),
            Err(Error::UnknownControl(0xEE))
        ));
        assert!(matches!(
            Packet::decode(8, &[1, 2, 3]),
            Err(Error::Truncated)
        ));
        assert!(matches!(
            Packet::decode(1, b"NOPE\x01\x00"),
            Err(Error::BadMagic)
        ));
        // Truncated stream mid-payload is an IO error, not a panic.
        let bytes = Packet::Ping { t_us: 1 }.encode();
        assert!(matches!(
            read_packet(&mut &bytes[..bytes.len() - 1]),
            Err(Error::Io(_))
        ));
    }
}
