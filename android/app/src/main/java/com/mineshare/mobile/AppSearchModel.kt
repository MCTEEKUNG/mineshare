package com.mineshare.mobile

import java.text.Normalizer
import java.util.Locale

data class SearchApp(val id: String, val label: String, val packageName: String)

/** Only launcher entries are indexed; no contacts, messages or app contents. */
class AppSearchModel {
    private data class IndexedApp(val app: SearchApp, val names: List<String>, val sortKey: String)
    private var indexedApps: List<SearchApp>? = null
    private var index = emptyList<IndexedApp>()
    private var cachedQuery: String? = null
    private var cachedResults = emptyList<SearchApp>()
    var armed = false
        private set
    var visible = false
        private set
    var query = ""
        private set
    var selected = 0
        private set
    private var recents = emptyList<String>()

    fun arm() { clear(); armed = true }
    fun clear() { armed = false; visible = false; query = ""; selected = 0 }
    fun text(value: String): Boolean {
        if (!armed && !visible) return false
        replaceQuery(query + value)
        visible = true; armed = false; selected = 0
        return true
    }
    fun replaceQuery(value: String) { query = value.filter { !it.isISOControl() }.take(120); selected = 0 }
    fun backspace() {
        if (query.isNotEmpty()) query = query.substring(0, query.offsetByCodePoints(query.length, -1))
        selected = 0
    }
    fun move(delta: Int, count: Int) { selected = (selected + delta).coerceIn(0, (count - 1).coerceAtLeast(0)) }
    /** Package names, most recently used first. They only reorder the empty-query list; typed queries keep relevance order. */
    fun setRecents(value: List<String>) { if (value != recents) { recents = value; cachedQuery = null } }

    /** The first [limit] apps from [apps] that were used recently, in use order (never more than one entry per package). */
    fun recentApps(apps: List<SearchApp>, limit: Int): List<SearchApp> =
        recents.asSequence().mapNotNull { pkg -> apps.firstOrNull { it.packageName == pkg } }.distinct().take(limit).toList()

    fun results(apps: List<SearchApp>): List<SearchApp> {
        if (apps !== indexedApps) {
            index = apps.map { app ->
                IndexedApp(app, listOf(normalized(app.label)) + when (app.packageName) {
                    "jp.naver.line.android" -> listOf("line", "ไลน์", "ไลน", "ไลน่")
                    else -> emptyList()
                }, app.label.lowercase(Locale.ROOT))
            }.sortedBy { it.sortKey }
            indexedApps = apps
            cachedQuery = null
        }
        val needle = normalized(query)
        if (cachedQuery == needle) return cachedResults
        val buckets = Array(3) { ArrayList<SearchApp>() }
        for (entry in index) {
            val names = entry.names
            val rank = when {
                names.any { it == needle } -> 0
                names.any { it.startsWith(needle) } -> 1
                names.any { needle in it } -> 2
                else -> continue
            }
            buckets[rank].add(entry.app)
        }
        cachedQuery = needle
        cachedResults = buckets.flatMap { it }.let { found ->
            if (needle.isEmpty() && recents.isNotEmpty()) {
                val top = recentApps(found, Int.MAX_VALUE)
                top + found.filter { it !in top }
            } else found
        }
        return cachedResults
    }
    private fun normalized(value: String) = Normalizer.normalize(value.trim(), Normalizer.Form.NFKC).lowercase(Locale.ROOT)
}
