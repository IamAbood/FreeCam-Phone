//! "FreeCam Phone Camera" — a Media Foundation virtual camera source
//! (Windows 11, MFCreateVirtualCamera). Loaded by the Windows Camera Frame
//! Server; reads NV12 frames from the receiver through `framebus`.
//!
//! This whole crate is a COM boundary: `unsafe` is confined to COM/Win32 calls,
//! each with a SAFETY note.
#![allow(non_snake_case)]

use framebus::shm::Reader;
use framebus::Frame;
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::{
    implement, IUnknown, IUnknownImpl, Interface, Result, GUID, HRESULT, PCWSTR, PROPVARIANT, PWSTR,
};
use windows::Win32::Foundation::*;
use windows::Win32::Media::KernelStreaming::{
    IKsControl, IKsControl_Impl, KSIDENTIFIER, PINNAME_VIDEO_CAPTURE,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{IClassFactory, IClassFactory_Impl};

const CLSID: GUID = GUID::from_u128(0x6E92D7D5_85AD_48BC_8300_3942300A7E13);
const SIZES: [(u32, u32); 3] = [(1920, 1080), (1280, 720), (640, 480)];
const FPS: u32 = 30;
const FRAME_100NS: i64 = 10_000_000 / FPS as i64;
/// ERROR_SET_NOT_FOUND as HRESULT: "no KS properties here".
const E_SET_NOT_FOUND: HRESULT = HRESULT(0x8007_0492_u32 as i32);

fn new_attrs(n: u32) -> Result<IMFAttributes> {
    let mut a = None;
    // SAFETY: valid out-pointer.
    unsafe { MFCreateAttributes(&mut a, n)? };
    a.ok_or_else(|| E_FAIL.into())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------- class factory

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Option<&IUnknown>,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> Result<()> {
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        // SAFETY: MFCreateAttributes has no preconditions.
        let attrs = new_attrs(1)?;
        let act: IMFActivate = Activator {
            attrs,
            source: Mutex::new(None),
        }
        .into();
        // SAFETY: riid/ppv come from the COM caller and are forwarded unchanged.
        unsafe { act.query(riid, ppv).ok() }
    }
    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------- activator

#[implement(IMFActivate)]
struct Activator {
    attrs: IMFAttributes,
    source: Mutex<Option<IMFMediaSource>>,
}

impl IMFActivate_Impl for Activator_Impl {
    fn ActivateObject(&self, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        let mut g = lock(&self.source);
        let src = match g.as_ref() {
            Some(s) => s.clone(),
            None => {
                let s: IMFMediaSourceEx = Source::new()?.into();
                let s: IMFMediaSource = s.cast()?;
                *g = Some(s.clone());
                s
            }
        };
        // SAFETY: forwarding the caller's riid/ppv to QueryInterface.
        unsafe { src.query(riid, ppv).ok() }
    }
    fn ShutdownObject(&self) -> Result<()> {
        if let Some(s) = lock(&self.source).take() {
            // SAFETY: valid source.
            unsafe { s.Shutdown()? };
        }
        Ok(())
    }
    fn DetachObject(&self) -> Result<()> {
        lock(&self.source).take();
        Ok(())
    }
}

/// Forwards one IMFAttributes method to `self.attrs` through its vtable.
macro_rules! fwd {
    ($self:ident . $m:ident ( $($a:expr),* )) => {{
        let a = &$self.attrs;
        // SAFETY: calling the inner attribute store's own vtable entry with
        // the caller-provided arguments unchanged.
        unsafe { (Interface::vtable(a).$m)(Interface::as_raw(a), $($a),*).ok() }
    }};
}

fn raw<T: Interface>(p: Option<&T>) -> *mut c_void {
    p.map_or(std::ptr::null_mut(), |p| p.as_raw())
}

impl IMFAttributes_Impl for Activator_Impl {
    fn GetItem(&self, k: *const GUID, v: *mut PROPVARIANT) -> Result<()> {
        fwd!(self.GetItem(k, v.cast()))
    }
    fn GetItemType(&self, k: *const GUID) -> Result<MF_ATTRIBUTE_TYPE> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.GetItemType(k) }
    }
    fn CompareItem(&self, k: *const GUID, v: *const PROPVARIANT) -> Result<BOOL> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.CompareItem(k, v) }
    }
    fn Compare(&self, theirs: Option<&IMFAttributes>, t: MF_ATTRIBUTES_MATCH_TYPE) -> Result<BOOL> {
        let mut r = BOOL::default();
        fwd!(self.Compare(raw(theirs), t, &mut r))?;
        Ok(r)
    }
    fn GetUINT32(&self, k: *const GUID) -> Result<u32> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.GetUINT32(k) }
    }
    fn GetUINT64(&self, k: *const GUID) -> Result<u64> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.GetUINT64(k) }
    }
    fn GetDouble(&self, k: *const GUID) -> Result<f64> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.GetDouble(k) }
    }
    fn GetGUID(&self, k: *const GUID) -> Result<GUID> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.GetGUID(k) }
    }
    fn GetStringLength(&self, k: *const GUID) -> Result<u32> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.GetStringLength(k) }
    }
    fn GetString(&self, k: *const GUID, v: PWSTR, n: u32, len: *mut u32) -> Result<()> {
        fwd!(self.GetString(k, v, n, len))
    }
    fn GetAllocatedString(&self, k: *const GUID, v: *mut PWSTR, len: *mut u32) -> Result<()> {
        fwd!(self.GetAllocatedString(k, v, len))
    }
    fn GetBlobSize(&self, k: *const GUID) -> Result<u32> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.GetBlobSize(k) }
    }
    fn GetBlob(&self, k: *const GUID, b: *mut u8, n: u32, len: *mut u32) -> Result<()> {
        fwd!(self.GetBlob(k, b, n, len))
    }
    fn GetAllocatedBlob(&self, k: *const GUID, b: *mut *mut u8, len: *mut u32) -> Result<()> {
        fwd!(self.GetAllocatedBlob(k, b, len))
    }
    fn GetUnknown(&self, k: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> Result<()> {
        fwd!(self.GetUnknown(k, riid, ppv))
    }
    fn SetItem(&self, k: *const GUID, v: *const PROPVARIANT) -> Result<()> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.SetItem(k, v) }
    }
    fn DeleteItem(&self, k: *const GUID) -> Result<()> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.DeleteItem(k) }
    }
    fn DeleteAllItems(&self) -> Result<()> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.DeleteAllItems() }
    }
    fn SetUINT32(&self, k: *const GUID, v: u32) -> Result<()> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.SetUINT32(k, v) }
    }
    fn SetUINT64(&self, k: *const GUID, v: u64) -> Result<()> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.SetUINT64(k, v) }
    }
    fn SetDouble(&self, k: *const GUID, v: f64) -> Result<()> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.SetDouble(k, v) }
    }
    fn SetGUID(&self, k: *const GUID, v: *const GUID) -> Result<()> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.SetGUID(k, v) }
    }
    fn SetString(&self, k: *const GUID, v: &PCWSTR) -> Result<()> {
        fwd!(self.SetString(k, *v))
    }
    fn SetBlob(&self, k: *const GUID, b: *const u8, n: u32) -> Result<()> {
        fwd!(self.SetBlob(k, b, n))
    }
    fn SetUnknown(&self, k: *const GUID, u: Option<&IUnknown>) -> Result<()> {
        fwd!(self.SetUnknown(k, raw(u)))
    }
    fn LockStore(&self) -> Result<()> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.LockStore() }
    }
    fn UnlockStore(&self) -> Result<()> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.UnlockStore() }
    }
    fn GetCount(&self) -> Result<u32> {
        // SAFETY: forwarding to the inner store.
        unsafe { self.attrs.GetCount() }
    }
    fn GetItemByIndex(&self, i: u32, k: *mut GUID, v: *mut PROPVARIANT) -> Result<()> {
        fwd!(self.GetItemByIndex(i, k, v.cast()))
    }
    fn CopyAllItems(&self, dest: Option<&IMFAttributes>) -> Result<()> {
        fwd!(self.CopyAllItems(raw(dest)))
    }
}

