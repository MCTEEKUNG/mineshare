package com.mineshare.mobile

enum class PointerShape { ARROW, CIRCLE, CROSSHAIR }

data class PointerOptions(
    val size: Float = 1f,
    val color: Int = 0xff4da3ff.toInt(),
    val shape: PointerShape = PointerShape.ARROW,
    val outline: Boolean = true,
    val shadow: Boolean = true,
    val motionSmoothing: Float = 0.15f,
    val scrollSpeed: Float = 1f,
    val scrollSmoothing: Float = 0.5f,
    val dragSmoothing: Float = 0.35f,
) {
    fun normalized() = copy(
        size = finite(size, 1f, 0.75f, 3f),
        motionSmoothing = finite(motionSmoothing, 0.15f, 0f, 1f),
        scrollSpeed = finite(scrollSpeed, 1f, 0.25f, 3f),
        scrollSmoothing = finite(scrollSmoothing, 0.5f, 0f, 1f),
        dragSmoothing = finite(dragSmoothing, 0.35f, 0f, 1f),
    )
    val scrollDuration: Long get() = (40 + scrollSmoothing * 100).toLong()
    val dragDuration: Long get() = (8 + dragSmoothing * 28).toLong()
    private fun finite(value: Float, fallback: Float, min: Float, max: Float) =
        if (value.isFinite()) value.coerceIn(min, max) else fallback
}

data class PointerPoint(val x: Float, val y: Float)

/** Preserve corners and the final position between Android gesture callbacks. */
class DragMotion {
    private val points = java.util.ArrayDeque<PointerPoint>()
    val isEmpty: Boolean get() = points.isEmpty()
    fun add(point: PointerPoint) {
        if (points.peekLast() == point) return
        if (points.size >= 128) points.removeFirst()
        points.addLast(point)
    }
    fun drain(): List<PointerPoint> = points.toList().also { points.clear() }
    fun clear() { points.clear() }
}

/** Keep wheel input received while Android is dispatching an earlier gesture. */
class ScrollMotion {
    private var x = 0f
    private var y = 0f
    fun add(dx: Float, dy: Float, options: PointerOptions, width: Int, height: Int) {
        if (!dx.isFinite() || !dy.isFinite()) return
        x = (x - dx * 100 * options.scrollSpeed).coerceIn(-width * 2f, width * 2f)
        y = (y + dy * 100 * options.scrollSpeed).coerceIn(-height * 2f, height * 2f)
    }
    fun take(width: Int, height: Int): PointerPoint? {
        if (kotlin.math.abs(x) < 0.01f && kotlin.math.abs(y) < 0.01f) return null
        val step = PointerPoint(x.coerceIn(-width * 0.65f, width * 0.65f), y.coerceIn(-height * 0.65f, height * 0.65f))
        x -= step.x; y -= step.y
        return step
    }
    fun clear() { x = 0f; y = 0f }
}

fun smoothPointer(current: Float, target: Float, elapsedSeconds: Float, smoothing: Float): Float {
    if (smoothing <= 0f || kotlin.math.abs(target - current) < 0.2f) return target
    val alpha = 1f - kotlin.math.exp(-elapsedSeconds.coerceIn(0.001f, 0.05f) / (0.032f * smoothing))
    return current + (target - current) * alpha
}
