package com.mineshare.mobile

import org.junit.Assert.*
import org.junit.Test

class PairingCodeTest {
    @Test fun acceptsExactlySixAsciiDigitsAndKeepsLeadingZeroes() {
        assertTrue(PairingCode.valid("012345"))
        assertFalse(PairingCode.valid("12345"))
        assertFalse(PairingCode.valid("1234567"))
        assertFalse(PairingCode.valid("๑๒๓๔๕๖"))
        assertFalse(PairingCode.valid("１２３４５６"))
        assertFalse(PairingCode.valid("123 45"))
    }
    @Test fun pasteFiltersSeparatorsAndUnsupportedDigitsWithoutChangingAscii() {
        assertEquals("012345", PairingCode.filter(" 012-345 678 "))
        assertEquals("", PairingCode.filter("๑๒๓๔๕๖１２３"))
        assertEquals("123", PairingCode.filter("ab1๒2\n3"))
    }
}
