package com.hachiwii.oink.android

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PixelFormat
import android.media.AudioManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min

/**
 * Turns remote input into Android interactions. Accessibility services cannot
 * inject raw pointer or key events, so:
 * - the pointer is an overlay arrow;
 * - a primary click is a tap (long press when held), a drag replays its path;
 * - wheel scrolling is a short swipe at the pointer;
 * - printable keys edit the focused text field; Backspace, Delete, Enter,
 *   arrows, Home/End and Ctrl/⌘ + A/C/X/V use node actions;
 * - Esc / secondary button = Back, middle button = Home, volume keys adjust volume.
 */
class InputService : AccessibilityService() {
    private val main = Handler(Looper.getMainLooper())
    private var cursor: CursorView? = null
    private var params: WindowManager.LayoutParams? = null
    private val hide = Runnable { cursor?.visibility = View.GONE }

    private var pointerX = 0f
    private var pointerY = 0f
    private var pressed = false
    private var pressedAt = 0L
    private val stroke = ArrayList<FloatArray>()

    private var scrollX = 0f
    private var scrollY = 0f
    private var scrollAtX = 0f
    private var scrollAtY = 0f
    private var scrollPending = false
    private var gestureBusy = false

    override fun onServiceConnected() {
        super.onServiceConnected()
        instance = this
        addCursor()
        Backend.accessibilityChanged(true)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}

    override fun onInterrupt() {}

    override fun onUnbind(intent: android.content.Intent?): Boolean {
        disconnect()
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        disconnect()
        super.onDestroy()
    }

    private fun disconnect() {
        if (instance === this) {
            instance = null
            Backend.accessibilityChanged(false)
        }
        cursor?.let { runCatching { getSystemService(WindowManager::class.java).removeView(it) } }
        cursor = null
    }

    // ── Pointer ─────────────────────────────────────────────────────────

