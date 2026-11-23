//! Cross-process frame ring: 3 NV12 slots + header in a named file mapping,
//! plus a named auto-reset event signalled per frame.
//!
//! The receiver (user session, medium IL) creates `Local\FreeCamPhoneFrameBus`.
//! The MF media source runs inside the Camera Frame Server service (session 0,
//! LOCAL SERVICE), so the security descriptor explicitly grants LOCAL SERVICE
//! and SYSTEM, and the reader opens it through `Session\<id>\...`.

use crate::image::Frame;
use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, SYNCHRONIZATION_SYNCHRONIZE,
};

pub const MAX_W: usize = 1920;
pub const MAX_H: usize = 1080;
const SLOT_BYTES: usize = MAX_W * MAX_H * 3 / 2;
const SLOTS: usize = 3;
const MAGIC: u32 = u32::from_le_bytes(*b"FCFB");
const NONE: u32 = u32::MAX;
const MAP_NAME: &str = "FreeCamPhoneFrameBus";
const EVENT_NAME: &str = "FreeCamPhoneFrameEvent";
/// SYSTEM, LOCAL SERVICE, Administrators and the creating owner get full access.
const SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;LS)(A;;GA;;;BA)(A;;GA;;;OW)";

#[repr(C)]
struct SlotHeader {
    seq: AtomicU64,
    width: AtomicU32,
    height: AtomicU32,
    /// Unix time in µs when the slot was written; readers treat old frames as "no phone".
    timestamp_us: AtomicU64,
}

#[repr(C)]
struct Header {
    magic: AtomicU32,
    latest: AtomicU32,
    /// Frames published so far; slot seq = 2n while stable, 2n-1 while writing.
    counter: AtomicU64,
    slots: [SlotHeader; SLOTS],
}

const DATA_OFFSET: usize = 4096;
const TOTAL: usize = DATA_OFFSET + SLOTS * SLOT_BYTES;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("frame bus not available")]
    NotFound,
    #[error("frame too large ({0}x{1})")]
    TooLarge(usize, usize),
    #[error(transparent)]
    Win(#[from] windows::core::Error),
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

struct Mapping {
    handle: HANDLE,
    event: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

// SAFETY: the mapping and event handles are process-wide kernel objects usable
// from any thread; all shared-memory access goes through atomics or the seqlock.
unsafe impl Send for Mapping {}
// SAFETY: see above; `&Mapping` only exposes atomic header access and reads.
unsafe impl Sync for Mapping {}

impl Mapping {
    fn header(&self) -> &Header {
        // SAFETY: the view is at least TOTAL bytes, page aligned, and Header is
        // made only of atomics, which are valid for any bit pattern.
        unsafe { &*(self.view.Value as *const Header) }
    }
    fn slot_ptr(&self, i: usize) -> *mut u8 {
        // SAFETY: i < SLOTS, so the offset stays inside the TOTAL-byte view.
        unsafe { (self.view.Value as *mut u8).add(DATA_OFFSET + i * SLOT_BYTES) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: view/handles were obtained from successful Win32 calls and
        // are released exactly once here.
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.handle);
            let _ = CloseHandle(self.event);
        }
    }
}

/// Producer side (receiver process).
pub struct Writer {
    m: Mapping,
}

impl Writer {
    pub fn create() -> Result<Writer, Error> {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: valid NUL-terminated SDDL string and out-pointer; the
        // descriptor is freed by the OS when the process exits (tiny, once).
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &HSTRING::from(SDDL),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )?;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        // SAFETY: plain Win32 object creation with valid arguments; returned
        // handles are owned by `Mapping` and closed on drop.
        let m = unsafe {
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                Some(&sa),
                PAGE_READWRITE,
                0,
                TOTAL as u32,
                &HSTRING::from(format!("Local\\{MAP_NAME}")),
            )?;
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, TOTAL);
            if view.Value.is_null() {
                let _ = CloseHandle(handle);
                return Err(windows::core::Error::from_win32().into());
            }
            let event = CreateEventW(
                Some(&sa),
                false,
                false,
                &HSTRING::from(format!("Local\\{EVENT_NAME}")),
            )?;
            Mapping {
                handle,
                event,
                view,
            }
        };
        let h = m.header();
        h.latest.store(NONE, Ordering::Release);
        h.magic.store(MAGIC, Ordering::Release);
        Ok(Writer { m })
    }

    pub fn publish(&self, f: &Frame) -> Result<(), Error> {
        if f.width > MAX_W || f.height > MAX_H || f.data.len() > SLOT_BYTES {
            return Err(Error::TooLarge(f.width, f.height));
        }
        let h = self.m.header();
        let latest = h.latest.load(Ordering::Acquire);
        let i = if latest == NONE {
            0
        } else {
            (latest as usize + 1) % SLOTS
        };
        let s = &h.slots[i];
        let n = h.counter.load(Ordering::Relaxed) + 1;
        s.seq.store(2 * n - 1, Ordering::Relaxed); // odd = writing
        fence(Ordering::Release);
        // SAFETY: slot i has SLOT_BYTES capacity and data.len() <= SLOT_BYTES.
        // Concurrent readers detect overlap via the odd/changed seq and retry.
        unsafe { std::ptr::copy_nonoverlapping(f.data.as_ptr(), self.m.slot_ptr(i), f.data.len()) };
        s.width.store(f.width as u32, Ordering::Relaxed);
        s.height.store(f.height as u32, Ordering::Relaxed);
        s.timestamp_us.store(now_us(), Ordering::Relaxed);
        s.seq.store(2 * n, Ordering::Release); // even = stable
        h.counter.store(n, Ordering::Relaxed);
        h.latest.store(i as u32, Ordering::Release);
        // SAFETY: valid event handle owned by the mapping.
        unsafe {
            let _ = SetEvent(self.m.event);
        }
        Ok(())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.m.header().latest.store(NONE, Ordering::Release);
    }
}

