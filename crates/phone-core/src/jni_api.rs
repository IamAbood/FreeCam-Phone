//! JNI entry points for `io.github.freecamphone.Native`. All `unsafe` for the
//! Android boundary lives here.

use crate::{lock, Core};
use jni::objects::{JByteArray, JByteBuffer, JClass, JString};
use jni::sys::{jboolean, jfloat, jint, jlong, jstring, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use protocol::Stats;
use std::sync::Mutex;
use std::time::Duration;

static CORE: Mutex<Option<Core>> = Mutex::new(None);

fn with_core(f: impl FnOnce(&Core)) {
    if let Some(c) = lock(&CORE).as_ref() {
        f(c)
    }
}

/// Copies `size` bytes at `off` out of a direct ByteBuffer.
fn direct_bytes(env: &JNIEnv, buf: &JByteBuffer, off: jint, size: jint) -> Option<Vec<u8>> {
    let ptr = env.get_direct_buffer_address(buf).ok()?;
    let cap = env.get_direct_buffer_capacity(buf).ok()?;
    let (off, size) = (usize::try_from(off).ok()?, usize::try_from(size).ok()?);
    if off.checked_add(size)? > cap {
        return None;
    }
    // SAFETY: `ptr` is the start of a live direct buffer of `cap` bytes (the JVM
    // keeps it alive for the duration of this native call) and we checked
    // `off + size <= cap`, so the range is in bounds.
    let s = unsafe { std::slice::from_raw_parts(ptr.add(off), size) };
    Some(s.to_vec())
}

#[no_mangle]
pub extern "system" fn Java_io_github_freecamphone_Native_start(
    mut env: JNIEnv,
    _: JClass,
    name: JString,
    port: jint,
) -> jboolean {
    let name: String = match env.get_string(&name) {
        Ok(s) => s.into(),
        Err(_) => return JNI_FALSE,
    };
    let mut g = lock(&CORE);
    if g.is_some() {
        return JNI_TRUE;
    }
    match Core::start(&name, port as u16) {
        Ok(c) => {
            *g = Some(c);
            JNI_TRUE
        }
        Err(_) => JNI_FALSE,
    }
}

#[no_mangle]
pub extern "system" fn Java_io_github_freecamphone_Native_stop(_: JNIEnv, _: JClass) {
    let core = lock(&CORE).take();
    if let Some(c) = core {
        c.stop();
    }
}

#[no_mangle]
pub extern "system" fn Java_io_github_freecamphone_Native_videoConfig(
    env: JNIEnv,
    _: JClass,
    width: jint,
    height: jint,
    fps: jint,
    buf: JByteBuffer,
    off: jint,
    size: jint,
) {
    if let Some(csd) = direct_bytes(&env, &buf, off, size) {
        with_core(|c| c.video_config(width as u16, height as u16, fps as u8, &csd));
    }
}

#[no_mangle]
pub extern "system" fn Java_io_github_freecamphone_Native_videoFrame(
    env: JNIEnv,
    _: JClass,
    buf: JByteBuffer,
    off: jint,
    size: jint,
    age_us: jlong,
    key: jboolean,
) {
    if let Some(data) = direct_bytes(&env, &buf, off, size) {
        with_core(|c| c.video_frame(&data, age_us.max(0) as u64, key != JNI_FALSE));
    }
}

#[no_mangle]
pub extern "system" fn Java_io_github_freecamphone_Native_audioFrame(
    env: JNIEnv,
    _: JClass,
    data: JByteArray,
    len: jint,
) {
    if let Ok(mut v) = env.convert_byte_array(&data) {
        v.truncate(len.max(0) as usize);
        with_core(|c| c.audio_frame(&v));
    }
}

#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub extern "system" fn Java_io_github_freecamphone_Native_stats(
    _: JNIEnv,
    _: JClass,
    fps: jfloat,
    kbps: jint,
    facing: jint,
    zoom_max: jfloat,
    ev_min: jint,
    ev_max: jint,
    torch: jboolean,
    mic: jboolean,
) {
    with_core(|c| {
        c.stats(Stats {
            fps,
            bitrate_kbps: kbps.max(0) as u32,
            facing: facing as u8,
            zoom_max,
            ev_min: ev_min.clamp(-128, 127) as i8,
            ev_max: ev_max.clamp(-128, 127) as i8,
            has_torch: torch != JNI_FALSE,
            mic_on: mic != JNI_FALSE,
        })
    });
}

#[no_mangle]
pub extern "system" fn Java_io_github_freecamphone_Native_nextControl(
    env: JNIEnv,
    _: JClass,
    timeout_ms: jint,
) -> jstring {
    // Don't hold the CORE lock while blocking: other calls need it.
    let rx = lock(&CORE).as_ref().map(|c| c.shared.clone());
    let Some(shared) = rx else {
        std::thread::sleep(Duration::from_millis(timeout_ms.max(0) as u64));
        return std::ptr::null_mut();
    };
    let r = lock(&shared.controls_rx);
    let cmd = r.as_ref().and_then(|rx| {
        rx.recv_timeout(Duration::from_millis(timeout_ms.max(0) as u64))
            .ok()
    });
    drop(r);
    match cmd.and_then(|s| env.new_string(s).ok()) {
        Some(s) => s.into_raw(),
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_github_freecamphone_Native_status(
    env: JNIEnv,
    _: JClass,
) -> jstring {
    let s = lock(&CORE)
        .as_ref()
        .map(|c| c.status())
        .unwrap_or_else(|| "Stopped".into());
    env.new_string(s)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
