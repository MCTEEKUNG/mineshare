package com.mineshare.mobile

import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

class PhonePairingDialogTest {
    @get:Rule val compose = createComposeRule()

    @Test fun onlyAsciiCodeCanSubmitAndWaitingCanBeCancelled() {
        val submitted = mutableListOf<String>()
        var cancelled = 0
        compose.setContent { MineShareTheme {
            PhonePairingDialog(21, "My laptop", "enter_pin", onConsent = {}, onSubmitCode = { submitted.add(it) },
                onCancel = { cancelled++ })
        } }
        compose.onNodeWithText("ยืนยันรหัส").assertIsNotEnabled()
        val field = compose.onNode(hasSetTextAction())
        field.performTextInput("１２๓")
        assertEquals("", field.fetchSemanticsNode().config[SemanticsProperties.EditableText].text)
        compose.onNodeWithText("ยืนยันรหัส").assertIsNotEnabled()
        field.performTextInput("012345")
        compose.onNodeWithText("ยืนยันรหัส").assertIsEnabled().performClick()
        compose.onNodeWithText("ส่งรหัสแล้ว").assertIsNotEnabled()
        compose.onNodeWithText("ยกเลิก").performClick()
        compose.runOnIdle { assertEquals(listOf("012345"), submitted); assertEquals(1, cancelled) }
    }

    @Test fun consentBecomesPendingAndCannotBeSubmittedAgain() {
        var accepted = 0
        compose.setContent { MineShareTheme {
            PhonePairingDialog(22, "My laptop", "consent", onConsent = { accepted++ }, onSubmitCode = {}, onCancel = {})
        } }
        compose.onNodeWithText("ยอมรับการจับคู่").performClick()
        compose.onNodeWithText("ยอมรับแล้ว").assertIsNotEnabled()
        compose.onNodeWithText("ยกเลิก").assertIsEnabled()
        compose.runOnIdle { assertEquals(1, accepted) }
    }

    @Test fun displayedCodeExplainsWhichDeviceNeedsIt() {
        compose.setContent { MineShareTheme {
            PhonePairingDialog(23, "My laptop", "show_pin", displayedCode = "123456", onConsent = {}, onSubmitCode = {}, onCancel = {})
        } }
        compose.onNodeWithText("123 456").assertIsDisplayed()
        compose.onNodeWithText("ไม่ต้องกรอกรหัสบนมือถือเครื่องนี้").assertIsDisplayed()
        compose.onNodeWithText("บน My laptop ให้กรอกรหัส 6 หลักนี้เพื่อยืนยันการจับคู่").assertIsDisplayed()
    }

    @Test fun passiveWaitOffersRecoveryWithoutAnotherSubmission() {
        compose.mainClock.autoAdvance = false
        compose.setContent { MineShareTheme {
            PhonePairingDialog(24, "My laptop", "show_pin", displayedCode = "123456",
                onConsent = {}, onSubmitCode = {}, onCancel = {})
        } }
        compose.mainClock.advanceTimeBy(16_000)
        compose.onNodeWithText("ยกเลิกแล้วลองใหม่").assertIsEnabled()
    }
}