// ---------------------------------------------------------------- media source

#[implement(IMFMediaSourceEx, IMFGetService, IKsControl)]
struct Source {
    queue: IMFMediaEventQueue,
    attrs: IMFAttributes,
    pd: IMFPresentationDescriptor,
    stream: Mutex<Option<IMFMediaStream2>>,
    stream_obj: windows::core::ComObject<Stream>,
}

fn media_type(w: u32, h: u32) -> Result<IMFMediaType> {
    // SAFETY: building a media type with standard attribute keys.
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
        t.SetUINT64(&MF_MT_FRAME_RATE, ((FPS as u64) << 32) | 1)?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1 << 32) | 1)?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        t.SetUINT32(&MF_MT_FIXED_SIZE_SAMPLES, 1)?;
        t.SetUINT32(&MF_MT_SAMPLE_SIZE, w * h * 3 / 2)?;
        t.SetUINT32(&MF_MT_DEFAULT_STRIDE, w)?;
        t.SetUINT32(&MF_MT_AVG_BITRATE, w * h * 12 * FPS)?;
        Ok(t)
    }
}

impl Source {
    fn new() -> Result<Source> {
        // SAFETY: standard MF object construction; all results are owned interfaces.
        unsafe {
            let attrs = new_attrs(4)?;
            attrs.SetGUID(
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
                &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
            )?;
            attrs.SetUINT32(&MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES, 1)?;

            let types: Vec<Option<IMFMediaType>> = SIZES
                .iter()
                .map(|&(w, h)| media_type(w, h).map(Some))
                .collect::<Result<_>>()?;
            let sd = MFCreateStreamDescriptor(0, &types)?;
            sd.GetMediaTypeHandler()?
                .SetCurrentMediaType(types[0].as_ref())?;
            let sattrs = new_attrs(4)?;
            for a in [&sattrs, &sd.cast::<IMFAttributes>()?] {
                a.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
                a.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, 0)?;
                a.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
                a.SetUINT32(
                    &MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES,
                    MFFrameSourceTypes_Color.0 as u32,
                )?;
            }
            let pd = MFCreatePresentationDescriptor(Some(&[Some(sd.clone())]))?;
            pd.SelectStream(0)?;
            let stream_obj = windows::core::ComObject::new(Stream {
                queue: MFCreateEventQueue()?,
                sd,
                attrs: sattrs,
                source: Mutex::new(None),
                inner: Mutex::new(StreamInner {
                    state: MF_STREAM_STATE_STOPPED,
                    reader: None,
                    last_open: None,
                    placeholder: None,
                    last_sample: None,
                }),
            });
            Ok(Source {
                queue: MFCreateEventQueue()?,
                attrs,
                pd,
                stream: Mutex::new(Some(stream_obj.to_interface())),
                stream_obj,
            })
        }
    }
}

