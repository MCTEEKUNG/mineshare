package com.mineshare.mobile

import android.accessibilityservice.AccessibilityService
import android.content.pm.LauncherActivityInfo
import android.content.pm.LauncherApps
import android.graphics.Color
import android.graphics.PixelFormat
import android.graphics.Typeface
import android.graphics.drawable.Drawable
import android.widget.GridLayout
import android.widget.ImageView
import android.text.TextUtils
import android.os.Process
import android.view.Gravity
import android.view.WindowManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ListView
import android.widget.BaseAdapter
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import android.widget.EditText
import android.widget.FrameLayout
import android.view.WindowInsets
import android.view.WindowInsetsAnimation
import android.view.MotionEvent
import android.view.ViewTreeObserver
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.text.Editable
import android.text.TextWatcher
import android.view.KeyEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.os.Handler
import android.os.Looper
import kotlinx.coroutines.*

class AppSearchOverlay(private val service: AccessibilityService, private val changed: () -> Unit) {
    private companion object { const val RECENT_GRID = 8 }
    private val model = AppSearchModel()
    private val windows = service.getSystemService(WindowManager::class.java)
    private val launcher = service.getSystemService(LauncherApps::class.java)
    private var entries = emptyList<LauncherActivityInfo>()
    private var apps = emptyList<SearchApp>()
    private var root: FrameLayout? = null
    private var panel: LinearLayout? = null
    private var title: TextView? = null
    private var summary: TextView? = null
    private var touchEntry = false
    private var windowFontScale = 0f
    private var windowDensity = 0f
    private var queryView: EditText? = null
    private var list: ListView? = null
    private var recentsCard: LinearLayout? = null
    private var recentsGrid: GridLayout? = null
    private var recentsWanted = false
    private var renderedRecents = ""
    private var shownResults = emptyList<SearchApp>()
    private val iconStates = HashMap<String, Drawable.ConstantState>()
    private val consumed = mutableSetOf<Int>()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var catalogJob: Job? = null
    private var catalogGeneration = 0L
    private var catalogReady = false
    private var renderedResults: List<SearchApp>? = null
    private var renderedSelection = -1
    private val resultAdapter = ResultsAdapter()
    private val catalogChanges = object : LauncherApps.Callback() {
        override fun onPackageAdded(packageName: String, user: android.os.UserHandle) = invalidateCatalog()
        override fun onPackageRemoved(packageName: String, user: android.os.UserHandle) = invalidateCatalog()
        override fun onPackageChanged(packageName: String, user: android.os.UserHandle) = invalidateCatalog()
        override fun onPackagesAvailable(packageNames: Array<out String>, user: android.os.UserHandle, replacing: Boolean) = invalidateCatalog()
        override fun onPackagesUnavailable(packageNames: Array<out String>, user: android.os.UserHandle, replacing: Boolean) = invalidateCatalog()
    }
    init { launcher.registerCallback(catalogChanges, Handler(Looper.getMainLooper())) }
    private val density get() = service.resources.displayMetrics.density
    private fun dp(value: Int) = (value * density).toInt()
    /** True when the system can blur the screen behind this window (off in battery saver or by the user). */
    private val blurAvailable get() = windows.isCrossWindowBlurEnabled
    private val hairline get() = dp(1).coerceAtLeast(1)

