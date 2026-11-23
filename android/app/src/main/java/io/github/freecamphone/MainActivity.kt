package io.github.freecamphone

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.SurfaceTexture
import android.net.ConnectivityManager
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.view.Gravity
import android.view.TextureView
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.TextView
import java.net.Inet4Address

class MainActivity : Activity() {
    private lateinit var preview: TextureView
    private lateinit var statusView: TextView
    private lateinit var camera: CameraStreamer
    private val audio = AudioStreamer()
    private var multicastLock: WifiManager.MulticastLock? = null
    @Volatile private var running = false
    private var controlThread: Thread? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        preview = TextureView(this)
        statusView = TextView(this).apply {
            setTextColor(Color.WHITE)
            setShadowLayer(4f, 0f, 0f, Color.BLACK)
            textSize = 16f
            setPadding(24, 24, 24, 24)
        }
        setContentView(FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
            addView(preview)
            addView(statusView, FrameLayout.LayoutParams(-2, -2, Gravity.TOP or Gravity.START))
        })
        camera = CameraStreamer(this, preview)
        // Restart the session once the preview surface exists so it gets frames too.
        preview.surfaceTextureListener = object : TextureView.SurfaceTextureListener {
            override fun onSurfaceTextureAvailable(s: SurfaceTexture, w: Int, h: Int) { if (running) camera.restart() }
            override fun onSurfaceTextureSizeChanged(s: SurfaceTexture, w: Int, h: Int) {}
            override fun onSurfaceTextureDestroyed(s: SurfaceTexture) = true
            override fun onSurfaceTextureUpdated(s: SurfaceTexture) {}
        }

        val missing = arrayOf(Manifest.permission.CAMERA, Manifest.permission.RECORD_AUDIO)
            .filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isEmpty()) begin() else requestPermissions(missing.toTypedArray(), 1)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<String>, results: IntArray) {
        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) begin()
        else statusView.text = "Camera permission is required."
    }

    private fun begin() {
        if (running) return
        running = true
        multicastLock = getSystemService(WifiManager::class.java)?.createMulticastLock("freecam-mdns")?.apply {
            setReferenceCounted(false)
            acquire()
        }
        Native.start(Build.MODEL, Native.PORT)
        camera.start()
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) audio.start()
        controlThread = Thread({
            while (running) {
                val cmd = Native.nextControl(500) ?: continue
                handle(cmd)
            }
        }, "controls").apply { start() }
        updateStatus()
    }

    private fun handle(cmd: String) {
        val parts = cmd.split(":")
        val arg = parts.getOrNull(1)
        when (parts[0]) {
            "switch" -> camera.post { front = !front; restart() }
            "res" -> arg?.toIntOrNull()?.let { h -> camera.post { targetHeight = h; restart() } }
            "fps" -> arg?.toIntOrNull()?.let { f -> camera.post { targetFps = f; restart() } }
            "torch" -> camera.post { torch = arg == "1"; apply() }
            "ev" -> arg?.toIntOrNull()?.let { e -> camera.post { ev = e; apply() } }
            "zoom" -> arg?.toFloatOrNull()?.let { z -> camera.post { zoom = z; apply() } }
            "focus" -> {
                val x = arg?.toFloatOrNull()
                val y = parts.getOrNull(2)?.toFloatOrNull()
                if (x != null && y != null) camera.post { focus(x, y) }
            }
            "mic" -> { audio.enabled = arg == "1"; camera.post { micOn = arg == "1" } }
            "key" -> camera.post { requestKeyframe() }
        }
    }

    private fun ipAddresses(): String {
        val cm = getSystemService(ConnectivityManager::class.java) ?: return ""
        val lp = cm.getLinkProperties(cm.activeNetwork) ?: return ""
        return lp.linkAddresses.map { it.address }.filterIsInstance<Inet4Address>()
            .joinToString { it.hostAddress ?: "" }
    }

    private fun updateStatus() {
        if (!running) return
        statusView.text = "FreeCam Phone\n${Native.status()}\nWi-Fi: ${ipAddresses()}  port ${Native.PORT}"
        statusView.postDelayed(::updateStatus, 1000)
    }

    override fun onStart() {
        super.onStart()
        if (running) {
            camera.start()
            audio.start()
        }
    }

    override fun onStop() {
        super.onStop()
        camera.stop()
        audio.stop()
    }

    override fun onDestroy() {
        running = false
        controlThread?.join(1000)
        camera.quit()
        audio.stop()
        Native.stop()
        multicastLock?.release()
        super.onDestroy()
    }
}