    private fun addCursor() {
        val size = (CURSOR_DP * resources.displayMetrics.density).toInt()
        val view = CursorView(this).apply { visibility = View.GONE }
        val lp = WindowManager.LayoutParams(
            size,
            size,
            WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS or
                WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
            } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
            }
        }
        runCatching { getSystemService(WindowManager::class.java).addView(view, lp) }
            .onSuccess {
                cursor = view
                params = lp
            }
    }

    fun pointer(x: Float, y: Float) {
        pointerX = x
        pointerY = y
        if (pressed && stroke.size < MAX_STROKE_POINTS) {
            stroke.add(floatArrayOf(x, y, (SystemClock.uptimeMillis() - pressedAt).toFloat()))
        }
        val view = cursor ?: return
        val lp = params ?: return
        lp.x = x.toInt()
        lp.y = y.toInt()
        view.visibility = View.VISIBLE
        runCatching { getSystemService(WindowManager::class.java).updateViewLayout(view, lp) }
        main.removeCallbacks(hide)
        main.postDelayed(hide, CURSOR_IDLE_MS)
    }

    fun hidePointer() {
        main.removeCallbacks(hide)
        cursor?.visibility = View.GONE
    }

    fun button(button: Int, down: Boolean, x: Float, y: Float) {
        when (button) {
            0 -> primary(down, x, y)
            1 -> if (down) performGlobalAction(GLOBAL_ACTION_BACK)
            2 -> if (down) performGlobalAction(GLOBAL_ACTION_HOME)
            3 -> if (down) performGlobalAction(GLOBAL_ACTION_BACK)
            4 -> if (down) performGlobalAction(GLOBAL_ACTION_RECENTS)
        }
    }

    private fun primary(down: Boolean, x: Float, y: Float) {
        if (down) {
            pressed = true
            pressedAt = SystemClock.uptimeMillis()
            stroke.clear()
            stroke.add(floatArrayOf(x, y, 0f))
            return
        }
        if (!pressed) return
        pressed = false
        val held = SystemClock.uptimeMillis() - pressedAt
        stroke.add(floatArrayOf(x, y, held.toFloat()))
        val start = stroke.first()
        val moved = stroke.maxOf { hypot(it[0] - start[0], it[1] - start[1]) }
        val path = Path().apply { moveTo(start[0], start[1]) }
        val duration: Long
        if (moved < TAP_SLOP_PX) {
            // Tap, or long press when the button was held.
            duration = if (held >= LONG_PRESS_MS) held.coerceAtMost(MAX_GESTURE_MS) else TAP_MS
        } else {
            for (point in sample(stroke)) path.lineTo(point[0], point[1])
            duration = held.coerceIn(MIN_DRAG_MS, MAX_GESTURE_MS)
        }
        dispatch(path, duration)
        stroke.clear()
    }

    private fun sample(points: List<FloatArray>): List<FloatArray> {
        if (points.size <= 64) return points.drop(1)
        val step = points.size / 64.0
        return (1..64).map { points[min(points.size - 1, (it * step).toInt())] }
    }

    fun scroll(x: Float, y: Float, dx: Float, dy: Float) {
        scrollX += dx
        scrollY += dy
        scrollAtX = x
        scrollAtY = y
        if (!scrollPending) {
            scrollPending = true
            main.postDelayed({ flushScroll() }, SCROLL_COALESCE_MS)
        }
    }

    private fun flushScroll() {
        scrollPending = false
        if (gestureBusy) {
            scrollPending = true
            main.postDelayed({ flushScroll() }, SCROLL_COALESCE_MS)
            return
        }
        val metrics = Backend.screen()
        val w = metrics.width.toFloat()
        val h = metrics.height.toFloat()
        val limit = min(w, h) * 0.4f
        val dx = scrollX.coerceIn(-limit, limit)
        val dy = scrollY.coerceIn(-limit, limit)
        scrollX = 0f
        scrollY = 0f
        if (abs(dx) < 1f && abs(dy) < 1f) return
        val fromX = scrollAtX.coerceIn(1f, w - 2f)
        val fromY = scrollAtY.coerceIn(1f, h - 2f)
        // Revealing content below means the finger moves up.
        val toX = (fromX - dx).coerceIn(1f, w - 2f)
        val toY = (fromY - dy).coerceIn(1f, h - 2f)
        val path = Path().apply {
            moveTo(fromX, fromY)
            lineTo(toX, toY)
        }
        dispatch(path, SCROLL_SWIPE_MS)
    }

    private fun dispatch(path: Path, duration: Long) {
        val gesture = GestureDescription.Builder()
            .addStroke(GestureDescription.StrokeDescription(path, 0, max(1L, duration)))
            .build()
        gestureBusy = true
        val done = object : GestureResultCallback() {
            override fun onCompleted(gestureDescription: GestureDescription?) {
                gestureBusy = false
            }

            override fun onCancelled(gestureDescription: GestureDescription?) {
                gestureBusy = false
            }
        }
        if (!dispatchGesture(gesture, done, main)) gestureBusy = false
    }

    // ── Keyboard ────────────────────────────────────────────────────────

    fun key(usage: Int, down: Boolean, modifiers: Int, text: String?) {
        if (!down) return
        val page = usage ushr 16
        val id = usage and 0xffff
        if (page == 0x0C) {
            when (id) {
                0xE9 -> volume(AudioManager.ADJUST_RAISE)
                0xEA -> volume(AudioManager.ADJUST_LOWER)
                0xE2 -> volume(AudioManager.ADJUST_TOGGLE_MUTE)
            }
            return
        }
        val command = modifiers and (CTRL or META) != 0
        if (command) {
            shortcut(id)
            return
        }
        if (text != null) {
            insert(text)
            return
        }
        when (id) {
            0x28, 0x58 -> enter()
            0x2A -> delete(backward = true)
            0x4C -> delete(backward = false)
            0x29 -> performGlobalAction(GLOBAL_ACTION_BACK)
            0x50 -> moveCaret(-1)
            0x4F -> moveCaret(1)
            0x4A -> caretTo(start = true)
            0x4D -> caretTo(start = false)
            0x2B -> focusNext()
            0x80 -> volume(AudioManager.ADJUST_RAISE)
            0x81 -> volume(AudioManager.ADJUST_LOWER)
            0x7F -> volume(AudioManager.ADJUST_TOGGLE_MUTE)
        }
    }

    private fun volume(direction: Int) {
        getSystemService(AudioManager::class.java)
            ?.adjustStreamVolume(AudioManager.STREAM_MUSIC, direction, AudioManager.FLAG_SHOW_UI)
    }

    private fun shortcut(id: Int) {
        val node = focusedEditable()
        when (id) {
            0x04 -> node?.let { // A
                val length = editableText(it).length
                setSelection(it, 0, length)
            }
            0x06 -> node?.performAction(AccessibilityNodeInfo.ACTION_COPY) // C
            0x1B -> node?.performAction(AccessibilityNodeInfo.ACTION_CUT) // X
            0x19 -> node?.performAction(AccessibilityNodeInfo.ACTION_PASTE) // V
        }
    }

    private fun focusedEditable(): AccessibilityNodeInfo? =
        findFocus(AccessibilityNodeInfo.FOCUS_INPUT)?.takeIf { it.isEditable }

    private fun editableText(node: AccessibilityNodeInfo): String {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && node.isShowingHintText) return ""
        return node.text?.toString() ?: ""
    }

    private fun selection(node: AccessibilityNodeInfo, length: Int): Pair<Int, Int> {
        var start = node.textSelectionStart
        var end = node.textSelectionEnd
        if (start < 0 || end < 0) {
            start = length
            end = length
        }
        return Pair(min(start, end).coerceIn(0, length), max(start, end).coerceIn(0, length))
    }

    private fun setText(node: AccessibilityNodeInfo, text: String, caret: Int) {
        val args = Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
        }
        if (node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)) {
            setSelection(node, caret, caret)
        }
    }

    private fun setSelection(node: AccessibilityNodeInfo, start: Int, end: Int) {
        val args = Bundle().apply {
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, start)
            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, end)
        }
        node.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, args)
    }

    private fun insert(text: String) {
        val node = focusedEditable() ?: return
        val current = editableText(node)
        val (start, end) = selection(node, current.length)
        setText(node, current.substring(0, start) + text + current.substring(end), start + text.length)
    }

    private fun delete(backward: Boolean) {
        val node = focusedEditable() ?: return
        val current = editableText(node)
        var (start, end) = selection(node, current.length)
        if (start == end) {
            if (backward && start > 0) {
                start = current.offsetByCodePoints(start, -1)
            } else if (!backward && end < current.length) {
                end = current.offsetByCodePoints(end, 1)
            } else {
                return
            }
        }
        setText(node, current.substring(0, start) + current.substring(end), start)
    }

    private fun enter() {
        val node = focusedEditable() ?: return
        if (node.isMultiLine) {
            insert("\n")
            return
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            node.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)
        }
    }

    private fun moveCaret(delta: Int) {
        val node = focusedEditable() ?: return
        val current = editableText(node)
        val (start, end) = selection(node, current.length)
        val caret = if (start != end) {
            if (delta < 0) start else end
        } else if (delta < 0 && start > 0) {
            current.offsetByCodePoints(start, -1)
        } else if (delta > 0 && end < current.length) {
            current.offsetByCodePoints(end, 1)
        } else {
            start
        }
        setSelection(node, caret, caret)
    }

    private fun caretTo(start: Boolean) {
        val node = focusedEditable() ?: return
        val caret = if (start) 0 else editableText(node).length
        setSelection(node, caret, caret)
    }

    private fun focusNext() {
        val node = findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return
        node.focusSearch(View.FOCUS_FORWARD)?.performAction(AccessibilityNodeInfo.ACTION_FOCUS)
    }

    /** A classic arrow pointer, tip at the view's top-left corner. */
    private class CursorView(context: Context) : View(context) {
        private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = 0xFFFFFFFF.toInt()
            style = Paint.Style.FILL
        }
        private val outline = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = 0xFF000000.toInt()
            style = Paint.Style.STROKE
            strokeJoin = Paint.Join.ROUND
        }
        private val arrow = Path()

        override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
            val s = min(w, h) / 24f
            outline.strokeWidth = 1.5f * s
            arrow.reset()
            arrow.moveTo(1.5f * s, 1.5f * s)
            arrow.lineTo(1.5f * s, 19f * s)
            arrow.lineTo(6f * s, 14.5f * s)
            arrow.lineTo(9.5f * s, 22f * s)
            arrow.lineTo(12.5f * s, 20.5f * s)
            arrow.lineTo(9f * s, 13.5f * s)
            arrow.lineTo(15f * s, 13.5f * s)
            arrow.close()
        }

        override fun onDraw(canvas: Canvas) {
            canvas.drawPath(arrow, fill)
            canvas.drawPath(arrow, outline)
        }
    }

    companion object {
        @Volatile
        var instance: InputService? = null
            private set

        private const val CURSOR_DP = 24
        private const val CURSOR_IDLE_MS = 4000L
        private const val TAP_SLOP_PX = 12f
        private const val TAP_MS = 40L
        private const val LONG_PRESS_MS = 450L
        private const val MIN_DRAG_MS = 80L
        private const val MAX_GESTURE_MS = 8000L
        private const val MAX_STROKE_POINTS = 2000
        private const val SCROLL_COALESCE_MS = 60L
        private const val SCROLL_SWIPE_MS = 250L
        private const val CTRL = 0x11
        private const val META = 0x88
    }
}