impl IMFMediaEventGenerator_Impl for Source_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        // SAFETY: forwarding to our event queue.
        unsafe { self.queue.GetEvent(flags.0) }
    }
    fn BeginGetEvent(&self, cb: Option<&IMFAsyncCallback>, state: Option<&IUnknown>) -> Result<()> {
        // SAFETY: forwarding to our event queue.
        unsafe { self.queue.BeginGetEvent(cb, state) }
    }
    fn EndGetEvent(&self, r: Option<&IMFAsyncResult>) -> Result<IMFMediaEvent> {
        // SAFETY: forwarding to our event queue.
        unsafe { self.queue.EndGetEvent(r) }
    }
    fn QueueEvent(
        &self,
        met: u32,
        ext: *const GUID,
        hr: HRESULT,
        v: *const PROPVARIANT,
    ) -> Result<()> {
        // SAFETY: forwarding to our event queue.
        unsafe { self.queue.QueueEventParamVar(met, ext, hr, v) }
    }
}

impl IMFMediaSource_Impl for Source_Impl {
    fn GetCharacteristics(&self) -> Result<u32> {
        Ok(MFMEDIASOURCE_IS_LIVE.0 as u32)
    }
    fn CreatePresentationDescriptor(&self) -> Result<IMFPresentationDescriptor> {
        // SAFETY: cloning our descriptor.
        unsafe { self.pd.Clone() }
    }
    fn Start(
        &self,
        _pd: Option<&IMFPresentationDescriptor>,
        _fmt: *const GUID,
        _pos: *const PROPVARIANT,
    ) -> Result<()> {
        let stream = lock(&self.stream).clone().ok_or(MF_E_SHUTDOWN)?;
        *lock(&self.stream_obj.source) =
            Some(self.to_object().to_interface::<IMFMediaSourceEx>().cast()?);
        // SAFETY: queueing standard start events on valid queues.
        unsafe {
            let t = PROPVARIANT::from(MFGetSystemTime());
            self.queue
                .QueueEventParamUnk(MENewStream.0 as u32, &GUID::zeroed(), S_OK, &stream)?;
            self.stream_obj.set_state(MF_STREAM_STATE_RUNNING);
            self.stream_obj.queue.QueueEventParamVar(
                MEStreamStarted.0 as u32,
                &GUID::zeroed(),
                S_OK,
                &t,
            )?;
            self.queue
                .QueueEventParamVar(MESourceStarted.0 as u32, &GUID::zeroed(), S_OK, &t)?;
        }
        Ok(())
    }
    fn Stop(&self) -> Result<()> {
        self.stream_obj.set_state(MF_STREAM_STATE_STOPPED);
        // SAFETY: queueing standard stop events.
        unsafe {
            let t = PROPVARIANT::from(MFGetSystemTime());
            self.stream_obj.queue.QueueEventParamVar(
                MEStreamStopped.0 as u32,
                &GUID::zeroed(),
                S_OK,
                &t,
            )?;
            self.queue
                .QueueEventParamVar(MESourceStopped.0 as u32, &GUID::zeroed(), S_OK, &t)?;
        }
        Ok(())
    }
    fn Pause(&self) -> Result<()> {
        Err(MF_E_INVALID_STATE_TRANSITION.into())
    }
    fn Shutdown(&self) -> Result<()> {
        self.stream_obj.set_state(MF_STREAM_STATE_STOPPED);
        lock(&self.stream).take();
        // Break the stream → source reference cycle.
        lock(&self.stream_obj.source).take();
        // SAFETY: shutting down our own queues.
        unsafe {
            let _ = self.stream_obj.queue.Shutdown();
            let _ = self.queue.Shutdown();
        }
        Ok(())
    }
}