/// Consumer side (media source in the Frame Server, or tests).
pub struct Reader {
    m: Mapping,
}

impl Reader {
    /// Opens the bus created by the receiver in the active console session
    /// (or this process's own session).
    pub fn open() -> Result<Reader, Error> {
        // SAFETY: no preconditions.
        let console = unsafe { WTSGetActiveConsoleSessionId() };
        let mut names = vec![format!("Local\\{MAP_NAME}")];
        // ponytail: console session + first few; enumerate WTS sessions if RDP users need it.
        for id in std::iter::once(console).chain(1..=4) {
            names.push(format!("Session\\{id}\\{MAP_NAME}"));
        }
        for name in names {
            let ev_name = name.replace(MAP_NAME, EVENT_NAME);
            // SAFETY: Win32 open calls with valid names; handles owned by Mapping.
            let m = unsafe {
                let Ok(handle) = OpenFileMappingW(FILE_MAP_READ.0, false, &HSTRING::from(&name))
                else {
                    continue;
                };
                let view = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, TOTAL);
                if view.Value.is_null() {
                    let _ = CloseHandle(handle);
                    continue;
                }
                let event = OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, &HSTRING::from(ev_name))
                    .unwrap_or_default();
                Mapping {
                    handle,
                    event,
                    view,
                }
            };
            if m.header().magic.load(Ordering::Acquire) == MAGIC {
                return Ok(Reader { m });
            }
        }
        Err(Error::NotFound)
    }

    /// Waits up to `ms` for a new-frame signal.
    pub fn wait(&self, ms: u32) -> bool {
        if self.m.event.is_invalid() {
            return false;
        }
        // SAFETY: valid event handle.
        unsafe { WaitForSingleObject(self.m.event, ms) == WAIT_OBJECT_0 }
    }

    /// Latest frame no older than `max_age_us`, along with its sequence number.
    pub fn latest(&self, max_age_us: u64) -> Option<(u64, Frame)> {
        let h = self.m.header();
        for _ in 0..4 {
            let i = h.latest.load(Ordering::Acquire);
            if i as usize >= SLOTS {
                return None;
            }
            let s = &h.slots[i as usize];
            let s1 = s.seq.load(Ordering::Acquire);
            if s1 & 1 == 1 {
                continue;
            }
            let (w, ht) = (
                s.width.load(Ordering::Relaxed) as usize,
                s.height.load(Ordering::Relaxed) as usize,
            );
            let ts = s.timestamp_us.load(Ordering::Relaxed);
            if w == 0 || ht == 0 || w > MAX_W || ht > MAX_H {
                return None;
            }
            let mut data = vec![0u8; w * ht * 3 / 2];
            // SAFETY: the slot holds SLOT_BYTES >= w*h*3/2 readable bytes. A
            // concurrent writer is detected by the seq re-check below.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.m.slot_ptr(i as usize),
                    data.as_mut_ptr(),
                    data.len(),
                )
            };
            fence(Ordering::Acquire);
            if s.seq.load(Ordering::Relaxed) != s1 {
                continue;
            }
            if now_us().saturating_sub(ts) > max_age_us {
                return None;
            }
            return Some((
                s1,
                Frame {
                    width: w,
                    height: ht,
                    data,
                },
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read() {
        let w = Writer::create().unwrap();
        let r = Reader::open().unwrap();
        assert!(r.latest(1_000_000).is_none());
        let f = Frame::placeholder(640, 360);
        w.publish(&f).unwrap();
        assert!(r.wait(100));
        let (seq1, got) = r.latest(1_000_000).unwrap();
        assert_eq!(got, f);
        let g = Frame::new(1920, 1080);
        w.publish(&g).unwrap();
        let (seq2, got) = r.latest(1_000_000).unwrap();
        assert_eq!(got, g);
        assert_ne!(seq1, seq2);
        assert!(w.publish(&Frame::new(3840, 2160)).is_err());
        drop(w);
        assert!(r.latest(1_000_000).is_none());
    }
}
