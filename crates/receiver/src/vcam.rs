//! Virtual camera outputs: Media Foundation (Windows 11) via the frame bus,
//! DirectShow (softcam) fallback.

use crate::settings::Backend;
use framebus::shm::Writer;
use framebus::Frame;
use std::sync::Arc;
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

pub struct Outputs {
    bus: Option<Writer>,
    mf: Option<MfCamera>,
    pub status: String,
}

impl Outputs {
    pub fn new(backend: Backend) -> Outputs {
        let mf_available = mf_create_fn().is_some();
        let (use_mf, _use_ds) = match backend {
            Backend::Auto => (mf_available, !mf_available),
            Backend::MediaFoundation => (true, false),
            Backend::DirectShow => (false, true),
            Backend::Both => (true, true),
        };
        let mut status = Vec::new();
        let mut out = Outputs {
            bus: None,
            mf: None,
            status: String::new(),
        };
        if use_mf {
            match Writer::create() {
                Ok(w) => out.bus = Some(w),
                Err(e) => status.push(format!("frame bus: {e}")),
            }
            match MfCamera::create() {
                Ok(c) => {
                    out.mf = Some(c);
                    status.push("Media Foundation camera active".into());
                }
                Err(e) => status.push(format!("Media Foundation camera unavailable: {e}")),
            }
        }
        out.status = status.join("; ");
        out
    }

    pub fn none() -> Outputs {
        Outputs {
            bus: None,
            mf: None,
            status: String::new(),
        }
    }

    pub fn send(&mut self, f: &Arc<Frame>) {
        if let Some(b) = &self.bus {
            let _ = b.publish(f);
        }
    }

    /// Called ~10×/s while no phone video is flowing. The MF source draws its
    /// own placeholder when the bus goes stale, so only DirectShow needs this.
    pub fn placeholder(&mut self) {}
}

type CreateFn = unsafe extern "system" fn(
    MFVirtualCameraType,
    MFVirtualCameraLifetime,
    MFVirtualCameraAccess,
    PCWSTR,
    PCWSTR,
    *const windows::core::GUID,
    u32,
    *mut *mut std::ffi::c_void,
) -> windows::core::HRESULT;

/// `MFCreateVirtualCamera` exists only on Windows 11+, so it is looked up at
/// runtime; a static import would stop the app loading on Windows 10.
fn mf_create_fn() -> Option<CreateFn> {
    // SAFETY: loading a system DLL by name and looking up a documented export;
    // the transmute matches the documented MFCreateVirtualCamera signature.
    unsafe {
        let lib = LoadLibraryW(&HSTRING::from("mfsensorgroup.dll")).ok()?;
        let p = GetProcAddress(lib, windows::core::s!("MFCreateVirtualCamera"))?;
        Some(std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            CreateFn,
        >(p))
    }
}

struct MfCamera(IMFVirtualCamera);

// SAFETY: created on an MTA thread; MTA interface pointers may be used from any
// MTA thread, and we only call Remove on drop.
unsafe impl Send for MfCamera {}

impl MfCamera {
    fn create() -> windows::core::Result<MfCamera> {
        let create =
            mf_create_fn().ok_or_else(|| windows::core::Error::from(MF_E_UNSUPPORTED_SERVICE))?;
        // Created on a fresh MTA thread so we don't touch the UI thread's COM apartment.
        let r = std::thread::spawn(move || -> windows::core::Result<SendCam> {
            // SAFETY: COM init + documented MFCreateVirtualCamera call with valid
            // strings; on success `raw` holds an owned IMFVirtualCamera reference.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
                let name = HSTRING::from(framebus::CAMERA_NAME);
                let clsid = HSTRING::from(framebus::MF_SOURCE_CLSID);
                let mut raw = std::ptr::null_mut();
                create(
                    MFVirtualCameraType_SoftwareCameraSource,
                    MFVirtualCameraLifetime_Session,
                    MFVirtualCameraAccess_CurrentUser,
                    PCWSTR(name.as_ptr()),
                    PCWSTR(clsid.as_ptr()),
                    std::ptr::null(),
                    0,
                    &mut raw,
                )
                .ok()?;
                let cam = IMFVirtualCamera::from_raw(raw);
                cam.Start(None)?;
                Ok(SendCam(cam))
            }
        })
        .join()
        .map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))??;
        Ok(MfCamera(r.0))
    }
}

struct SendCam(IMFVirtualCamera);
// SAFETY: see MfCamera.
unsafe impl Send for SendCam {}

impl Drop for MfCamera {
    fn drop(&mut self) {
        // SAFETY: valid camera interface.
        unsafe {
            let _ = self.0.Remove();
        }
    }
}
