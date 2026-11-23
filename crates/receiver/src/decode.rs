//! H.264 → NV12 using the Media Foundation H.264 decoder MFT (no FFmpeg).
//! All `unsafe` here is Media Foundation COM calls.

use anyhow::{bail, Context, Result};
use framebus::Frame;
use std::mem::ManuallyDrop;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

pub struct Decoder {
    mft: IMFTransform,
    /// Display size from the stream's VideoConfig.
    width: usize,
    height: usize,
    /// Decoded surface size (height is often aligned up to 16).
    surf_w: usize,
    surf_h: usize,
}

/// Call once per thread that uses a Decoder.
pub fn init_thread() -> Result<()> {
    // SAFETY: COM/MF init with valid flags; repeated calls on a thread are harmless.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
    }
    Ok(())
}

impl Decoder {
    pub fn new(width: usize, height: usize, fps: u32) -> Result<Decoder> {
        // SAFETY: standard MF decoder setup; every interface comes from a
        // successful COM call and is reference-counted by the windows crate.
        unsafe {
            let mft: IMFTransform =
                CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)
                    .context("H.264 decoder MFT not available")?;
            if let Ok(attrs) = mft.GetAttributes() {
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            // ponytail: software decode path (MS decoder is multi-threaded and handles 1080p60 on any
            // recent CPU); hand it an IMFDXGIDeviceManager for DXVA if CPU use becomes a problem.
            let it = MFCreateMediaType()?;
            it.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            it.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            it.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | height as u64)?;
            it.SetUINT64(&MF_MT_FRAME_RATE, ((fps.max(1) as u64) << 32) | 1)?;
            it.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            mft.SetInputType(0, &it, 0)?;
            let mut d = Decoder {
                mft,
                width,
                height,
                surf_w: width,
                surf_h: height,
            };
            d.set_output_type()?;
            d.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            d.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(d)
        }
    }

    unsafe fn set_output_type(&mut self) -> Result<()> {
        let mut i = 0;
        loop {
            let t = self
                .mft
                .GetOutputAvailableType(0, i)
                .context("no NV12 output type")?;
            if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                self.mft.SetOutputType(0, &t, 0)?;
                if let Ok(sz) = t.GetUINT64(&MF_MT_FRAME_SIZE) {
                    self.surf_w = (sz >> 32) as usize;
                    self.surf_h = (sz & 0xFFFF_FFFF) as usize;
                }
                return Ok(());
            }
            i += 1;
        }
    }

    /// Feeds one access unit; returns any frames that came out.
    pub fn decode(&mut self, data: &[u8], pts_us: u64) -> Result<Vec<(u64, Frame)>> {
        let mut out = Vec::new();
        // SAFETY: MF sample/buffer creation and locking; the locked pointer is
        // valid for `max` bytes until Unlock and we copy at most data.len() <= max.
        unsafe {
            let buf = MFCreateMemoryBuffer(data.len() as u32)?;
            let mut p = std::ptr::null_mut();
            let mut max = 0;
            buf.Lock(&mut p, Some(&mut max), None)?;
            if (max as usize) < data.len() {
                buf.Unlock()?;
                bail!("buffer too small");
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
            buf.Unlock()?;
            buf.SetCurrentLength(data.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buf)?;
            sample.SetSampleTime(pts_us as i64 * 10)?;

            if let Err(e) = self.mft.ProcessInput(0, &sample, 0) {
                if e.code() != MF_E_NOTACCEPTING {
                    return Err(e.into());
                }
                self.drain(&mut out)?;
                self.mft.ProcessInput(0, &sample, 0)?;
            }
            self.drain(&mut out)?;
        }
        Ok(out)
    }

    unsafe fn drain(&mut self, out: &mut Vec<(u64, Frame)>) -> Result<()> {
        loop {
            let info = self.mft.GetOutputStreamInfo(0)?;
            let provides = info.dwFlags
                & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0)
                    as u32
                != 0;
            let sample = if provides {
                None
            } else {
                let s = MFCreateSample()?;
                s.AddBuffer(&MFCreateMemoryBuffer(
                    info.cbSize.max((self.surf_w * self.surf_h * 3 / 2) as u32),
                )?)?;
                Some(s)
            };
            let mut odb = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let r = self.mft.ProcessOutput(0, &mut odb, &mut status);
            let sample = ManuallyDrop::into_inner(std::mem::replace(
                &mut odb[0].pSample,
                ManuallyDrop::new(None),
            ));
            drop(ManuallyDrop::into_inner(std::mem::replace(
                &mut odb[0].pEvents,
                ManuallyDrop::new(None),
            )));
            match r {
                Ok(()) => {
                    if let Some(s) = sample {
                        let pts = s.GetSampleTime().unwrap_or(0) as u64 / 10;
                        out.push((pts, self.copy_out(&s)?));
                    }
                }
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.set_output_type()?,
                Err(e) => return Err(e.into()),
            }
        }
    }

    unsafe fn copy_out(&self, s: &IMFSample) -> Result<Frame> {
        let buf = s.ConvertToContiguousBuffer()?;
        let (w, h) = (self.width.min(self.surf_w), self.height.min(self.surf_h));
        let mut f = Frame::new(w, h);
        // Prefer the 2D lock for the real pitch; fall back to a packed layout.
        if let Ok(b2) = windows::core::Interface::cast::<IMF2DBuffer>(&buf) {
            let mut p = std::ptr::null_mut();
            let mut pitch = 0i32;
            b2.Lock2D(&mut p, &mut pitch)?;
            let pitch = pitch.unsigned_abs() as usize;
            copy_planes(p, pitch, self.surf_h, w, h, &mut f.data);
            b2.Unlock2D()?;
        } else {
            let mut p = std::ptr::null_mut();
            let mut len = 0;
            buf.Lock(&mut p, None, Some(&mut len))?;
            if (len as usize) >= self.surf_w * self.surf_h * 3 / 2 {
                copy_planes(p, self.surf_w, self.surf_h, w, h, &mut f.data);
            }
            buf.Unlock()?;
        }
        Ok(f)
    }
}

