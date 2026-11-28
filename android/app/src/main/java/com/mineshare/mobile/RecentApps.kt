package com.mineshare.mobile

import android.content.Context
import android.content.Intent
import android.provider.Settings

/**
 * Most-recently-used apps, learned from which window came to the front (the accessibility service already receives those events,
 * so no usage-access permission is needed). Only package names are kept, on this phone, and only up to [LIMIT].
 */
object RecentApps {
    private const val PREFS = "recent_apps"
    private const val KEY = "packages"
    const val LIMIT = 16

    private var ignoredAt = 0L
    private var ignored = emptySet<String>()

    fun load(context: Context): List<String> =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, "").orEmpty().split('\n').filter { it.isNotBlank() }

    /** Moves [packageName] to the front. Returns false when it was ignored or already first (nothing written). */
    fun record(context: Context, packageName: String): Boolean {
        if (packageName.isBlank() || packageName in ignoredPackages(context)) return false
        val current = load(context)
        if (current.firstOrNull() == packageName) return false
        val next = (listOf(packageName) + current.filter { it != packageName }).take(LIMIT)
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(KEY, next.joinToString("\n")).apply()
        return true
    }

    /** System surfaces that are not "apps the user opened": MineShare itself, status bar, the keyboard and the home screen. */
    private fun ignoredPackages(context: Context): Set<String> {
        val now = android.os.SystemClock.uptimeMillis()
        if (now - ignoredAt < 60_000 && ignored.isNotEmpty()) return ignored
        val keyboard = Settings.Secure.getString(context.contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
            ?.substringBefore('/')
        // Settings ships a placeholder "FallbackHome" that Android may report as the home app; Settings itself is a real app.
        val home = context.packageManager.resolveActivity(Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_HOME), 0)
            ?.activityInfo?.takeUnless { it.name.endsWith("FallbackHome") }?.packageName
        ignored = setOfNotNull(context.packageName, "com.android.systemui", "android", keyboard, home)
        ignoredAt = now
        return ignored
    }
}
