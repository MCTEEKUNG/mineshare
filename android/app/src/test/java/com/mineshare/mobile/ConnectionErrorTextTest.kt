package com.mineshare.mobile

import org.junit.Assert.*
import org.junit.Test

class ConnectionErrorTextTest {
    @Test fun refusedConnectionGetsNextActionAndKeepsExactDiagnostic() {
        val original = "Connection refused (os error 111)"
        val text = connectionErrorText(original)
        assertEquals("ยังเชื่อมต่อไม่ได้", text.title)
        assertTrue(text.message.contains("เปิด MineShare"))
        assertTrue(text.message.contains("ลองเชื่อมต่ออีกครั้ง"))
        assertEquals(original, text.details)
    }

    @Test fun recognizedFailuresHaveDistinctRecoveryGuidance() {
        val cases = mapOf(
            "Network is unreachable (os error 101)" to "ยังติดต่ออุปกรณ์ไม่ได้",
            "No route to host" to "ยังติดต่ออุปกรณ์ไม่ได้",
            "Device is offline" to "อุปกรณ์ยังไม่พร้อมเชื่อมต่อ",
            "Connection TIMED OUT" to "ยังไม่ได้รับคำตอบ",
            "Pairing request expired" to "เริ่มจับคู่อีกครั้ง",
            "Pairing timeout" to "เริ่มจับคู่อีกครั้ง",
            "PIN expired" to "เริ่มจับคู่อีกครั้ง",
        )
        cases.forEach { (original, expectedTitle) ->
            val text = connectionErrorText(original)
            assertEquals(original, expectedTitle, text.title)
            assertEquals(original, text.details)
            assertTrue(text.message.any { it in '\u0E00'..'\u0E7F' })
        }
    }

    @Test fun actionableThaiMessageIsNotReplacedOrHidden() {
        val original = "ยังไม่ได้อนุญาตบันทึกเสียง จึงยังไม่เริ่มแชร์เสียงมือถือ ลองเปิดแชร์อีกครั้งเมื่อพร้อม"
        val text = connectionErrorText(original)
        assertEquals(original, text.message)
        assertNull(text.details)
    }

    @Test fun unknownFailureUsesSafeFallbackAndPreservesLongOriginal() {
        val original = "Unexpected failure: " + "diagnostic\n".repeat(300)
        val text = connectionErrorText(original)
        assertEquals("ยังทำรายการไม่สำเร็จ", text.title)
        assertTrue(text.message.contains("ลองทำรายการอีกครั้ง"))
        assertEquals(original, text.details)
        assertEquals("", connectionErrorText("").details)
        assertEquals("ยังทำรายการไม่สำเร็จ", connectionErrorText("Mapping cache expired").title)
    }
}
