package com.mineshare.mobile

import org.junit.Assert.*
import org.junit.Test

class AudioPollCadenceTest {
    @Test fun silenceBacksOffButActivePlaybackKeepsLowLatency() {
        val cadence = AudioPollCadence()
        assertEquals(listOf(5L, 10L, 20L, 40L, 40L), (0..4).map { cadence.emptyDelay(1000) })
        cadence.received(1100)
        assertEquals(5L, cadence.emptyDelay(1101))
        assertEquals(5L, cadence.emptyDelay(1600))
        assertEquals(5L, cadence.emptyDelay(1601))
        assertEquals(10L, cadence.emptyDelay(1610))
        cadence.received(1620)
        assertEquals(5L, cadence.emptyDelay(1621))
    }
}
