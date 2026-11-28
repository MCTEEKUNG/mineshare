package com.mineshare.mobile

import org.junit.Assert.*
import org.junit.Test

class SearchPanelGeometryTest {
    @Test fun portraitPanelFitsCutoutAndNavigationArea() {
        val size = searchPanelGeometry(1080, 2400, 3f, 96, 72, 0, 0, 0)
        assertEquals(1008, size.width)
        assertEquals(1260, size.height)
        assertEquals(132, size.top)
        assertFalse(size.compact)
        assertTrue(size.top + size.height <= 2400 - 72)
    }
    @Test fun landscapeKeyboardKeepsPanelAboveImeWithoutDoubleCountingNavigation() {
        val size = searchPanelGeometry(2400, 1080, 3f, 72, 72, 96, 0, 600)
        assertEquals(1560, size.width)
        assertEquals(336, size.height)
        assertTrue(size.compact)
        assertTrue(size.top + size.height <= 1080 - 600)
        val restored = searchPanelGeometry(2400, 1080, 3f, 72, 72, 96, 0, 0)
        assertTrue(restored.height > size.height)
    }
}
