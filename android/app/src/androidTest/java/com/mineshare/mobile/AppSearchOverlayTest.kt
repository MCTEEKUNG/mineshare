package com.mineshare.mobile

import android.accessibilityservice.AccessibilityServiceInfo
import android.app.UiAutomation
import android.content.Intent
import android.graphics.Rect
import android.os.Build
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.view.KeyEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.After
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test

/** Exercises the real native overlay and its bound service, on an emulator only. */
class AppSearchOverlayTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext
    private lateinit var automation: UiAutomation
    private val originalSettings = linkedMapOf<Pair<String, String>, String>()

    private fun shell(command: String): String = ParcelFileDescriptor.AutoCloseInputStream(
        automation.executeShellCommand(command)).bufferedReader().use { it.readText().trim() }

    @Before fun prepare() {
        assumeTrue("Only emulator hardware is allowed", Build.HARDWARE == "ranchu" || Build.HARDWARE == "goldfish")
        automation = instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        automation.serviceInfo = automation.serviceInfo.apply {
            flags = flags or AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS
        }
        listOf("secure" to "enabled_accessibility_services", "secure" to "accessibility_enabled",
            "secure" to "default_input_method", "secure" to "show_ime_with_hard_keyboard",
            "system" to "font_scale", "system" to "accelerometer_rotation", "system" to "user_rotation").forEach {
            originalSettings[it] = shell("settings get ${it.first} ${it.second}")
        }
        val original = originalSettings.getValue("secure" to "enabled_accessibility_services")
        val component = "com.mineshare.mobile/com.mineshare.mobile.MineShareAccessibility"
        val services = original.takeUnless { it == "null" || it.isBlank() }?.split(':').orEmpty()
        val enabled = (services + component).distinct().joinToString(":")
        // executeShellCommand launches argv directly; shell quote characters
        // would become part of the stored component name.
        shell("settings put secure enabled_accessibility_services $enabled")
        shell("settings put secure accessibility_enabled 1")
        shell("settings put secure show_ime_with_hard_keyboard 1")
        shell("settings put system accelerometer_rotation 0")
        shell("settings put system user_rotation 0")
        shell("settings put system font_scale 1.0")
        context.startActivity(Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        await("MineShare Accessibility service did not bind") { MineShareAccessibility.instance != null }
    }

    @After fun restore() {
        if (!::automation.isInitialized) return
        // Close through the public key path so a focused overlay cannot cover
        // Settings while font, rotation, keyboard and service settings restore.
        runCatching { key(KeyEvent.KEYCODE_ESCAPE) }
        originalSettings.entries.reversed().forEach { (setting, value) ->
            runCatching {
                if (value == "null" || value.isBlank()) shell("settings delete ${setting.first} ${setting.second}")
                else shell("settings put ${setting.first} ${setting.second} $value")
            }
        }
    }

    @Test fun emptyFeedbackAndSelectionSurviveLargeFontLandscapeAboveKeyboard() {
        openSearch()
        replaceQuery("zzzxq_no_installed_app_91384")
        await("Empty result feedback missing from focused query") { query()?.stateDescription?.toString() == "ไม่พบแอป" }
        val label = settingsLabel()
        replaceQuery(label)
        await("Settings result not selected") { selectedSettings(label) != null }
        shell("settings put system font_scale 2.0")
        shell("settings put system user_rotation 1")
        await("Query was lost during font/rotation change") { query()?.text?.toString() == label &&
            context.resources.configuration.fontScale >= 1.9f &&
            context.resources.configuration.orientation == android.content.res.Configuration.ORIENTATION_LANDSCAPE }
        await("Selected result or IME missing after recreation") { selectedSettings(label) != null && imeBounds() != null }
        val selected = checkNotNull(selectedSettings(label))
        val rowBounds = Rect().also { selected.getBoundsInScreen(it) }
        val keyboardBounds = checkNotNull(imeBounds())
        assertTrue("Settings row must remain visible", selected.isVisibleToUser && rowBounds.height() > 0)
        assertTrue("Settings row overlaps keyboard: $rowBounds / $keyboardBounds", rowBounds.bottom <= keyboardBounds.top)
        assertTrue("Result count missing from query semantics", query()?.stateDescription?.toString()?.contains("แอป") == true)
        key(KeyEvent.KEYCODE_ESCAPE)
        await("Escape did not close overlay") { query() == null }
    }

    @Test fun enterLaunchesSelectedSettingsExactlyOnce() {
        openSearch()
        val label = settingsLabel()
        replaceQuery(label)
        await("Settings result not selected") { selectedSettings(label) != null }
        val launchesBefore = launchCount()
        key(KeyEvent.KEYCODE_ENTER)
        await("Enter did not open system Settings") {
            query() == null && roots().any { it.packageName?.toString() == "com.android.settings" }
        }
        await("Successful launch event missing") { launchCount() > launchesBefore }
        assertEquals("One Enter down/up pair must launch only once", launchesBefore + 1, launchCount())
    }

    @Test fun emptyFieldShowsRecentlyUsedAppsAndTappingOneLaunchesIt() {
        val pm = context.packageManager
        val launchable = pm.queryIntentActivities(Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER), 0)
            .map { it.activityInfo.packageName }.distinct().filter { it != context.packageName }
        context.getSharedPreferences("recent_apps", android.content.Context.MODE_PRIVATE).edit().clear().commit()
        // Seed in order; the package recorded last (Settings) becomes the most recent.
        val seeded = launchable.filter { it != "com.android.settings" }.take(5).plus("com.android.settings")
            .filter { RecentApps.record(context, it) || RecentApps.load(context).contains(it) }
        assertTrue("Need some launchable apps on the emulator", seeded.size >= 3)
        val expectedLabels = RecentApps.load(context).take(8).mapNotNull { pkg ->
            runCatching { pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString() }.getOrNull() }
        openSearch()
        // Recently used apps appear as tappable icons above the list; each cell is described by its app name.
        await("Recent icons missing") { expectedLabels.take(3).all { label -> recentCells().any { it.contentDescription?.toString() == label } } }
        assertTrue("The field must still be empty while the recents are shown", query()?.isShowingHintText == true)
        InstrumentationRegistry.getArguments().getString("hold")?.toLongOrNull()?.let { SystemClock.sleep(it) }
        val launchesBefore = launchCount()
        val first = checkNotNull(recentCells().firstOrNull { it.contentDescription?.toString() == expectedLabels.first() })
        assertTrue(first.performAction(AccessibilityNodeInfo.ACTION_CLICK))
        await("Overlay stayed open after tapping a recent app (launches before=$launchesBefore now=${launchCount()})") { query() == null }
        await("Tapping a recent app did not report a launch (before=$launchesBefore now=${launchCount()})") { launchCount() > launchesBefore }
    }

    /** Tappable icon cells of the overlay only: another app on screen can also have a clickable "Settings". */
    private fun recentCells() = nodes().filter {
        it.packageName?.toString() == context.packageName && it.className?.toString() == "android.widget.LinearLayout" && it.isClickable
    }

    private fun settingsLabel() = context.packageManager.getApplicationLabel(
        context.packageManager.getApplicationInfo("com.android.settings", 0)).toString()

    private fun openSearch() {
        instrumentation.runOnMainSync { checkNotNull(MineShareAccessibility.instance).openAppSearch() }
        await("Search edit field not visible") { query() != null }
        await("Catalog never left loading state") { query()?.stateDescription?.toString()?.let { it != "กำลังโหลดแอป…" } == true }
    }

    private fun replaceQuery(value: String) {
        val field = checkNotNull(query())
        val arguments = Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, value) }
        assertTrue("Native query must support ACTION_SET_TEXT", field.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments))
        await("Query update not reflected") { query()?.text?.toString() == value }
    }

    private fun roots() = automation.windows.mapNotNull { it.root }
    private fun nodes(): List<AccessibilityNodeInfo> {
        val result = ArrayList<AccessibilityNodeInfo>()
        fun visit(node: AccessibilityNodeInfo) {
            result.add(node)
            for (index in 0 until node.childCount) node.getChild(index)?.let { visit(it) }
        }
        roots().forEach { visit(it) }
        return result
    }
    private fun query() = nodes().firstOrNull { it.className?.toString() == "android.widget.EditText" &&
        it.contentDescription?.toString() == "ชื่อแอปที่ต้องการค้นหา" }
    private fun selectedSettings(label: String) = nodes().firstOrNull {
        it.className?.toString() == "android.widget.Button" && it.text?.toString() == "› $label" && it.isSelected
    }
    private fun imeBounds(): Rect? = automation.windows.firstOrNull {
        it.type == AccessibilityWindowInfo.TYPE_INPUT_METHOD
    }?.let { window -> Rect().also { window.getBoundsInScreen(it) }.takeUnless { it.isEmpty } }
    private fun launchCount() = shell("logcat -d -s MineShareInput:I").lineSequence().count { "launcher activity opened" in it }

    private fun key(code: Int) {
        val now = SystemClock.uptimeMillis()
        assertTrue(automation.injectInputEvent(KeyEvent(now, now, KeyEvent.ACTION_DOWN, code, 0), true))
        assertTrue(automation.injectInputEvent(KeyEvent(now, now, KeyEvent.ACTION_UP, code, 0), true))
    }
    private fun await(message: String, condition: () -> Boolean) {
        val deadline = SystemClock.uptimeMillis() + 5_000
        do {
            if (condition()) return
            SystemClock.sleep(50)
        } while (SystemClock.uptimeMillis() < deadline)
        fail("$message; native nodes=" + nodes().filter { it.packageName?.toString() == context.packageName &&
            (it.text != null || it.contentDescription != null) }
            .joinToString(" | ") { "${it.className}: ${it.text} / ${it.contentDescription} selected=${it.isSelected} click=${it.isClickable} state=${it.stateDescription}" })
    }
}