/// Copies a `w x h` NV12 image out of a surface with `pitch` and `surf_h` rows of luma.
///
/// # Safety
/// `src` must point to at least `pitch * surf_h * 3 / 2` readable bytes, with
/// `w <= pitch` and `h <= surf_h`.
unsafe fn copy_planes(
    src: *const u8,
    pitch: usize,
    surf_h: usize,
    w: usize,
    h: usize,
    dst: &mut [u8],
) {
    for y in 0..h {
        std::ptr::copy_nonoverlapping(src.add(y * pitch), dst[y * w..].as_mut_ptr(), w);
    }
    let uv = src.add(pitch * surf_h);
    for y in 0..h / 2 {
        std::ptr::copy_nonoverlapping(uv.add(y * pitch), dst[w * h + y * w..].as_mut_ptr(), w);
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: valid MFT; ending streaming before release is the documented order.
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes a stream recorded with freecam-dump if one is present (out.h264 at repo root).
    #[test]
    fn decodes_recorded_stream_if_present() {
        let Ok(data) = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../out.h264")) else {
            return;
        };
        init_thread().unwrap();
        let mut d = Decoder::new(1280, 720, 30).unwrap();
        // Split on IDR/non-IDR slice starts (one slice per frame from Android encoders).
        let mut starts: Vec<usize> = (0..data.len().saturating_sub(4))
            .filter(|&i| data[i..i + 4] == [0, 0, 0, 1] && matches!(data[i + 4] & 0x1F, 1 | 7))
            .collect();
        starts.push(data.len());
        let mut frames = 0;
        for (n, win) in starts.windows(2).enumerate() {
            frames += d
                .decode(&data[win[0]..win[1]], n as u64 * 33_333)
                .unwrap()
                .len();
        }
        assert!(
            frames > starts.len() / 2,
            "decoded {frames} of {}",
            starts.len()
        );
    }
}