impl IMFMediaSourceEx_Impl for Source_Impl {
    fn GetSourceAttributes(&self) -> Result<IMFAttributes> {
        Ok(self.attrs.clone())
    }
    fn GetStreamAttributes(&self, id: u32) -> Result<IMFAttributes> {
        if id != 0 {
            return Err(E_INVALIDARG.into());
        }
        Ok(self.stream_obj.attrs.clone())
    }
    fn SetD3DManager(&self, _m: Option<&IUnknown>) -> Result<()> {
        // System-memory samples only.
        Ok(())
    }
}

impl IMFGetService_Impl for Source_Impl {
    fn GetService(
        &self,
        _s: *const GUID,
        _riid: *const GUID,
        _ppv: *mut *mut c_void,
    ) -> Result<()> {
        Err(MF_E_UNSUPPORTED_SERVICE.into())
    }
}

impl IKsControl_Impl for Source_Impl {
    fn KsProperty(
        &self,
        _: *const KSIDENTIFIER,
        _: u32,
        _: *mut c_void,
        _: u32,
        _: *mut u32,
    ) -> Result<()> {
        Err(E_SET_NOT_FOUND.into())
    }
    fn KsMethod(
        &self,
        _: *const KSIDENTIFIER,
        _: u32,
        _: *mut c_void,
        _: u32,
        _: *mut u32,
    ) -> Result<()> {
        Err(E_SET_NOT_FOUND.into())
    }
    fn KsEvent(
        &self,
        _: *const KSIDENTIFIER,
        _: u32,
        _: *mut c_void,
        _: u32,
        _: *mut u32,
    ) -> Result<()> {
        Err(E_SET_NOT_FOUND.into())
    }
}

// ---------------------------------------------------------------- stream

struct StreamInner {
    state: MF_STREAM_STATE,
    reader: Option<Reader>,
    last_open: Option<Instant>,
    placeholder: Option<Frame>,
    last_sample: Option<Instant>,
}

#[implement(IMFMediaStream2, IKsControl)]
struct Stream {
    queue: IMFMediaEventQueue,
    sd: IMFStreamDescriptor,
    attrs: IMFAttributes,
    source: Mutex<Option<IMFMediaSource>>,
    inner: Mutex<StreamInner>,
}

