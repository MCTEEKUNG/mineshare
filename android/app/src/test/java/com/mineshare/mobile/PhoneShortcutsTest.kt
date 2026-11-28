package com.mineshare.mobile

import org.junit.Assert.*
import org.junit.Test

class PhoneShortcutsTest {
    @Test fun rotationAndPointerRecoveryDoNotRepeatOrCollideWithPeerSwitch() {
        val keys = PhoneShortcuts()
        keys.key(29, true); keys.key(56, true)
        assertEquals(PhoneAction.ROTATE, keys.key(24, true).action)
        assertNull(keys.key(24, true).action)
        assertTrue(keys.key(24, false).consumed)
        keys.key(42, true)
        assertEquals(PhoneAction.AUTO_ROTATE, keys.key(24, true).action)
        keys.key(24, false); keys.key(42, false)
        assertEquals(PhoneAction.RECOVER_POINTER, keys.key(57, true).action)
        keys.key(57, false)
        assertFalse(keys.key(19, true).consumed) // Ctrl+Alt+R belongs to the desktop.
        keys.reset()
        assertFalse(keys.key(24, true).consumed)
    }
    @Test fun windowsOpensRecentsOnlyOnReleaseAndDoesNotRepeatAfterAChord() {
        val keys = PhoneShortcuts()
        assertNull(keys.key(125, true).action)
        assertEquals(PhoneAction.RECENTS, keys.key(125, false).action)
        assertNull(keys.key(125, false).action)
        keys.key(125, true)
        assertEquals(PhoneAction.RECENTS, keys.key(15, true).action)
        assertNull(keys.key(15, true).action)
        assertTrue(keys.key(15, false).consumed)
        assertNull(keys.key(125, false).action)
    }

    @Test fun altTabHomeBackAndDisconnectPreserveOrdinaryTyping() {
        val keys = PhoneShortcuts()
        assertFalse(keys.key(30, true).consumed)
        assertFalse(keys.key(30, false).consumed)
        keys.key(56, true)
        assertEquals(PhoneAction.SWITCH_APP, keys.key(15, true).action)
        keys.key(15, false)
        keys.key(56, false)
        keys.key(125, true)
        assertEquals(PhoneAction.HOME, keys.key(32, true).action)
        keys.reset()
        assertNull(keys.key(125, false).action)
        assertEquals(PhoneAction.BACK, keys.key(1, true).action)
        assertTrue(keys.key(1, false).consumed)
        assertFalse(keys.key(29, true).consumed)
    }
}
