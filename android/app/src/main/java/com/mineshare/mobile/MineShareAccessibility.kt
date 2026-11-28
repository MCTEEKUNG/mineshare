package com.mineshare.mobile

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.res.Configuration
import android.graphics.Path
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.Choreographer
import android.view.KeyEvent
import android.view.WindowManager
import android.view.accessibility.AccessibilityEvent
import org.json.JSONObject

/** Explicitly enabled by the phone owner; only authenticated native sessions reach accept(). */
class MineShareAccessibility : AccessibilityService() {
    companion object { @Volatile var instance: MineShareAccessibility? = null }
    private val handler = Handler(Looper.getMainLooper())
    private lateinit var windows: WindowManager
    private var cursor: PointerArtwork? = null
    private var options = PointerOptions()
    private var visualX = 0f
    private var visualY = 0f
    private var framePending = false
    private var framePostedAt = 0L
    private var lastFrame = 0L
    private val scrollMotion = ScrollMotion()
    private val dragPoints = DragMotion()
    private var gestureGeneration = 0L
    private val cursorFrame = object : Choreographer.FrameCallback {
        override fun doFrame(frameTimeNanos: Long) {
            framePending = false
            if (!controlled) return
            val elapsed = if (lastFrame == 0L) 1f / 60 else (frameTimeNanos - lastFrame) / 1_000_000_000f
            lastFrame = frameTimeNanos
            visualX = smoothPointer(visualX, x, elapsed, options.motionSmoothing)
            visualY = smoothPointer(visualY, y, elapsed, options.motionSmoothing)
            renderCursor()
            if (visualX != x || visualY != y) showCursor()
        }
    }
    private var width = 1
    private var height = 1
    private var x = 0f
    private var y = 0f
    private var controlled = false
    private var down = false
    private var stroke: GestureDescription.StrokeDescription? = null
    private var dispatching = false
    private var sentX = 0f
    private var sentY = 0f
    private var dirty = false
    private var meta = 0
    private val heldKeys = mutableSetOf<Int>()
    private val shortcuts = PhoneShortcuts()
    private var shortcutGeneration = 0L
    private val peerSides = mutableMapOf<String, String>()
    private var returnEdge = "left"
    private lateinit var search: AppSearchOverlay
    private val cursorHealth = object : Runnable {
        override fun run() {
            if (controlled) {
                val now = android.os.SystemClock.uptimeMillis()
                if (framePending && now - framePostedAt > 250) {
                    android.util.Log.w("MineShareInput", "cursor frame request stalled ${now - framePostedAt} ms; reposting")
                    Choreographer.getInstance().removeFrameCallback(cursorFrame)
                    framePending = false
                }
                // Attach directly: the window must not depend on a frame callback arriving.
                if (cursor?.isAttachedToWindow != true) {
                    cursor?.let { runCatching { windows.removeViewImmediate(it) } }
                    cursor = null
                    renderCursor()
                }
                showCursor()
            }
            handler.postDelayed(this, 500)
        }
    }

    override fun onServiceConnected() {
        instance = this
        windows = getSystemService(WindowManager::class.java)
        search = AppSearchOverlay(this, ::rebuildCursor)
        PointerPreferences.initialize(this)
        options = PointerPreferences.state.value
        peerSides.putAll(BridgeState.peerSides)
        refreshBounds()
        handler.post(cursorHealth)
        BridgeState.state.value.peers.firstOrNull { it.inputActive && it.connected }?.let {
            accept(JSONObject().put("kind", "take_control").put("device_id", it.id))
        }
        BridgeState.state.value = BridgeState.state.value.copy(accessibility = true)
    }

    fun updatePointerOptions(value: PointerOptions) = handler.post {
        options = value.normalized()
        cursor?.options = options
        showCursor()
    }

    fun openAppSearch() = handler.post { search.show() }