impl Stream {
    fn set_state(&self, s: MF_STREAM_STATE) {
        lock(&self.inner).state = s;
    }

    fn current_size(&self) -> (usize, usize) {
        // SAFETY: reading our own descriptor's current type.
        let sz = unsafe {
            self.sd
                .GetMediaTypeHandler()
                .and_then(|h| h.GetCurrentMediaType())
                .and_then(|t| t.GetUINT64(&MF_MT_FRAME_SIZE))
        };
        match sz {
            Ok(v) => ((v >> 32) as usize, (v & 0xFFFF_FFFF) as usize),
            Err(_) => (SIZES[0].0 as usize, SIZES[0].1 as usize),
        }
    }

    /// Latest phone frame scaled to `w x h`, or the placeholder card.
    fn next_frame(inner: &mut StreamInner, w: usize, h: usize) -> Frame {
        if inner.reader.is_none()
            && inner
                .last_open
                .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
        {
            inner.last_open = Some(Instant::now());
            inner.reader = Reader::open().ok();
        }
        if let Some((_, f)) = inner.reader.as_ref().and_then(|r| r.latest(1_000_000)) {
            return f.scaled(w, h);
        }
        match &inner.placeholder {
            Some(p) if p.width == w && p.height == h => p.clone(),
            _ => {
                let p = Frame::placeholder(w, h);
                inner.placeholder = Some(p.clone());
                p
            }
        }
    }
}

impl IMFMediaEventGenerator_Impl for Stream_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        // SAFETY: forwarding to our event queue.
        unsafe { self.queue.GetEvent(flags.0) }
    }
    fn BeginGetEvent(&self, cb: Option<&IMFAsyncCallback>, state: Option<&IUnknown>) -> Result<()> {
        // SAFETY: forwarding to our event queue.
        unsafe { self.queue.BeginGetEvent(cb, state) }
    }
    fn EndGetEvent(&self, r: Option<&IMFAsyncResult>) -> Result<IMFMediaEvent> {
        // SAFETY: forwarding to our event queue.
        unsafe { self.queue.EndGetEvent(r) }
    }
    fn QueueEvent(
        &self,
        met: u32,
        ext: *const GUID,
        hr: HRESULT,
        v: *const PROPVARIANT,
    ) -> Result<()> {
        // SAFETY: forwarding to our event queue.
        unsafe { self.queue.QueueEventParamVar(met, ext, hr, v) }
    }
}

impl IMFMediaStream_Impl for Stream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        lock(&self.source)
            .clone()
            .ok_or_else(|| MF_E_SHUTDOWN.into())
    }
    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        Ok(self.sd.clone())
    }
    fn RequestSample(&self, token: Option<&IUnknown>) -> Result<()> {
        let (w, h) = self.current_size();
        let mut inner = lock(&self.inner);
        if inner.state != MF_STREAM_STATE_RUNNING {
            return Err(MF_E_MEDIA_SOURCE_WRONGSTATE.into());
        }
        // ponytail: pace by sleeping in RequestSample (<= one frame); move to an MF work queue
        // timer if a client ever needs non-blocking RequestSample.
        if let Some(t) = inner.last_sample {
            let period = Duration::from_nanos(FRAME_100NS as u64 * 100);
            if let Some(rest) = period.checked_sub(t.elapsed()) {
                if let Some(r) = &inner.reader {
                    r.wait(rest.as_millis() as u32);
                } else {
                    std::thread::sleep(rest);
                }
            }
        }
        inner.last_sample = Some(Instant::now());
        let frame = Stream::next_frame(&mut inner, w, h);
        drop(inner);
        // SAFETY: creating and filling a system-memory sample; the locked
        // pointer is valid for `max >= len` bytes until Unlock.
        unsafe {
            let len = frame.data.len();
            let buf = MFCreateMemoryBuffer(len as u32)?;
            let mut p = std::ptr::null_mut();
            let mut max = 0;
            buf.Lock(&mut p, Some(&mut max), None)?;
            std::ptr::copy_nonoverlapping(frame.data.as_ptr(), p, len.min(max as usize));
            buf.Unlock()?;
            buf.SetCurrentLength(len as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buf)?;
            sample.SetSampleTime(MFGetSystemTime())?;
            sample.SetSampleDuration(FRAME_100NS)?;
            if let Some(tok) = token {
                sample.SetUnknown(&MFSampleExtension_Token, tok)?;
            }
            self.queue
                .QueueEventParamUnk(MEMediaSample.0 as u32, &GUID::zeroed(), S_OK, &sample)
        }
    }
}

