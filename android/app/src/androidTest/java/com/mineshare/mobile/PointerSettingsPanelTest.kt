package com.mineshare.mobile

import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Rule
import org.junit.Test

class PointerSettingsPanelTest {
    @get:Rule val compose = createComposeRule()
    private lateinit var original: PointerOptions

    @Before fun preservePreferences() {
        PointerPreferences.initialize(InstrumentationRegistry.getInstrumentation().targetContext)
        original = PointerPreferences.state.value
        PointerPreferences.update(PointerOptions(size = 1.5f, outline = false))
    }
    @After fun restorePreferences() { PointerPreferences.update(original) }

    @Test fun resettingNeedsConfirmationAndCancelKeepsLiveValues() {
        compose.setContent { MineShareTheme { PointerSettingsPanel() } }
        compose.onNodeWithText("ปรับแต่งเมาส์บน Android").performClick()
        compose.onNodeWithText("คืนค่าเริ่มต้น").performClick()
        compose.onNodeWithText("คืนค่าเมาส์เริ่มต้น?").assertIsDisplayed()
        compose.onNodeWithText("เก็บค่าเดิม").performClick()
        compose.runOnIdle { assertEquals(1.5f, PointerPreferences.state.value.size); assertFalse(PointerPreferences.state.value.outline) }
        compose.onNodeWithText("คืนค่าเริ่มต้น").performClick()
        compose.onNodeWithText("คืนค่าทั้งหมด").performClick()
        compose.runOnIdle { assertEquals(PointerOptions(), PointerPreferences.state.value) }
    }

    @Test fun advancedControlsStayDiscoverableAndSwitchLabelTogglesOnce() {
        compose.setContent { MineShareTheme { PointerSettingsPanel() } }
        compose.onNodeWithText("ปรับแต่งเมาส์บน Android").performClick()
        compose.onNodeWithText("ความเร็วการเลื่อน").assertDoesNotExist()
        compose.onNodeWithText("เส้นขอบ").performScrollTo().assertIsOff().performClick().assertIsOn()
        compose.runOnIdle { assertTrue(PointerPreferences.state.value.outline) }
        compose.onNodeWithText("ปรับการเลื่อนและการลาก ▾").performScrollTo().performClick()
        compose.onNodeWithText("ความเร็วการเลื่อน").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("ความลื่นของการลาก").performScrollTo().assertIsDisplayed()
    }
}