    fun refreshBounds() = handler.post {
        val bounds = windows.maximumWindowMetrics.bounds
        val changed = width != bounds.width() || height != bounds.height()
        if (controlled && changed) endContact()
        val horizontal = if (width > 1) x / (width - 1) else 0.5f
        val vertical = if (height > 1) y / (height - 1) else 0.5f
        width = bounds.width()
        height = bounds.height()
        x = (horizontal * (width - 1)).coerceIn(0f, (width - 1).toFloat())
        y = (vertical * (height - 1)).coerceIn(0f, (height - 1).toFloat())
        visualX = x; visualY = y
        if (::search.isInitialized) search.resize()
        showCursor()
        BridgeState.command("display") { put("width", width).put("height", height) }
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        refreshBounds()
    }

    fun accept(event: JSONObject) = handler.post {
        when (event.optString("kind")) {
            "peer_side" -> peerSides[event.optString("device_id")] = event.optString("side", event.optString("peer_side", "left")).lowercase()
            "take_control" -> {
                android.util.Log.i("MineShareInput", "take_control peer=${event.optString("device_id")}")
                if (controlled) { endContact(); releaseKeys(); search.dismiss(); shortcuts.reset() }
                controlled = true
                returnEdge = peerSides[event.optString("device_id")] ?: "left"
                val entryX = when (returnEdge) { "left" -> 12.0; "right" -> width - 13.0; else -> width / 2.0 }
                val entryY = when (returnEdge) { "top" -> 12.0; "bottom" -> height - 13.0; else -> height / 2.0 }
                x = event.optDouble("x", entryX).toFloat().coerceIn(0f, width - 1f)
                y = event.optDouble("y", entryY).toFloat().coerceIn(0f, height - 1f)
                visualX = x; visualY = y; lastFrame = 0L
                renderCursor()
                showCursor()
            }
            "release" -> { android.util.Log.i("MineShareInput", "release peer=${event.optString("device_id")}"); release() }
            "move" -> if (controlled) {
                val nextX = x + event.optDouble("dx").toFloat()
                val nextY = y + event.optDouble("dy").toFloat()
                x = nextX.coerceIn(0f, width - 1f)
                y = nextY.coerceIn(0f, height - 1f)
                showCursor()
                if (down) {
                    dragPoints.add(PointerPoint(x, y))
                    dirty = true
                    continueDrag()
                }
                val crossing = when (returnEdge) {
                    "left" -> nextX < -8
                    "right" -> nextX > width + 8
                    "top" -> nextY < -8
                    "bottom" -> nextY > height + 8
                    else -> false
                }
                if (!down && crossing) {
                    android.util.Log.i("MineShareInput", "return_edge=$returnEdge position=$x,$y")
                    BridgeState.command("release_input")
                    release()
                }
            }
            "button" -> if (controlled) button(event.optString("button"), event.optBoolean("down"))
            "scroll" -> if (controlled && !down) {
                val dx = event.optDouble("dx").toFloat()
                val dy = event.optDouble("dy").toFloat()
                scrollMotion.add(dx, dy, options, width, height)
                continueScroll()
            }
            "text" -> if (controlled && !search.text(event.optString("text"))) inputMethod?.currentInputConnection?.commitText(event.optString("text"), 1, null)
            "key" -> if (controlled) key(event.optInt("code"), event.optBoolean("down"))
        }
    }

    private fun button(button: String, pressed: Boolean) {
        if (pressed) search.disarmOnPointer()
        when (button.lowercase()) {
            "right" -> if (!pressed) performGlobalAction(GLOBAL_ACTION_BACK)
            "middle" -> if (!pressed) performGlobalAction(GLOBAL_ACTION_HOME)
            "x1" -> if (!pressed) performGlobalAction(GLOBAL_ACTION_RECENTS)
            "left" -> {
                if (down == pressed) return
                down = pressed
                visualX = x; visualY = y
                showCursor()
                if (pressed) { dragPoints.clear(); dragPoints.add(PointerPoint(x, y)) }
                dirty = true
                continueDrag()
            }
        }
    }

