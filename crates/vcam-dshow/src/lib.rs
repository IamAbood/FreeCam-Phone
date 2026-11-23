//! DirectShow "FreeCam Phone Camera" via softcam. The cdylib is the filter
//! DLL apps load; the rlib is the sender side used by the receiver.

use std::ffi::c_void;

extern "C" {
    fn fc_create_camera(width: i32, height: i32, fps: f32) -> *mut c_void;
    fn fc_delete_camera(cam: *mut c_void);
    fn fc_send_frame(cam: *mut c_void, bits: *const c_void);
    fn fc_is_connected(cam: *mut c_void) -> bool;
}

pub struct Camera {
    handle: *mut c_void,
    width: usize,
    height: usize,
}

// SAFETY: softcam's sender handle is a heap object guarded by its own named
// mutex; it may be used from any thread (one at a time, enforced by &mut).
unsafe impl Send for Camera {}

impl Camera {
    /// Fails if another softcam-based camera with the same shared-memory name exists.
    pub fn new(width: usize, height: usize, fps: f32) -> Option<Camera> {
        // SAFETY: plain C call with valid scalar arguments.
        let handle = unsafe { fc_create_camera(width as i32, height as i32, fps) };
        (!handle.is_null()).then_some(Camera {
            handle,
            width,
            height,
        })
    }

    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// True while some app has the camera open.
    pub fn is_connected(&self) -> bool {
        // SAFETY: valid handle owned by self.
        unsafe { fc_is_connected(self.handle) }
    }

    /// `bgr` must be a bottom-up BGR24 image of exactly width*height*3 bytes.
    pub fn send(&mut self, bgr: &[u8]) {
        if bgr.len() != self.width * self.height * 3 {
            return;
        }
        // SAFETY: valid handle; `bgr` holds the exact frame size softcam reads.
        unsafe { fc_send_frame(self.handle, bgr.as_ptr().cast()) }
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        // SAFETY: handle came from fc_create_camera and is deleted once.
        unsafe { fc_delete_camera(self.handle) }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn create_send_delete() {
        let mut cam = super::Camera::new(320, 240, 0.0).expect("camera");
        assert!(!cam.is_connected());
        cam.send(&vec![0u8; 320 * 240 * 3]);
        // A second instance can't coexist.
        assert!(super::Camera::new(320, 240, 0.0).is_none());
    }
}
