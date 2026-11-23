package io.github.freecamphone

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Rect
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CameraMetadata
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.params.MeteringRectangle
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.Range
import android.util.Size
import android.view.Surface
import android.view.TextureView

/**
 * Camera2 capture straight into a MediaCodec H.264 encoder surface (plus the
 * on-screen preview). Every method runs on [handler]; call them via [post].
 */
class CameraStreamer(context: Context, private val preview: TextureView) {
    private val mgr = context.getSystemService(CameraManager::class.java)
    private val thread = HandlerThread("camera").apply { start() }
    private val handler = Handler(thread.looper)

    // Settings (changed remotely).
    var front = false
    var targetHeight = 720
    var targetFps = 30
    var torch = false
    var ev = 0
    var zoom = 1f
    var micOn = true

    private var chars: CameraCharacteristics? = null
    private var device: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    private var codec: MediaCodec? = null
    private var encSurface: Surface? = null
    private var previewSurface: Surface? = null
    private var afRegion: MeteringRectangle? = null
    private var size = Size(1280, 720)
    private var fps = 30
    private var realtimeClock = false
    private var open = false
    /** Bumped on every close so callbacks from a previous open are ignored. */
    private var generation = 0

    private var frames = 0
    private var bytes = 0L
    private var lastStats = SystemClock.elapsedRealtime()

    fun post(block: CameraStreamer.() -> Unit) {
        handler.post { runCatching { block() } }
    }

    fun start() = post { openCamera() }
    fun stop() = post { closeCamera() }
    fun restart() = post { closeCamera(); openCamera() }

