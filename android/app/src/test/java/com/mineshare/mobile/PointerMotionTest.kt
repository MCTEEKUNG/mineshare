package com.mineshare.mobile

import org.junit.Assert.*
import org.junit.Test

class PointerMotionTest {
    @Test fun dragPreservesCornersAndFinalReleasePositionBetweenCallbacks() {
        val drag = DragMotion()
        val path = listOf(PointerPoint(10f, 10f), PointerPoint(100f, 10f), PointerPoint(100f, 100f))
        path.forEach { drag.add(it) }
        drag.add(path.last())
        assertEquals(path, drag.drain())
        assertTrue(drag.isEmpty)
        drag.add(PointerPoint(120f, 100f))
        assertEquals(listOf(PointerPoint(120f, 100f)), drag.drain())
        drag.add(PointerPoint(1f, 1f))
        drag.clear()
        assertTrue(drag.isEmpty)
    }
    @Test fun wheelInputReceivedDuringAnAnimationIsRetainedInOrder() {
        val wheel = ScrollMotion()
        val options = PointerOptions()
        wheel.add(0f, 1f, options, 1000, 1000)
        assertEquals(100f, wheel.take(1000, 1000)!!.y, 0.001f)
        // More wheel packets arrive while the first Android gesture is busy.
        repeat(5) { wheel.add(0f, 2f, options, 1000, 1000) }
        val first = wheel.take(1000, 1000)!!
        val second = wheel.take(1000, 1000)!!
        assertEquals(1000f, first.y + second.y, 0.001f)
        assertNull(wheel.take(1000, 1000))
        wheel.add(1f, -1f, options.copy(scrollSpeed = 2f), 1000, 1000)
        assertEquals(PointerPoint(-200f, -200f), wheel.take(1000, 1000))
        wheel.clear()
        assertNull(wheel.take(1000, 1000))
    }

    @Test fun smoothingReachesTheExactClickTargetWithoutOvershoot() {
        var position = 0f
        repeat(100) { position = smoothPointer(position, 100f, 1f / 120, 1f); assertTrue(position <= 100f) }
        assertEquals(100f, position, 0f)
        assertEquals(100f, smoothPointer(0f, 100f, 1f / 120, 0f), 0f)
    }

    @Test fun invalidSettingsAndInputCannotCreateUnboundedGestures() {
        val options = PointerOptions(size = Float.NaN, scrollSpeed = 100f, dragSmoothing = -1f).normalized()
        assertEquals(1f, options.size, 0f)
        assertEquals(3f, options.scrollSpeed, 0f)
        assertEquals(8L, options.dragDuration)
        val wheel = ScrollMotion()
        wheel.add(Float.NaN, 1f, options, 1000, 1000)
        assertNull(wheel.take(1000, 1000))
        wheel.add(0f, 10000f, options, 1000, 1000)
        var batches = 0
        while (wheel.take(1000, 1000) != null) { batches++; assertTrue(batches <= 4) }
    }
}
