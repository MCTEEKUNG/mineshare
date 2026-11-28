package com.mineshare.mobile

import org.junit.Assert.*
import org.junit.Test

class SharedVolumeLevelTest {
    @Test fun buttonsAdjustSharedAudioAndDisconnectRestoresTheLatestLevel() {
        val volume = SharedVolumeLevel(15, 6)
        volume.route(6)
        repeat(3) { volume.adjust(-1) }
        assertEquals(3f / 15f, volume.gain, 0.0001f)
        assertEquals(3, volume.restore())
        assertFalse(volume.routed)
        // The user adjusted normal Android volume while disconnected.
        volume.route(10)
        assertEquals(10, volume.level)
        volume.adjust(-1)
        assertEquals(9, volume.restore())
    }

    @Test fun zeroIsSilentAndVolumeUpAlwaysUnmutes() {
        val volume = SharedVolumeLevel(15, 1)
        volume.route(1)
        repeat(30) { volume.adjust(-1) }
        assertEquals(0f, volume.gain, 0f)
        volume.adjust(1)
        assertTrue(volume.gain > 0f)
        repeat(30) { volume.adjust(1) }
        assertEquals(1f, volume.gain, 0f)
        volume.toggleMute()
        assertEquals(0, volume.level)
        volume.toggleMute()
        assertEquals(15, volume.restore())
    }

    @Test fun systemVolumeSliderUsesTheSameLevelAsButtonsAndRestoration() {
        val volume = SharedVolumeLevel(15, 6)
        volume.route(6)
        volume.setLevel(12)
        volume.adjust(-1)
        assertEquals(11, volume.level)
        assertEquals(11f / 15f, volume.gain, 0.0001f)
        assertEquals(11, volume.restore())
        volume.setLevel(100)
        assertEquals(15, volume.level)
    }
}
