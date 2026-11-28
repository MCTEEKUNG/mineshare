package com.mineshare.mobile

import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.InsetDrawable
import android.graphics.drawable.LayerDrawable

/**
 * "Liquid glass" surfaces for the native app-search overlay: a frosted dark pane, a specular edge that is
 * brightest where light would hit (top) and a soft sheen. When the system can blur the screen behind the
 * window the pane is lighter so the blur shows through; otherwise it is nearly opaque so text never sits on
 * a sharp, busy background (worst-case body text contrast stays above WCAG AA either way).
 */
object SearchGlass {
    private const val ARGB_BLUE = 0xff4da3ff.toInt()

    /**
     * Fill alpha of the pane. The user wants the app behind to stay visible, so the pane is genuinely see-through. Samsung One UI
     * reports system blur off, so without it the screen behind is dimmed only 30% and the pane is 56% dark; text legibility then
     * comes from the denser pills ([control]) and light text. Worst case (pure white behind) the lit top edge is ~#616a74, where
     * the muted summary (#f0f4f8) keeps >= 4.9:1 and the title 5.5:1; the field hint sits on a dense pill. (0.84, 0.92 and 0.96 were judged too opaque or ghosted; see history R21.)
     */
    fun paneAlpha(blurred: Boolean) = if (blurred) 0.66f else 0.56f
    const val DIM_WITH_BLUR = 0.22f
    const val DIM_WITHOUT_BLUR = 0.30f

    private fun argb(color: Int, alpha: Float) = (color and 0x00ffffff) or ((alpha * 255).toInt().coerceIn(0, 255) shl 24)

    private fun shape(radius: Float, vararg colors: Int, orientation: GradientDrawable.Orientation = GradientDrawable.Orientation.TOP_BOTTOM) =
        GradientDrawable(orientation, colors).apply { cornerRadius = radius; shape = GradientDrawable.RECTANGLE }

    private fun glow(color: Int, alpha: Float, cx: Float, cy: Float, radiusPx: Float, cornerPx: Float) =
        GradientDrawable().apply {
            gradientType = GradientDrawable.RADIAL_GRADIENT
            colors = intArrayOf(argb(color, alpha), argb(color, 0f))
            setGradientCenter(cx, cy); gradientRadius = radiusPx; cornerRadius = cornerPx
        }

    /** Rim light, tinted base, two coloured light leaks (they stand in for refraction) and a sheen, stacked in one drawable. */
    fun pane(radiusPx: Float, strokePx: Int, blurred: Boolean, glowPx: Float): Drawable {
        val inner = radiusPx - strokePx
        val rim = shape(radiusPx, argb(0xffffffff.toInt(), 0.42f), argb(0xffffffff.toInt(), 0.06f))
        val base = shape(inner, argb(0xff0a0c0f.toInt(), paneAlpha(blurred)), argb(0xff10151c.toInt(), paneAlpha(blurred)))
        val leakBlue = glow(ARGB_BLUE, if (blurred) 0.20f else 0.10f, 0.12f, 0.0f, glowPx, inner)
        val leakViolet = glow(0xff7b6cff.toInt(), if (blurred) 0.14f else 0.08f, 0.92f, 1.0f, glowPx, inner)
        val sheen = shape(inner, argb(0xffffffff.toInt(), if (blurred) 0.11f else 0.08f), argb(0xffffffff.toInt(), 0.0f))
        return LayerDrawable(arrayOf(rim, base, leakBlue, leakViolet, sheen)).apply {
            for (i in 1..4) setLayerInset(i, strokePx, strokePx, strokePx, strokePx)
        }
    }

    /** A pill-shaped glass control (search field, close button, result row). Dark under-layer keeps its text readable over a visible background. */
    fun control(radiusPx: Float, strokePx: Int, selected: Boolean, insetPx: Int = 0): Drawable {
        val inner = radiusPx - strokePx
        val rim = shape(radiusPx, argb(0xffffffff.toInt(), if (selected) 0.65f else 0.30f), argb(0xffffffff.toInt(), if (selected) 0.25f else 0.07f))
        val dark = shape(inner, argb(0xff05070a.toInt(), if (selected) 0.30f else 0.42f), argb(0xff05070a.toInt(), if (selected) 0.40f else 0.52f))
        val wash = if (selected) shape(inner, argb(ARGB_BLUE, 0.52f), argb(ARGB_BLUE, 0.34f))
            else shape(inner, argb(0xffffffff.toInt(), 0.12f), argb(0xffffffff.toInt(), 0.03f))
        val layers = LayerDrawable(arrayOf(rim, dark, wash)).apply { for (i in 1..2) setLayerInset(i, strokePx, strokePx, strokePx, strokePx) }
        return if (insetPx > 0) InsetDrawable(layers, insetPx, insetPx / 2, insetPx, insetPx / 2) else layers
    }
}