    fun arm() { dismiss(); model.arm() }
    fun show() { arm(); touchEntry = true; text("") }
    fun dismiss() {
        model.clear()
        root?.let {
            service.getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(it.windowToken, 0)
            runCatching { windows.removeView(it) }
        }
        root = null; panel = null; title = null; summary = null; queryView = null; list = null; touchEntry = false
        recentsCard = null; recentsGrid = null; recentsWanted = false; renderedRecents = ""; shownResults = emptyList()
        renderedResults = null; renderedSelection = -1
    }
    fun close() { dismiss(); catalogJob?.cancel(); scope.cancel(); launcher.unregisterCallback(catalogChanges) }
    private fun invalidateCatalog() {
        catalogReady = false
        catalogGeneration++
        catalogJob?.cancel()
        catalogJob = null
        if (root != null) loadCatalog()
    }
    private fun loadCatalog() {
        if (catalogReady || catalogJob?.isActive == true) return
        val generation = catalogGeneration
        catalogJob = scope.launch {
            val loaded = withContext(Dispatchers.IO) {
                val entries = runCatching { launcher.getActivityList(null, Process.myUserHandle()) }.getOrDefault(emptyList())
                    .distinctBy { it.componentName }
                entries to entries.map { SearchApp(it.componentName.flattenToString(), it.label.toString(), it.componentName.packageName) }
            }
            if (generation != catalogGeneration) return@launch
            entries = loaded.first; apps = loaded.second; catalogReady = true; iconStates.clear(); renderedRecents = ""
            if (root != null) render()
        }
    }
    fun text(value: String): Boolean {
        if (!model.text(value)) return false
        if (root == null) open()
        render()
        return true
    }
    fun key(scan: Int, down: Boolean): Boolean {
        if (!down && consumed.remove(scan)) return true
        if (!model.visible || (scan != 14 && scan != 28 && scan != 96 && scan != 103 && scan != 108 && scan != 1)) return false
        if (down) consumed.add(scan)
        if (down) when (scan) {
            14 -> { model.backspace(); render() }
            28, 96 -> launchSelected()
            103 -> { model.move(-1, shownResults.size); render() }
            108 -> { model.move(1, shownResults.size); render() }
            1 -> dismiss()
        }
        return true
    }
    fun disarmOnPointer() { if (root == null) model.clear() }
    fun resize() {
        root?.let { view ->
            if (windowFontScale != service.resources.configuration.fontScale || windowDensity != density) {
                // Native views retain their old SP size across service config
                // changes. Recreate the window, keeping the search model/cache.
                touchEntry = view.rootWindowInsets?.isVisible(WindowInsets.Type.ime()) == true
                runCatching { windows.removeView(view) }
                root = null; panel = null; title = null; summary = null; queryView = null; list = null
                recentsCard = null; recentsGrid = null; renderedRecents = ""
                renderedResults = null; renderedSelection = -1
                open()
                render()
                return
            }
            val params = view.layoutParams as WindowManager.LayoutParams
            val insets = view.rootWindowInsets ?: windows.maximumWindowMetrics.windowInsets
            val size = size(insets)
            if (params.width != size.width || params.x != size.horizontalOffset) {
                params.width = size.width; params.x = size.horizontalOffset; windows.updateViewLayout(view, params)
            }
            placePanel(insets)
        }
    }
    private fun size(insets: WindowInsets): SearchPanelGeometry {
        val bounds = windows.maximumWindowMetrics.bounds
        val safe = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.displayCutout())
        return searchPanelGeometry(bounds.width(), bounds.height(), density, safe.top, safe.bottom, safe.left, safe.right,
            insets.getInsets(WindowInsets.Type.ime()).bottom)
    }
    private fun placePanel(insets: WindowInsets) {
        val size = size(insets)
        panel?.let { content ->
            val params = content.layoutParams as FrameLayout.LayoutParams
            if (params.height != size.height || params.topMargin != size.top) {
                params.height = size.height; params.topMargin = size.top; content.layoutParams = params
            }
        }
        title?.visibility = if (size.compact) View.GONE else View.VISIBLE
        summary?.visibility = if (size.compact) View.GONE else View.VISIBLE
        recentsCard?.visibility = if (size.compact || !recentsWanted) View.GONE else View.VISIBLE
    }
    private fun open() {
        windowFontScale = service.resources.configuration.fontScale
        windowDensity = density
        val column = LinearLayout(service).apply {
            orientation = LinearLayout.VERTICAL
            background = SearchGlass.pane(dp(28).toFloat(), hairline, blurAvailable, dp(300).toFloat())
            setPadding(dp(16), dp(12), dp(16), dp(8))
        }
        title = TextView(service).apply {
            text = "ค้นหาแอป"; textSize = 20f; setTextColor(Color.WHITE); setTypeface(null, Typeface.BOLD); setShadowLayer(5f, 0f, 1f, 0xaa000000.toInt())
            isAccessibilityHeading = true; setPadding(0, dp(4), 0, dp(8))
        }
        column.addView(title)
        queryView = EditText(service).apply {
            textSize = 18f; minHeight = dp(52); setTextColor(0xffffffff.toInt())
            background = SearchGlass.control(dp(26).toFloat(), hairline, false)
            setPadding(dp(16), dp(8), dp(16), dp(8))
            setCompoundDrawablesRelativeWithIntrinsicBounds(R.drawable.ic_search, 0, 0, 0); compoundDrawablePadding = dp(10)
            contentDescription = "ชื่อแอปที่ต้องการค้นหา"
            hint = "พิมพ์ชื่อแอป…"; setHintTextColor(0xffe2e8ef.toInt()); isSingleLine = true
            highlightColor = 0x664da3ff
            imeOptions = EditorInfo.IME_ACTION_SEARCH or EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_FLAG_NO_FULLSCREEN
            addTextChangedListener(object : TextWatcher {
                override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) = Unit
                override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) {
                    if (s.toString() != model.query) { model.replaceQuery(s.toString()); render() }
                }
                override fun afterTextChanged(s: Editable?) = Unit
            })
            setOnEditorActionListener { _, action, event ->
                if (action == EditorInfo.IME_ACTION_SEARCH || action == EditorInfo.IME_ACTION_DONE) {
                    launchSelected(); true
                } else if (event?.keyCode == KeyEvent.KEYCODE_ENTER) {
                    if (event.action == KeyEvent.ACTION_UP) launchSelected()
                    true
                } else false
            }
            setOnKeyListener { _, code, event ->
                val scan = when (code) {
                    KeyEvent.KEYCODE_ENTER -> 28
                    KeyEvent.KEYCODE_DPAD_UP -> 103
                    KeyEvent.KEYCODE_DPAD_DOWN -> 108
                    KeyEvent.KEYCODE_ESCAPE, KeyEvent.KEYCODE_BACK -> 1
                    else -> return@setOnKeyListener false
                }
                key(scan, event.action == KeyEvent.ACTION_DOWN)
            }
        }
        val inputRow = LinearLayout(service).apply { orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL }
        inputRow.addView(queryView, LinearLayout.LayoutParams(0, -2, 1f))
        inputRow.addView(Button(service).apply {
            text = "ปิด"; contentDescription = "ปิดการค้นหา"; isAllCaps = false; minimumHeight = dp(52); minimumWidth = dp(64)
            setTextColor(0xffe8edf3.toInt()); stateListAnimator = null
            background = SearchGlass.control(dp(26).toFloat(), hairline, false)
            setOnClickListener { dismiss() }
        }, LinearLayout.LayoutParams(dp(72), -2).apply { marginStart = dp(8) })
        column.addView(inputRow)
        summary = TextView(service).apply {
            textSize = 13f; setTextColor(0xfff0f4f8.toInt()); setShadowLayer(4f, 0f, 1f, 0xaa000000.toInt())
            setPadding(0, dp(8), 0, dp(8))
            accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
        }
        column.addView(summary)
        recentsGrid = GridLayout(service).apply { columnCount = 4 }
        recentsCard = LinearLayout(service).apply {
            orientation = LinearLayout.VERTICAL
            background = SearchGlass.control(dp(22).toFloat(), hairline, false)
            setPadding(dp(10), dp(10), dp(10), dp(6)); visibility = View.GONE
            addView(TextView(service).apply {
                text = "ใช้ล่าสุด"; textSize = 14f; setTextColor(0xfff0f4f8.toInt()); setTypeface(null, Typeface.BOLD)
                isAccessibilityHeading = true; setShadowLayer(4f, 0f, 1f, 0xaa000000.toInt()); setPadding(dp(6), 0, 0, dp(4))
            })
            addView(recentsGrid, LinearLayout.LayoutParams(-1, -2))
        }
        column.addView(recentsCard, LinearLayout.LayoutParams(-1, -2).apply { bottomMargin = dp(8) })
        list = ListView(service).apply { adapter = resultAdapter; divider = null }
        column.addView(list, LinearLayout.LayoutParams(-1, 0, 1f))
        val outer = FrameLayout(service)
        // Wait for this window to own focus; a timed request can run before the
        // input connection exists, especially after opening from another app.
        if (touchEntry) {
            val focus = object : ViewTreeObserver.OnWindowFocusChangeListener {
                override fun onWindowFocusChanged(hasFocus: Boolean) {
                    if (!hasFocus) return
                    outer.viewTreeObserver.removeOnWindowFocusChangeListener(this)
                    queryView?.post {
                        if (root === outer) outer.windowInsetsController?.show(WindowInsets.Type.ime())
                    }
                }
            }
            outer.viewTreeObserver.addOnWindowFocusChangeListener(focus)
        }
        val initial = size(windows.maximumWindowMetrics.windowInsets)
        outer.addView(column, FrameLayout.LayoutParams(-1, initial.height).apply { topMargin = initial.top })
        panel = column
        outer.setOnApplyWindowInsetsListener { _, insets -> placePanel(insets); insets }
        outer.setWindowInsetsAnimationCallback(object : WindowInsetsAnimation.Callback(DISPATCH_MODE_CONTINUE_ON_SUBTREE) {
            override fun onProgress(insets: WindowInsets, runningAnimations: MutableList<WindowInsetsAnimation>): WindowInsets {
                placePanel(insets); return insets
            }
        })
        outer.setOnTouchListener { _, event ->
            if (event.action == MotionEvent.ACTION_OUTSIDE || (event.action == MotionEvent.ACTION_DOWN &&
                    (event.x < column.left || event.x > column.right ||
                        event.y < column.top || event.y > column.bottom))) { dismiss(); true } else false
        }
        windows.addView(outer, WindowManager.LayoutParams(
            initial.width, WindowManager.LayoutParams.MATCH_PARENT,
            WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
            WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN or WindowManager.LayoutParams.FLAG_WATCH_OUTSIDE_TOUCH or
                WindowManager.LayoutParams.FLAG_DIM_BEHIND or
                (if (blurAvailable) WindowManager.LayoutParams.FLAG_BLUR_BEHIND else 0),
            PixelFormat.TRANSLUCENT
        ).apply { gravity = Gravity.TOP or Gravity.CENTER_HORIZONTAL; x = initial.horizontalOffset
            if (blurAvailable) { blurBehindRadius = dp(28); dimAmount = SearchGlass.DIM_WITH_BLUR } else dimAmount = SearchGlass.DIM_WITHOUT_BLUR
            title = "ค้นหาแอป MineShare"
            softInputMode = WindowManager.LayoutParams.SOFT_INPUT_ADJUST_NOTHING or WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_HIDDEN })
        root = outer
        placePanel(windows.maximumWindowMetrics.windowInsets)
        loadCatalog()
        queryView?.requestFocus()
        outer.requestApplyInsets()
        changed()
    }
    private fun render() {
        queryView?.let { if (it.text.toString() != model.query) { it.setText(model.query); it.setSelection(it.length()) } }
        val container = list ?: return
        model.setRecents(RecentApps.load(service))
        val results = model.results(apps)
        val grid = if (model.query.isBlank() && catalogReady) model.recentApps(apps, RECENT_GRID) else emptyList()
        val shown = if (grid.isEmpty()) results else grid.map { it.id }.toSet().let { used -> results.filter { it.id !in used } }
        if (shown != shownResults) shownResults = shown
        refreshRecents(grid)
        val resultStatus = if (!catalogReady) "กำลังโหลดแอป…" else if (results.isEmpty()) "ไม่พบแอป" else "${results.size} แอป"
        summary?.text = "$resultStatus · ↑↓ เลือก · Enter เปิด"
        // Keep result feedback on the focused field when the compact panel
        // hides the visual summary to leave room above the keyboard.
        queryView?.stateDescription = resultStatus
        queryView?.accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
        resultAdapter.results = shownResults
        resultAdapter.notifyDataSetChanged()
        if (renderedResults !== shownResults || renderedSelection != model.selected) container.setSelection(model.selected)
        renderedResults = shownResults; renderedSelection = model.selected
    }
    private fun appIcon(id: String, size: Int): Drawable? {
        val state = iconStates[id] ?: entries.firstOrNull { it.componentName.flattenToString() == id }
            ?.getBadgedIcon(0)?.constantState?.also { iconStates[id] = it }
        return state?.newDrawable(service.resources)?.mutate()?.apply { setBounds(0, 0, size, size) }
    }

    /** Rebuilds the "used recently" icon grid only when the set of apps changed. */
    private fun refreshRecents(items: List<SearchApp>) {
        val grid = recentsGrid ?: return
        recentsWanted = items.isNotEmpty()
        val key = items.joinToString("|") { it.id }
        if (key != renderedRecents) {
            renderedRecents = key
            grid.removeAllViews()
            for (app in items) grid.addView(LinearLayout(service).apply {
                orientation = LinearLayout.VERTICAL; gravity = Gravity.CENTER_HORIZONTAL
                setPadding(dp(2), dp(6), dp(2), dp(6)); isClickable = true; contentDescription = app.label
                addView(ImageView(service).apply { setImageDrawable(appIcon(app.id, dp(48))); importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO },
                    LinearLayout.LayoutParams(dp(48), dp(48)))
                addView(TextView(service).apply {
                    text = app.label; textSize = 12f; setTextColor(Color.WHITE); maxLines = 1; ellipsize = TextUtils.TruncateAt.END
                    gravity = Gravity.CENTER; setShadowLayer(3f, 0f, 1f, 0xaa000000.toInt()); setPadding(0, dp(6), 0, 0)
                    importantForAccessibility = View.IMPORTANT_FOR_ACCESSIBILITY_NO
                }, LinearLayout.LayoutParams(-1, -2))
                setOnClickListener { launch(app.id) }
            }, GridLayout.LayoutParams(GridLayout.spec(GridLayout.UNDEFINED), GridLayout.spec(GridLayout.UNDEFINED, 1f)).apply { width = 0 })
        }
        root?.rootWindowInsets?.let { placePanel(it) }
    }

    /** ListView recycles only the visible rows, including when showing all apps. */
    private inner class ResultsAdapter : BaseAdapter() {
        var results = emptyList<SearchApp>()
        private val selectedGlass get() = SearchGlass.control(dp(16).toFloat(), hairline, true, dp(4))
        private val normalGlass get() = SearchGlass.control(dp(16).toFloat(), hairline, false, dp(4))
        private val selectionDelegate = object : View.AccessibilityDelegate() {
            override fun onInitializeAccessibilityNodeInfo(host: View, info: AccessibilityNodeInfo) {
                super.onInitializeAccessibilityNodeInfo(host, info)
                info.isSelected = host.tag == results.getOrNull(model.selected)?.id
            }
        }
        override fun getCount() = results.size.coerceAtLeast(1)
        override fun getItem(position: Int): SearchApp? = results.getOrNull(position)
        override fun getItemId(position: Int) = position.toLong()
        override fun getViewTypeCount() = 2
        override fun getItemViewType(position: Int) = if (results.isEmpty()) 1 else 0
        override fun isEnabled(position: Int) = results.isNotEmpty()
        override fun getView(position: Int, convertView: View?, parent: ViewGroup): View {
            val app = results.getOrNull(position) ?: return ((convertView as? TextView) ?: TextView(service)).apply {
                text = if (catalogReady) "ไม่พบแอป" else "กำลังโหลดแอป…"; setTextColor(Color.WHITE)
            }
            val selected = position == model.selected
            return ((convertView as? Button) ?: Button(service).apply {
                isAllCaps = false; isFocusable = false; minHeight = dp(48); stateListAnimator = null
            }).apply {
                tag = app.id
                // AbsListView clears delegates when scrapping rows. Reattach
                // the shared delegate on every bind, including recycled views.
                accessibilityDelegate = selectionDelegate
                isSelected = selected
                text = if (selected) "› ${app.label}" else app.label
                setTextColor(Color.WHITE)
                background = if (selected) selectedGlass else normalGlass
                setCompoundDrawablesRelative(appIcon(app.id, dp(32)), null, null, null); compoundDrawablePadding = dp(12)
                gravity = Gravity.START or Gravity.CENTER_VERTICAL; textAlignment = View.TEXT_ALIGNMENT_VIEW_START
                setPadding(dp(18), dp(6), dp(18), dp(6))
                setOnClickListener { launch(app.id) }
            }
        }
    }
    private fun launchSelected() { if (model.visible) shownResults.getOrNull(model.selected)?.let { launch(it.id) } }
    private fun launch(id: String) {
        val entry = entries.firstOrNull { it.componentName.flattenToString() == id } ?: return
        runCatching { launcher.startMainActivity(entry.componentName, entry.user, null, null) }
            .onSuccess { android.util.Log.i("MineShareInput", "launcher activity opened"); dismiss(); changed() }
            .onFailure { BridgeState.error("เปิดแอปไม่ได้: ${it.message}") }
    }
}
