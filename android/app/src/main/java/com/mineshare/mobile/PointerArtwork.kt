package com.mineshare.mobile

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.view.View

class PointerArtwork(context: Context) : View(context) {
    var options = PointerOptions()
        set(value) {
            if (field == value) return
            val changedShape = field.shape != value.shape
            field = value
            if (changedShape) rebuildPath()
            invalidate()
        }
    var preview = false
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val path = Path()
    val scale: Float get() = resources.displayMetrics.density * options.size * 0.65f
    val artworkWidth: Int get() = kotlin.math.ceil(40 * scale).toInt()
    val artworkHeight: Int get() = kotlin.math.ceil(48 * scale).toInt()
    val hotspotX: Float get() = (if (options.shape == PointerShape.ARROW) 2f else 20f) * scale
    val hotspotY: Float get() = (if (options.shape == PointerShape.ARROW) 2f else 24f) * scale

    init { setLayerType(LAYER_TYPE_SOFTWARE, null); rebuildPath() }

    private fun rebuildPath() {
        path.rewind()
        path.apply {
            when (options.shape) {
                PointerShape.ARROW -> { moveTo(2f, 2f); lineTo(2f, 35f); lineTo(10f, 27f); lineTo(17f, 43f); lineTo(23f, 40f); lineTo(16f, 24f); lineTo(29f, 24f); close() }
                PointerShape.CIRCLE -> addCircle(20f, 24f, 10f, Path.Direction.CW)
                PointerShape.CROSSHAIR -> { moveTo(20f, 8f); lineTo(20f, 40f); moveTo(4f, 24f); lineTo(36f, 24f); addCircle(20f, 24f, 7f, Path.Direction.CW) }
            }
        }
    }

    override fun onDraw(canvas: Canvas) {
        canvas.save()
        val fit = if (preview) minOf(1f, width * 0.85f / artworkWidth, height * 0.85f / artworkHeight) else 1f
        if (preview) canvas.translate((width - artworkWidth * fit) / 2f, (height - artworkHeight * fit) / 2f)
        canvas.scale(scale * fit, scale * fit)
        paint.strokeJoin = Paint.Join.ROUND
        paint.strokeCap = Paint.Cap.ROUND
        if (options.shadow) paint.setShadowLayer(3f, 1f, 2f, 0x99000000.toInt()) else paint.clearShadowLayer()
        val cross = options.shape == PointerShape.CROSSHAIR
        if (options.outline) {
            paint.color = if (options.color == 0xff0a0c0f.toInt() || options.color == 0xff191816.toInt()) 0xfff5f1e8.toInt() else 0xff0a0c0f.toInt()
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = if (cross) 5f else 3f
            canvas.drawPath(path, paint)
        }
        paint.clearShadowLayer()
        paint.color = options.color
        paint.style = if (cross) Paint.Style.STROKE else Paint.Style.FILL
        paint.strokeWidth = 2f
        canvas.drawPath(path, paint)
        canvas.restore()
    }
}
