package io.github.freecamphone

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder

/** Captures 48 kHz mono s16le in 10 ms chunks and hands them to the core. */
class AudioStreamer {
    @Volatile var enabled = true
    @Volatile private var running = false
    private var thread: Thread? = null

    @SuppressLint("MissingPermission")
    fun start() {
        if (thread != null) return
        running = true
        thread = Thread({
            val min = AudioRecord.getMinBufferSize(48000, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            val rec = try {
                AudioRecord(
                    MediaRecorder.AudioSource.MIC, 48000, AudioFormat.CHANNEL_IN_MONO,
                    AudioFormat.ENCODING_PCM_16BIT, maxOf(min, 9600),
                )
            } catch (e: Exception) {
                return@Thread
            }
            if (rec.state != AudioRecord.STATE_INITIALIZED) {
                rec.release()
                return@Thread
            }
            rec.startRecording()
            val buf = ByteArray(960) // 10 ms
            while (running) {
                val n = rec.read(buf, 0, buf.size)
                if (n > 0 && enabled) Native.audioFrame(buf, n)
            }
            rec.stop()
            rec.release()
        }, "audio").apply { start() }
    }

    fun stop() {
        running = false
        thread?.join(500)
        thread = null
    }
}
