//! Shared-memory frame transport between the receiver and the virtual camera,
//! plus NV12 helpers (scaling, mirroring, placeholder card).

pub mod image;
#[cfg(windows)]
pub mod shm;

pub use image::Frame;

/// CLSID of the Media Foundation virtual camera source (crates/vcam-mf).
pub const MF_SOURCE_CLSID: &str = "{6E92D7D5-85AD-48BC-8300-3942300A7E13}";
/// CLSID of the DirectShow filter (crates/vcam-dshow).
pub const DSHOW_FILTER_CLSID: &str = "{3AD27E84-C813-4ACB-8580-F20BAFB1B524}";
pub const CAMERA_NAME: &str = "FreeCam Phone Camera";
