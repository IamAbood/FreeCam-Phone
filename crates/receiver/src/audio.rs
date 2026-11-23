//! Phone microphone → a Windows render device (WASAPI, shared mode).
//!
//! Windows has no user-mode API for creating a capture device, so v1 plays the
//! phone audio into a virtual cable (e.g. VB-CABLE's "CABLE Input"); apps then
//! pick the cable's output ("CABLE Output") as their microphone.

use crate::conn::lock;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

const RATE: u32 = 48_000;
/// Keep at most this much queued; drop the oldest beyond it (phone/PC clock drift).
const MAX_QUEUE: usize = RATE as usize / 10;

type Queue = Arc<Mutex<VecDeque<i16>>>;

pub struct Sink {
    queue: Option<Queue>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Sink {
    /// `device`: substring of the render device's friendly name; empty = off.
    pub fn new(device: &str) -> Sink {
        let stop = Arc::new(AtomicBool::new(false));
        if device.is_empty() {
            return Sink {
                queue: None,
                stop,
                thread: None,
            };
        }
        let queue: Queue = Arc::default();
        let (q, s, name) = (queue.clone(), stop.clone(), device.to_lowercase());
        let thread = std::thread::spawn(move || {
            let _ = render(&name, &q, &s);
        });
        Sink {
            queue: Some(queue),
            stop,
            thread: Some(thread),
        }
    }

    /// Queues 48 kHz mono s16le PCM.
    pub fn push(&mut self, pcm: &[u8]) {
        let Some(q) = &self.queue else { return };
        let mut q = lock(q);
        q.extend(
            pcm.as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b)),
        );
        let excess = q.len().saturating_sub(MAX_QUEUE);
        q.drain(..excess);
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn devices() -> windows::core::Result<Vec<(String, IMMDevice)>> {
    // SAFETY: COM enumeration of audio endpoints with valid arguments.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let coll = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..coll.GetCount()? {
            let d = coll.Item(i)?;
            let name = d
                .OpenPropertyStore(STGM_READ)?
                .GetValue(&PKEY_Device_FriendlyName)?
                .to_string();
            out.push((name, d));
        }
        Ok(out)
    }
}

/// Friendly names of active playback devices, for the settings combo box.
pub fn render_devices() -> Vec<String> {
    devices()
        .map(|v| v.into_iter().map(|(n, _)| n).collect())
        .unwrap_or_default()
}

fn render(name: &str, q: &Queue, stop: &AtomicBool) -> windows::core::Result<()> {
    let Some((_, dev)) = devices()?
        .into_iter()
        .find(|(n, _)| n.to_lowercase().contains(name))
    else {
        return Ok(());
    };
    // SAFETY: standard WASAPI shared-mode setup; buffers returned by GetBuffer
    // are valid for `n` frames of our 16-bit mono format until ReleaseBuffer.
    unsafe {
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: 1,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 2,
            nBlockAlign: 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            200_000, // 20 ms
            0,
            &fmt,
            None,
        )?;
        let event = CreateEventW(None, false, false, None)?;
        client.SetEventHandle(event)?;
        let rc: IAudioRenderClient = client.GetService()?;
        let size = client.GetBufferSize()?;
        client.Start()?;
        while !stop.load(Ordering::Relaxed) {
            if WaitForSingleObject(event, 100) != WAIT_OBJECT_0 {
                continue;
            }
            let free = size - client.GetCurrentPadding()?;
            let mut data = lock(q);
            let n = (free as usize).min(data.len());
            if n == 0 {
                continue;
            }
            let p = rc.GetBuffer(n as u32)? as *mut i16;
            for (i, s) in data.drain(..n).enumerate() {
                *p.add(i) = s;
            }
            drop(data);
            rc.ReleaseBuffer(n as u32, 0)?;
        }
        let _ = client.Stop();
        let _ = CloseHandle(event);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn sink_starts_and_stops() {
        let names = super::render_devices();
        println!("render devices: {names:?}");
        // Unknown device: thread exits, pushes are harmless, drop doesn't hang.
        let mut s = super::Sink::new("no-such-device");
        s.push(&[0u8; 960]);
        drop(s);
        if let Some(first) = names.first() {
            let mut s = super::Sink::new(first);
            s.push(&[0u8; 960 * 10]); // 100 ms of silence
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}
