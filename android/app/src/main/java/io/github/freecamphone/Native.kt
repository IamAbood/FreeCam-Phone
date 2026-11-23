package io.github.freecamphone

import java.nio.ByteBuffer

/** JNI bridge to the Rust core (crates/phone-core). */
object Native {
    init {
        System.loadLibrary("phone_core")
    }

    const val PORT = 47800

    @JvmStatic external fun start(name: String, port: Int): Boolean
    @JvmStatic external fun stop()
    @JvmStatic external fun videoConfig(width: Int, height: Int, fps: Int, buf: ByteBuffer, off: Int, size: Int)
    @JvmStatic external fun videoFrame(buf: ByteBuffer, off: Int, size: Int, ageUs: Long, key: Boolean)
    @JvmStatic external fun audioFrame(data: ByteArray, len: Int)
    @JvmStatic external fun stats(
        fps: Float, kbps: Int, facing: Int, zoomMax: Float,
        evMin: Int, evMax: Int, torch: Boolean, mic: Boolean,
    )

    /** Blocks up to [timeoutMs]; returns a command like "zoom:2.0" or null. */
    @JvmStatic external fun nextControl(timeoutMs: Int): String?
    @JvmStatic external fun status(): String
}