impl IMFMediaStream2_Impl for Stream_Impl {
    fn SetStreamState(&self, s: MF_STREAM_STATE) -> Result<()> {
        self.set_state(s);
        Ok(())
    }
    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        Ok(lock(&self.inner).state)
    }
}

impl IKsControl_Impl for Stream_Impl {
    fn KsProperty(
        &self,
        _: *const KSIDENTIFIER,
        _: u32,
        _: *mut c_void,
        _: u32,
        _: *mut u32,
    ) -> Result<()> {
        Err(E_SET_NOT_FOUND.into())
    }
    fn KsMethod(
        &self,
        _: *const KSIDENTIFIER,
        _: u32,
        _: *mut c_void,
        _: u32,
        _: *mut u32,
    ) -> Result<()> {
        Err(E_SET_NOT_FOUND.into())
    }
    fn KsEvent(
        &self,
        _: *const KSIDENTIFIER,
        _: u32,
        _: *mut c_void,
        _: u32,
        _: *mut u32,
    ) -> Result<()> {
        Err(E_SET_NOT_FOUND.into())
    }
}

// ---------------------------------------------------------------- DLL exports

mod exports;

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the source like the Frame Server would, without registration.
    #[test]
    fn source_produces_placeholder_samples() {
        // SAFETY: test-only MF calls on valid objects.
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).unwrap();
            let f: IClassFactory = Factory.into();
            let act: IMFActivate = f.CreateInstance(None).unwrap();
            let src: IMFMediaSourceEx = act.ActivateObject().unwrap();
            let pd = src.CreatePresentationDescriptor().unwrap();
            assert_eq!(pd.GetStreamDescriptorCount().unwrap(), 1);
            src.Start(&pd, &GUID::zeroed(), &PROPVARIANT::default())
                .unwrap();
            // MENewStream carries the stream.
            let ev = src
                .GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))
                .unwrap();
            assert_eq!(ev.GetType().unwrap(), MENewStream.0 as u32);
            let v = ev.GetValue().unwrap();
            let stream: IMFMediaStream = IUnknown::try_from(&v).unwrap().cast().unwrap();
            assert_eq!(
                src.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))
                    .unwrap()
                    .GetType()
                    .unwrap(),
                MESourceStarted.0 as u32
            );
            assert_eq!(
                stream
                    .GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))
                    .unwrap()
                    .GetType()
                    .unwrap(),
                MEStreamStarted.0 as u32
            );
            stream.RequestSample(None).unwrap();
            let ev = stream
                .GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))
                .unwrap();
            assert_eq!(ev.GetType().unwrap(), MEMediaSample.0 as u32);
            let sample: IMFSample = IUnknown::try_from(&ev.GetValue().unwrap())
                .unwrap()
                .cast()
                .unwrap();
            assert_eq!(sample.GetTotalLength().unwrap(), 1920 * 1080 * 3 / 2);

            // Once the receiver publishes, the phone frame (scaled) replaces the placeholder.
            let w = framebus::shm::Writer::create().unwrap();
            let mut f = Frame::new(1280, 720);
            f.data[..1280 * 720].fill(200);
            std::thread::sleep(Duration::from_millis(1100)); // reader re-open interval
            w.publish(&f).unwrap();
            stream.RequestSample(None).unwrap();
            let ev = stream
                .GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))
                .unwrap();
            let sample: IMFSample = IUnknown::try_from(&ev.GetValue().unwrap())
                .unwrap()
                .cast()
                .unwrap();
            let buf = sample.ConvertToContiguousBuffer().unwrap();
            let mut p = std::ptr::null_mut();
            buf.Lock(&mut p, None, None).unwrap();
            assert_eq!(*p, 200);
            buf.Unlock().unwrap();
            src.Shutdown().unwrap();
            act.ShutdownObject().unwrap();
        }
    }
}