    private fun continueDrag() {
        if (dispatching || !dirty) return
        dirty = false
        val previous = stroke
        if (previous == null && !down && dragPoints.isEmpty) { continueScroll(); return }
        val points = dragPoints.drain()
        val path = Path().apply {
            val start = if (previous == null) points.firstOrNull() ?: PointerPoint(x, y) else PointerPoint(sentX, sentY)
            moveTo(start.x, start.y)
            for (point in points) lineTo(point.x, point.y)
            lineTo(x, y)
        }
        val next = previous?.continueStroke(path, 0, options.dragDuration, down)
            ?: GestureDescription.StrokeDescription(path, 0, options.dragDuration, down)
        sentX = x
        sentY = y
        stroke = if (down) next else null
        dispatch(next)
    }

    private fun continueScroll() {
        if (dispatching || down || stroke != null || !controlled || width < 16 || height < 16) return
        val delta = scrollMotion.take(width - 8, height - 8) ?: return
        val startX = if (delta.x >= 0) x.coerceIn(4f, width - 5f - delta.x) else x.coerceIn(4f - delta.x, width - 5f)
        val startY = if (delta.y >= 0) y.coerceIn(4f, height - 5f - delta.y) else y.coerceIn(4f - delta.y, height - 5f)
        val path = Path().apply { moveTo(startX, startY); lineTo(startX + delta.x, startY + delta.y) }
        dispatch(GestureDescription.StrokeDescription(path, 0, options.scrollDuration))
    }

    private fun dispatch(next: GestureDescription.StrokeDescription) {
        dispatching = true
        val generation = gestureGeneration
        val accepted = dispatchGesture(GestureDescription.Builder().addStroke(next).build(),
            object : GestureResultCallback() {
                override fun onCompleted(gesture: GestureDescription?) {
                    if (generation != gestureGeneration) return
                    dispatching = false
                    if (down || stroke != null || dirty) continueDrag()
                    continueScroll()
                }
                override fun onCancelled(gesture: GestureDescription?) {
                    if (generation != gestureGeneration) return
                    dispatching = false
                    stroke = null
                    down = false
                    dirty = false
                    dragPoints.clear()
                    scrollMotion.clear()
                }
            }, handler)
        if (!accepted) { dispatching = false; stroke = null; down = false; dirty = false; dragPoints.clear(); scrollMotion.clear() }
    }

