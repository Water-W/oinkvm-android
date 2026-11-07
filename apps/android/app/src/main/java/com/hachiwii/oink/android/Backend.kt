package com.hachiwii.oink.android

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.hardware.display.DisplayManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.DisplayMetrics
import android.util.Log
import android.view.Display
import android.view.WindowManager
import java.util.Locale
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.Executors

/**
 * Process-wide owner of the in-process backend. All native calls that may
 * block run on background executors; callbacks are forwarded to the
 * accessibility service, the clipboard and interface listeners.
 */
object Backend : Native.Callbacks {
    private const val TAG = "Oink"

    private lateinit var app: Context
    private val main = Handler(Looper.getMainLooper())
    private val starter = Executors.newSingleThreadExecutor()
    private val calls = Executors.newCachedThreadPool()
    private val listeners = CopyOnWriteArrayList<(String) -> Unit>()

    @Volatile
    var started = false
        private set

    @Volatile
    var startError: String? = null
        private set

    fun init(context: Context) {
        app = context.applicationContext
    }

    /** Idempotent; the result is reported through [started] / [startError]. */
    fun start(onDone: (() -> Unit)? = null) {
        starter.execute {
            if (!started) {
                val screen = screen()
                val error = try {
                    Native.nativeStart(
                        dataDir = app.filesDir.resolve("oink").absolutePath,
                        deviceName = deviceName(),
                        language = Locale.getDefault().toLanguageTag(),
                        width = screen.width,
                        height = screen.height,
                        density = screen.density,
                        xdpi = screen.xdpi,
                        ydpi = screen.ydpi,
                        rotation = screen.rotation,
                        callbacks = this,
                    )
                } catch (t: Throwable) {
                    t.toString()
                }
                startError = error
                started = error == null
                if (error != null) {
                    Log.e(TAG, "backend start failed: $error")
                } else {
                    Native.nativeSetAccessibility(InputService.instance != null)
                }
            }
            onDone?.let { main.post(it) }
        }
    }

    fun stop() {
        starter.execute {
            if (started) {
                Native.nativeStop()
                started = false
            }
        }
    }

    /** One API request; [reply] runs on the main thread with the JSON reply. */
    fun call(request: String, reply: (String) -> Unit) {
        calls.execute {
            val result = if (started) {
                try {
                    Native.nativeCall(request)
                } catch (t: Throwable) {
                    """{"err":{"code":"internal","message":${org.json.JSONObject.quote(t.toString())}}}"""
                }
            } else {
                val message = startError ?: "starting"
                """{"err":{"code":"unavailable","message":${org.json.JSONObject.quote(message)}}}"""
            }
            main.post { reply(result) }
        }
    }

    fun setLiveViews(json: String) {
        if (started) calls.execute { Native.nativeSetLiveViews(json) }
    }

    fun addListener(listener: (String) -> Unit) = listeners.add(listener)

    fun removeListener(listener: (String) -> Unit) = listeners.remove(listener)

    fun accessibilityChanged(connected: Boolean) {
        if (started) calls.execute { Native.nativeSetAccessibility(connected) }
    }

    /** Share text that the app could read (foreground) as a local copy. */
    fun localClipboardText(text: String?) {
        if (started) calls.execute { Native.nativeClipboardText(text) }
    }

    /** Read the clipboard; only works while one of this app's windows has focus. */
    fun readClipboard(context: Context): String? {
        val manager = context.getSystemService(ClipboardManager::class.java) ?: return null
        val clip = manager.primaryClip ?: return null
        if (clip.itemCount == 0) return null
        return clip.getItemAt(0).coerceToText(context)?.toString()
    }

    fun screenChanged() {
        if (!started) return
        val s = screen()
        calls.execute { Native.nativeSetScreen(s.width, s.height, s.density, s.xdpi, s.ydpi, s.rotation) }
    }

    data class Screen(
        val width: Int,
        val height: Int,
        val density: Float,
        val xdpi: Float,
        val ydpi: Float,
        val rotation: Int,
    )

    /** The full physical display (including system bars) of the default display. */
    @Suppress("DEPRECATION")
    fun screen(): Screen {
        val display = app.getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        val metrics = DisplayMetrics()
        display.getRealMetrics(metrics)
        var width = metrics.widthPixels
        var height = metrics.heightPixels
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val bounds = app.getSystemService(WindowManager::class.java).maximumWindowMetrics.bounds
            width = bounds.width()
            height = bounds.height()
        }
        return Screen(width, height, metrics.density, metrics.xdpi, metrics.ydpi, display.rotation)
    }

    private fun deviceName(): String {
        val name = Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME)
        return name?.takeIf { it.isNotBlank() } ?: Build.MODEL ?: "Android"
    }

    // ── Native.Callbacks (backend threads) ──────────────────────────────

    override fun pointer(x: Float, y: Float) {
        main.post { InputService.instance?.pointer(x, y) }
    }

    override fun button(button: Int, pressed: Boolean, x: Float, y: Float) {
        main.post { InputService.instance?.button(button, pressed, x, y) }
    }

    override fun scroll(x: Float, y: Float, dx: Float, dy: Float) {
        main.post { InputService.instance?.scroll(x, y, dx, dy) }
    }

    override fun key(usage: Int, pressed: Boolean, modifiers: Int, text: String?) {
        main.post { InputService.instance?.key(usage, pressed, modifiers, text) }
    }

    override fun pointerHidden() {
        main.post { InputService.instance?.hidePointer() }
    }

    override fun setClipboard(text: String) {
        main.post {
            val manager = app.getSystemService(ClipboardManager::class.java)
            manager?.setPrimaryClip(ClipData.newPlainText("Oink", text))
        }
    }

    override fun event(json: String) {
        for (listener in listeners) listener(json)
    }
}