    @SuppressLint("MissingPermission")
    private fun openCamera() {
        if (open) return
        val facing = if (front) CameraMetadata.LENS_FACING_FRONT else CameraMetadata.LENS_FACING_BACK
        val id = mgr.cameraIdList.firstOrNull {
            mgr.getCameraCharacteristics(it).get(CameraCharacteristics.LENS_FACING) == facing
        } ?: mgr.cameraIdList.firstOrNull() ?: return
        val c = mgr.getCameraCharacteristics(id)
        chars = c
        realtimeClock = c.get(CameraCharacteristics.SENSOR_INFO_TIMESTAMP_SOURCE) ==
            CameraMetadata.SENSOR_INFO_TIMESTAMP_SOURCE_REALTIME
        val map = c.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP) ?: return
        size = chooseSize(map.getOutputSizes(MediaCodec::class.java), targetHeight)
        fps = chooseFps(c).upper
        afRegion = null
        startEncoder()
        open = true
        val gen = generation
        mgr.openCamera(id, object : CameraDevice.StateCallback() {
            override fun onOpened(d: CameraDevice) {
                if (gen != generation) { d.close(); return }
                device = d
                try {
                    createSession(d, gen)
                } catch (e: Exception) {
                    d.close(); device = null
                }
            }
            override fun onDisconnected(d: CameraDevice) { d.close(); if (gen == generation) device = null }
            override fun onError(d: CameraDevice, error: Int) { d.close(); if (gen == generation) device = null }
        }, handler)
        scheduleStats(gen)
    }

    private fun closeCamera() {
        open = false
        generation++
        runCatching { session?.close() }
        runCatching { device?.close() }
        runCatching { codec?.stop() }
        runCatching { codec?.release() }
        encSurface?.release()
        previewSurface?.release()
        session = null; device = null; codec = null; encSurface = null; previewSurface = null
    }

    private fun chooseSize(sizes: Array<Size>, h: Int): Size {
        val wide = sizes.filter { it.width * 9 == it.height * 16 }
        return wide.firstOrNull { it.height == h }
            ?: wide.filter { it.height <= h }.maxByOrNull { it.height }
            ?: sizes.minByOrNull { Math.abs(it.height - h) }
            ?: Size(1280, 720)
    }

    private fun chooseFps(c: CameraCharacteristics): Range<Int> {
        val ranges = c.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES) ?: return Range(30, 30)
        return ranges.firstOrNull { it.lower == targetFps && it.upper == targetFps }
            ?: ranges.filter { it.upper == targetFps }.maxByOrNull { it.lower }
            ?: ranges.firstOrNull { it.lower == 30 && it.upper == 30 }
            ?: ranges.maxByOrNull { it.upper }!!
    }

    private fun startEncoder() {
        val base = when {
            size.height >= 1080 -> 6_000_000
            size.height >= 720 -> 3_000_000
            else -> 1_500_000
        }
        val bitrate = if (fps >= 60) base * 3 / 2 else base
        fun format(withProfile: Boolean) = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, size.width, size.height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
            setInteger(MediaFormat.KEY_FRAME_RATE, fps)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1)
            setInteger(MediaFormat.KEY_PRIORITY, 0)
            setInteger(MediaFormat.KEY_LATENCY, 1)
            if (Build.VERSION.SDK_INT >= 29) setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
            if (withProfile) {
                setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline)
                setInteger(MediaFormat.KEY_LEVEL, MediaCodecInfo.CodecProfileLevel.AVCLevel42)
            }
        }
        val c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        c.setCallback(object : MediaCodec.Callback() {
            override fun onInputBufferAvailable(codec: MediaCodec, index: Int) {}
            override fun onError(codec: MediaCodec, e: MediaCodec.CodecException) {}
            override fun onOutputFormatChanged(codec: MediaCodec, format: MediaFormat) {}
            override fun onOutputBufferAvailable(codec: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
                try {
                    val buf = codec.getOutputBuffer(index)
                    if (buf != null && info.size > 0) {
                        if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                            Native.videoConfig(size.width, size.height, fps, buf, info.offset, info.size)
                        } else {
                            val now = if (realtimeClock) SystemClock.elapsedRealtimeNanos() / 1000 else System.nanoTime() / 1000
                            val key = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                            Native.videoFrame(buf, info.offset, info.size, now - info.presentationTimeUs, key)
                            frames++
                            bytes += info.size
                        }
                    }
                    codec.releaseOutputBuffer(index, false)
                } catch (e: IllegalStateException) {
                    // Codec was released while this callback was queued.
                }
            }
        }, handler)
        try {
            c.configure(format(true), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        } catch (e: Exception) {
            c.configure(format(false), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        }
        encSurface = c.createInputSurface()
        c.start()
        codec = c
    }

    @Suppress("DEPRECATION")
    private fun createSession(d: CameraDevice, gen: Int) {
        val targets = mutableListOf(encSurface ?: return)
        preview.surfaceTexture?.let {
            it.setDefaultBufferSize(size.width, size.height)
            val s = Surface(it)
            previewSurface = s
            targets += s
        }
        d.createCaptureSession(targets, object : CameraCaptureSession.StateCallback() {
            override fun onConfigured(s: CameraCaptureSession) {
                if (gen != generation) { s.close(); return }
                session = s
                apply()
            }
            override fun onConfigureFailed(s: CameraCaptureSession) {}
        }, handler)
    }

    private fun cropRegion(): Rect? {
        val c = chars ?: return null
        val active = c.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE) ?: return null
        val z = zoom.coerceIn(1f, zoomMax())
        val w = (active.width() / z).toInt()
        val h = (active.height() / z).toInt()
        val cx = active.centerX() - active.left
        val cy = active.centerY() - active.top
        return Rect(cx - w / 2, cy - h / 2, cx + w / 2, cy + h / 2)
    }

    private fun zoomMax() = chars?.get(CameraCharacteristics.SCALER_AVAILABLE_MAX_DIGITAL_ZOOM) ?: 1f
    private fun evRange() = chars?.get(CameraCharacteristics.CONTROL_AE_COMPENSATION_RANGE) ?: Range(0, 0)
    private fun hasTorch() = chars?.get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true

    private fun request(): CaptureRequest.Builder? {
        val d = device ?: return null
        val b = d.createCaptureRequest(CameraDevice.TEMPLATE_RECORD)
        encSurface?.let(b::addTarget)
        previewSurface?.let(b::addTarget)
        b.set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, chars?.let(::chooseFps))
        b.set(CaptureRequest.CONTROL_AE_EXPOSURE_COMPENSATION, ev.coerceIn(evRange().lower, evRange().upper))
        b.set(CaptureRequest.FLASH_MODE, if (torch && hasTorch()) CameraMetadata.FLASH_MODE_TORCH else CameraMetadata.FLASH_MODE_OFF)
        cropRegion()?.let { b.set(CaptureRequest.SCALER_CROP_REGION, it) }
        val r = afRegion
        if (r != null) {
            b.set(CaptureRequest.CONTROL_AF_MODE, CameraMetadata.CONTROL_AF_MODE_AUTO)
            b.set(CaptureRequest.CONTROL_AF_REGIONS, arrayOf(r))
            if ((chars?.get(CameraCharacteristics.CONTROL_MAX_REGIONS_AE) ?: 0) > 0) {
                b.set(CaptureRequest.CONTROL_AE_REGIONS, arrayOf(r))
            }
        } else {
            b.set(CaptureRequest.CONTROL_AF_MODE, CameraMetadata.CONTROL_AF_MODE_CONTINUOUS_VIDEO)
        }
        return b
    }

    /** Re-applies all settings to the repeating request. */
    fun apply() {
        val s = session ?: return
        val b = request() ?: return
        runCatching { s.setRepeatingRequest(b.build(), null, handler) }
    }

    /** Tap-to-focus at normalised image coordinates. */
    fun focus(x: Float, y: Float) {
        val crop = cropRegion() ?: return
        if ((chars?.get(CameraCharacteristics.CONTROL_MAX_REGIONS_AF) ?: 0) == 0) return
        val px = crop.left + (x.coerceIn(0f, 1f) * crop.width()).toInt()
        val py = crop.top + (y.coerceIn(0f, 1f) * crop.height()).toInt()
        val half = crop.width() / 20
        afRegion = MeteringRectangle(
            Rect(maxOf(crop.left, px - half), maxOf(crop.top, py - half), minOf(crop.right, px + half), minOf(crop.bottom, py + half)),
            MeteringRectangle.METERING_WEIGHT_MAX - 1,
        )
        apply()
        val b = request() ?: return
        b.set(CaptureRequest.CONTROL_AF_TRIGGER, CameraMetadata.CONTROL_AF_TRIGGER_START)
        runCatching { session?.capture(b.build(), null, handler) }
    }

    fun requestKeyframe() {
        runCatching { codec?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) }) }
    }

    private fun scheduleStats(gen: Int) {
        handler.postDelayed({
            if (!open || gen != generation) return@postDelayed
            val now = SystemClock.elapsedRealtime()
            val dt = (now - lastStats).coerceAtLeast(1) / 1000f
            Native.stats(
                frames / dt, (bytes * 8 / 1000 / dt).toInt(), if (front) 1 else 0, zoomMax(),
                evRange().lower, evRange().upper, hasTorch(), micOn,
            )
            frames = 0; bytes = 0; lastStats = now
            scheduleStats(gen)
        }, 1000)
    }

    fun quit() {
        post { closeCamera() }
        thread.quitSafely()
    }
}
