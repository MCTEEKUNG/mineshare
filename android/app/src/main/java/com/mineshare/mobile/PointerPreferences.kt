package com.mineshare.mobile

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow

object PointerPreferences {
    private const val BRAND_BLUE = 0xff4da3ff.toInt()
    // Gold was the old brand default; follow the brand to blue instead of keeping a removed swatch.
    private const val RETIRED_GOLD = 0xffd4af37.toInt()
    val state = MutableStateFlow(PointerOptions())
    private var preferences: android.content.SharedPreferences? = null

    fun initialize(context: Context) {
        if (preferences != null) return
        val saved = context.applicationContext.getSharedPreferences("pointer_options", Context.MODE_PRIVATE)
        preferences = saved
        state.value = PointerOptions(
            size = saved.getFloat("size", 1f), color = saved.getInt("color", BRAND_BLUE).let { if (it == RETIRED_GOLD) BRAND_BLUE else it },
            shape = runCatching { PointerShape.valueOf(saved.getString("shape", "ARROW")!!) }.getOrDefault(PointerShape.ARROW),
            outline = saved.getBoolean("outline", true), shadow = saved.getBoolean("shadow", true),
            motionSmoothing = saved.getFloat("motion", 0.15f), scrollSpeed = saved.getFloat("scroll_speed", 1f),
            scrollSmoothing = saved.getFloat("scroll_smoothing", 0.5f), dragSmoothing = saved.getFloat("drag_smoothing", 0.35f),
        ).normalized()
    }

    fun update(value: PointerOptions) {
        val options = value.normalized()
        state.value = options
        preferences?.edit()?.putFloat("size", options.size)?.putInt("color", options.color)
            ?.putString("shape", options.shape.name)?.putBoolean("outline", options.outline)?.putBoolean("shadow", options.shadow)
            ?.putFloat("motion", options.motionSmoothing)?.putFloat("scroll_speed", options.scrollSpeed)
            ?.putFloat("scroll_smoothing", options.scrollSmoothing)?.putFloat("drag_smoothing", options.dragSmoothing)?.apply()
        MineShareAccessibility.instance?.updatePointerOptions(options)
    }
}
