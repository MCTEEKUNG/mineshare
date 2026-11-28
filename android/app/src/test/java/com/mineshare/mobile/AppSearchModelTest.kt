package com.mineshare.mobile

import org.junit.Assert.*
import org.junit.Test

class AppSearchModelTest {
    @Test fun cachedResultsAreReusedDuringNavigationAndInvalidatedByNewCatalog() {
        val search = AppSearchModel()
        search.arm(); search.text("LINE")
        val first = search.results(apps)
        search.move(1, first.size)
        assertSame(first, search.results(apps))
        search.replaceQuery("line")
        assertSame(first, search.results(apps))
        val changed = apps.filter { it.id != "b" }
        assertEquals(listOf("a"), search.results(changed).map { it.id })
        search.replaceQuery("settings")
        assertEquals(listOf("c"), search.results(changed).map { it.id })
    }
    private val apps = listOf(SearchApp("a", "LINE Camera", "camera"), SearchApp("b", "LINE", "jp.naver.line.android"), SearchApp("c", "Settings", "settings"))

    @Test fun typingOnlyStartsSearchAfterWindowsAndExactAppWins() {
        val search = AppSearchModel()
        assertFalse(search.text("ordinary typing"))
        search.arm()
        assertFalse(search.visible)
        assertTrue(search.text("LiNe"))
        assertTrue(search.visible)
        assertEquals(listOf("b", "a"), search.results(apps).map { it.id })
        search.move(1, 2)
        assertEquals(1, search.selected)
        search.text(" x")
        assertEquals(0, search.selected)
        assertTrue(search.results(apps).isEmpty())
        search.clear()
        assertFalse(search.text("do not capture app text"))
    }
    @Test fun thaiLineAliasBackspaceAndEmptyResultsAreSafe() {
        val search = AppSearchModel()
        search.arm(); search.text("ไลน์")
        assertEquals("b", search.results(apps).single().id)
        search.replaceQuery("😀")
        search.backspace()
        assertEquals("", search.query)
        search.backspace()
        search.move(-1, 0)
        assertEquals(0, search.selected)
        search.text("a".repeat(300) + "\n")
        assertEquals(120, search.query.length)
        assertFalse(search.query.contains('\n'))
    }
    @Test fun newWindowsPressClearsPreviousSearchAndSelection() {
        val search = AppSearchModel()
        search.arm(); search.text("line"); search.move(1, 2)
        search.arm()
        assertFalse(search.visible)
        assertTrue(search.armed)
        assertEquals("", search.query)
        assertEquals(0, search.selected)
    }

    @Test fun emptyQueryListsRecentAppsFirstInUseOrderAndTypedQueriesKeepRelevance() {
        val search = AppSearchModel()
        search.arm(); search.text("")
        assertEquals(listOf("b", "a", "c"), search.results(apps).map { it.id })
        search.setRecents(listOf("settings", "jp.naver.line.android", "gone.app"))
        assertEquals(listOf("c", "b", "a"), search.results(apps).map { it.id })
        assertEquals(listOf("c", "b"), search.recentApps(apps, 8).map { it.id })
        assertEquals(listOf("c"), search.recentApps(apps, 1).map { it.id })
        search.replaceQuery("line")
        assertEquals(listOf("b", "a"), search.results(apps).map { it.id })
        search.setRecents(emptyList())
        search.replaceQuery("")
        assertEquals(listOf("b", "a", "c"), search.results(apps).map { it.id })
    }
}