    private fun key(scan: Int, pressed: Boolean) {
        if (search.key(scan, pressed)) return
        val shortcut = shortcuts.key(scan, pressed)
        if (shortcut.consumed) {
            shortcut.action?.let { action ->
                val generation = ++shortcutGeneration
                releaseKeys()
                if (action != PhoneAction.RECENTS) search.dismiss()
                when (action) {
                    PhoneAction.RECENTS -> { search.arm(); performGlobalAction(GLOBAL_ACTION_RECENTS) }
                    PhoneAction.HOME -> performGlobalAction(GLOBAL_ACTION_HOME)
                    PhoneAction.BACK -> performGlobalAction(GLOBAL_ACTION_BACK)
                    PhoneAction.ROTATE -> PhoneRotation.apply(this)
                    PhoneAction.AUTO_ROTATE -> PhoneRotation.apply(this, automatic = true)
                    PhoneAction.RECOVER_POINTER -> { endContact(); x = width / 2f; y = height / 2f; visualX = x; visualY = y; rebuildCursor() }
                    PhoneAction.SWITCH_APP -> {
                        if (performGlobalAction(GLOBAL_ACTION_RECENTS)) handler.postDelayed({
                            if (controlled && shortcutGeneration == generation) performGlobalAction(GLOBAL_ACTION_RECENTS)
                        }, 180)
                    }
                }
            }
            return
        }
        val code = when (scan) {
            1 -> KeyEvent.KEYCODE_ESCAPE
            14 -> KeyEvent.KEYCODE_DEL
            15 -> KeyEvent.KEYCODE_TAB
            28, 96 -> KeyEvent.KEYCODE_ENTER
            29, 97 -> KeyEvent.KEYCODE_CTRL_LEFT
            42, 54 -> KeyEvent.KEYCODE_SHIFT_LEFT
            56, 100 -> KeyEvent.KEYCODE_ALT_LEFT
            102 -> KeyEvent.KEYCODE_MOVE_HOME
            103 -> KeyEvent.KEYCODE_DPAD_UP
            104 -> KeyEvent.KEYCODE_PAGE_UP
            105 -> KeyEvent.KEYCODE_DPAD_LEFT
            106 -> KeyEvent.KEYCODE_DPAD_RIGHT
            107 -> KeyEvent.KEYCODE_MOVE_END
            108 -> KeyEvent.KEYCODE_DPAD_DOWN
            109 -> KeyEvent.KEYCODE_PAGE_DOWN
            111 -> KeyEvent.KEYCODE_FORWARD_DEL
            125, 126 -> KeyEvent.KEYCODE_META_LEFT
            else -> {
                if (meta and (KeyEvent.META_CTRL_ON or KeyEvent.META_ALT_ON or KeyEvent.META_META_ON) == 0) return
                when (scan) {
                    30 -> KeyEvent.KEYCODE_A
                    48 -> KeyEvent.KEYCODE_B
                    46 -> KeyEvent.KEYCODE_C
                    32 -> KeyEvent.KEYCODE_D
                    18 -> KeyEvent.KEYCODE_E
                    33 -> KeyEvent.KEYCODE_F
                    34 -> KeyEvent.KEYCODE_G
                    35 -> KeyEvent.KEYCODE_H
                    23 -> KeyEvent.KEYCODE_I
                    36 -> KeyEvent.KEYCODE_J
                    37 -> KeyEvent.KEYCODE_K
                    38 -> KeyEvent.KEYCODE_L
                    50 -> KeyEvent.KEYCODE_M
                    49 -> KeyEvent.KEYCODE_N
                    24 -> KeyEvent.KEYCODE_O
                    25 -> KeyEvent.KEYCODE_P
                    16 -> KeyEvent.KEYCODE_Q
                    19 -> KeyEvent.KEYCODE_R
                    31 -> KeyEvent.KEYCODE_S
                    20 -> KeyEvent.KEYCODE_T
                    22 -> KeyEvent.KEYCODE_U
                    47 -> KeyEvent.KEYCODE_V
                    17 -> KeyEvent.KEYCODE_W
                    45 -> KeyEvent.KEYCODE_X
                    21 -> KeyEvent.KEYCODE_Y
                    44 -> KeyEvent.KEYCODE_Z
                    else -> return
                }
            }
        }
        val flag = when (code) {
            KeyEvent.KEYCODE_CTRL_LEFT -> KeyEvent.META_CTRL_ON
            KeyEvent.KEYCODE_SHIFT_LEFT -> KeyEvent.META_SHIFT_ON
            KeyEvent.KEYCODE_ALT_LEFT -> KeyEvent.META_ALT_ON
            KeyEvent.KEYCODE_META_LEFT -> KeyEvent.META_META_ON
            else -> 0
        }
        meta = if (pressed) meta or flag else meta and flag.inv()
        if (pressed) heldKeys.add(code) else heldKeys.remove(code)
        val now = android.os.SystemClock.uptimeMillis()
        inputMethod?.currentInputConnection?.sendKeyEvent(KeyEvent(now, now,
            if (pressed) KeyEvent.ACTION_DOWN else KeyEvent.ACTION_UP, code, 0, meta))
    }

