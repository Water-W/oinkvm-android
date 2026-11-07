package com.hachiwii.oink.android

/** JNI surface of `crates/oink-android` (liboink_android.so). */
object Native {
    init {
        System.loadLibrary("oink_android")
    }

    /** Called by the backend on its own threads; implementations must return quickly. */
    interface Callbacks {
        /** The remote pointer moved; physical screen pixels. */
        fun pointer(x: Float, y: Float)

        /** 0 primary, 1 secondary, 2 middle, 3 back, 4 forward. */
        fun button(button: Int, pressed: Boolean, x: Float, y: Float)

        /** Positive dy reveals content below. */
        fun scroll(x: Float, y: Float, dx: Float, dy: Float)

        /** HID usage (keyboard page) or `0x0C shl 16 or usage` (consumer page). */
        fun key(usage: Int, pressed: Boolean, modifiers: Int, text: String?)

        fun pointerHidden()

        fun setClipboard(text: String)

        /** One serialized ApiEvent. */
        fun event(json: String)
    }

    /** Returns null on success, otherwise an error message. Blocks: call off the main thread. */
    @JvmStatic
    external fun nativeStart(
        dataDir: String,
        deviceName: String,
        language: String,
        width: Int,
        height: Int,
        density: Float,
        xdpi: Float,
        ydpi: Float,
        rotation: Int,
        callbacks: Callbacks,
    ): String?

    /** `{"method": ..., "params": ...}` → `{"ok": ...}` or `{"err": ...}`. Blocks. */
    @JvmStatic
    external fun nativeCall(request: String): String

    @JvmStatic
    external fun nativeSetLiveViews(views: String)

    @JvmStatic
    external fun nativeSetScreen(width: Int, height: Int, density: Float, xdpi: Float, ydpi: Float, rotation: Int)

    @JvmStatic
    external fun nativeSetAccessibility(connected: Boolean)

    @JvmStatic
    external fun nativeClipboardText(text: String?)

    @JvmStatic
    external fun nativeStop()
}