    // The overlay uses physical display coordinates regardless of text direction.
    @android.annotation.SuppressLint("RtlHardcoded")
    private fun showCursor() {
        if (!controlled) return
        if (!framePending) { framePending = true; framePostedAt = android.os.SystemClock.uptimeMillis(); Choreographer.getInstance().postFrameCallback(cursorFrame) }
    }

    @android.annotation.SuppressLint("RtlHardcoded")
    private fun rebuildCursor() {
        cursor?.let { runCatching { windows.removeViewImmediate(it) } }
        cursor = null
        showCursor()
    }

    @android.annotation.SuppressLint("RtlHardcoded")
    private fun renderCursor() {
      try {
        var view = cursor
        if (view == null) {
            view = PointerArtwork(this).also { it.options = options }
            cursor = view
            android.util.Log.i("MineShareInput", "cursor window add at ${visualX.toInt()},${visualY.toInt()}")
            windows.addView(view, WindowManager.LayoutParams(view.artworkWidth, view.artworkHeight,
                WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
                WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                    WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS,
                android.graphics.PixelFormat.TRANSLUCENT).apply { gravity = Gravity.TOP or Gravity.LEFT })
        }
        val params = view.layoutParams as WindowManager.LayoutParams
        val nextX = (visualX - view.hotspotX).toInt()
        val nextY = (visualY - view.hotspotY).toInt()
        if (params.width == view.artworkWidth && params.height == view.artworkHeight && params.x == nextX && params.y == nextY) return
        params.width = view.artworkWidth
        params.height = view.artworkHeight
        params.x = nextX
        params.y = nextY
        windows.updateViewLayout(view, params)
    } catch (error: RuntimeException) {
        android.util.Log.w("MineShareInput", "cursor window needs reattachment", error)
        cursor?.let { runCatching { windows.removeViewImmediate(it) } }
        cursor = null
        // The health check retries without giving up keyboard/mouse ownership.
      }
    }

    fun release() {
        // Do not enqueue a second cleanup behind a newer take_control event.
        if (Looper.myLooper() == handler.looper) releaseNow() else handler.post { releaseNow() }
    }

    private fun endContact() {
        gestureGeneration++
        dispatching = false; down = false; dirty = false
        scrollMotion.clear(); dragPoints.clear()
        stroke?.let {
            val path = Path().apply { moveTo(sentX, sentY) }
            runCatching { dispatchGesture(GestureDescription.Builder().addStroke(it.continueStroke(path, 0, 1, false)).build(), null, handler) }
        }
        stroke = null
    }

    private fun releaseNow() {
        controlled = false
        endContact()
        if (::search.isInitialized) search.dismiss()
        Choreographer.getInstance().removeFrameCallback(cursorFrame)
        framePending = false
        shortcutGeneration++
        shortcuts.reset()
        releaseKeys()
        cursor?.let { runCatching { windows.removeView(it) } }
        cursor = null
    }

    private fun releaseKeys() {
        val now = android.os.SystemClock.uptimeMillis()
        heldKeys.forEach { code -> inputMethod?.currentInputConnection?.sendKeyEvent(KeyEvent(now, now, KeyEvent.ACTION_UP, code, 0)) }
        heldKeys.clear()
        meta = 0
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event?.eventType != AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED) return
        if (controlled) showCursor()
        // Learn which apps were used lately for the search overlay's "used recently" grid (package names only, kept on this phone).
        event.packageName?.toString()?.let { RecentApps.record(this, it) }
    }
    override fun onKeyEvent(event: KeyEvent): Boolean = BridgeState.service?.handleVolumeKey(event) == true
    override fun onInterrupt() { BridgeState.command("release_input"); release() }
    override fun onDestroy() {
        handler.removeCallbacks(cursorHealth)
        BridgeState.command("release_input")
        release()
        if (::search.isInitialized) search.close()
        instance = null
        BridgeState.state.value = BridgeState.state.value.copy(accessibility = false)
        super.onDestroy()
    }

}
