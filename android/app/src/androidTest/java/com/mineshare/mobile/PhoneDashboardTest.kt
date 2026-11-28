package com.mineshare.mobile

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.junit4.StateRestorationTester
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

/** UI-only fixtures run in the instrumentation APK, never in the shipped app. */
class PhoneDashboardTest {
    @get:Rule val compose = createComposeRule()
    private val laptop = PhonePeer("fixture-laptop", "My laptop", "windows", connected = true,
        inputActive = false, sendAudio = false, receiveAudio = false, online = true, paired = true)

    @Composable private fun Frame(content: @Composable () -> Unit) {
        MineShareTheme {
            Box(Modifier.requiredSize(360.dp, 800.dp)) { content() }
        }
    }

    @Test fun connectedDeviceAndAudioAreVisibleWithoutScrolling() {
        compose.setContent {
            Frame { PhoneDashboard(PhoneState(running = true, accessibility = true, peers = listOf(laptop)),
                connect = {}, share = {}, pause = {}, stopAudio = {}, accessibility = {}, rotate = { _, _ -> }) }
        }
        compose.onNodeWithText("My laptop").assertIsDisplayed()
        compose.onNodeWithText("เสียงคอมพิวเตอร์ → มือถือ").assertIsDisplayed()
        compose.onNodeWithText("เสียงมือถือ → คอมพิวเตอร์").assertIsDisplayed()
    }

    @Test fun connectedWithoutControlLeadsWithThePermissionInLandscape() {
        var opened = 0
        compose.setContent {
            MineShareTheme {
                Box(Modifier.requiredSize(800.dp, 360.dp)) {
                    PhoneDashboard(PhoneState(running = true, accessibility = false, peers = listOf(laptop)),
                        connect = {}, share = {}, pause = {}, stopAudio = {}, accessibility = { opened++ }, rotate = { _, _ -> })
                }
            }
        }
        val warning = compose.onNodeWithText("เปิดการควบคุมมือถือก่อนใช้เมาส์").assertIsDisplayed()
        compose.onNodeWithText("เปิดสิทธิ์ควบคุม").assertIsDisplayed().performClick()
        assertEquals(1, opened)
        // A shallow landscape screen may not even compose the device card; if it does, it must be below the warning.
        val devices = compose.onAllNodesWithText("My laptop").fetchSemanticsNodes()
        val warningTop = warning.fetchSemanticsNode().boundsInRoot.top
        assert(devices.all { warningTop < it.boundsInRoot.top }) { "Control warning must come before the device card" }
    }

    @Test fun audioLabelIsOneSwitchAndOneTapRequestsSharingOnce() {
        var calls = 0
        compose.setContent {
            var state by remember { mutableStateOf(PhoneState(running = true, accessibility = true, peers = listOf(laptop))) }
            Frame { PhoneDashboard(state, connect = {}, share = {
                calls++
                state = state.copy(sharingAudio = true, peers = listOf(laptop.copy(sendAudio = true)))
            }, pause = {}, stopAudio = {}, accessibility = {}, rotate = { _, _ -> }) }
        }
        compose.onNodeWithText("เสียงมือถือ → คอมพิวเตอร์").assertIsOff().performClick().assertIsOn()
        compose.runOnIdle { assertEquals(1, calls) }
    }

    @Test fun startingConnectionDoesNotRequireFindingAnAdvancedSetting() {
        var calls = 0
        compose.setContent {
            Frame { PhoneDashboard(PhoneState(), connect = { calls++ }, share = {}, pause = {}, stopAudio = {},
                accessibility = {}, rotate = { _, _ -> }) }
        }
        compose.onNodeWithText("เริ่มเชื่อมต่อผ่าน Wi-Fi").assertIsDisplayed().performClick()
        compose.runOnIdle { assertEquals(1, calls) }
        compose.onNodeWithText("ปรับแต่ง").performClick()
        compose.onNodeWithText("ปรับแต่งเมาส์บน Android").assertIsDisplayed()
        compose.onNodeWithText("อุปกรณ์").performClick()
        compose.onNodeWithText("เริ่มเชื่อมต่อผ่าน Wi-Fi").assertIsDisplayed()
    }

    @Test fun forgettingRequiresConfirmationAndCancelKeepsDevice() {
        compose.setContent {
            Frame { PhoneDashboard(PhoneState(running = true, accessibility = true, peers = listOf(laptop)),
                connect = {}, share = {}, pause = {}, stopAudio = {}, accessibility = {}, rotate = { _, _ -> }) }
        }
        compose.onNodeWithText("ลบการจับคู่").performScrollTo().performClick()
        compose.onNodeWithText("ลบการจับคู่ My laptop?").assertIsDisplayed()
        compose.onNodeWithText("ยกเลิก").performClick()
        compose.onNodeWithText("My laptop").assertIsDisplayed()
    }

    @Test fun pendingAudioShowsWaitingAndDoesNotPretendToBeSharing() {
        var calls = 0
        compose.setContent {
            Frame { PhoneDashboard(PhoneState(running = true, accessibility = true, peers = listOf(laptop)),
                connect = {}, share = { calls++ }, pause = {}, stopAudio = {}, accessibility = {}, rotate = { _, _ -> },
                pendingAudioPeerId = laptop.id) }
        }
        compose.onNodeWithText("เสียงมือถือ → คอมพิวเตอร์").assertIsOff().assertIsNotEnabled()
        compose.runOnIdle { assertEquals(0, calls) }
    }

    @Test fun sessionActionsStayReachableAcrossTabsWithManyOfflineDevices() {
        var stops = 0
        var pauses = 0
        val peers = listOf(laptop.copy(inputActive = true, sendAudio = true)) +
            (1..30).map { laptop.copy(id = "offline-$it", name = "Offline $it", connected = false, online = false) }
        compose.setContent {
            Frame { PhoneDashboard(PhoneState(running = true, accessibility = true, sharingAudio = true, peers = peers),
                connect = {}, share = {}, pause = { pauses++ }, stopAudio = { stops++ }, accessibility = {}, rotate = { _, _ -> }) }
        }
        // These actions live outside the device list; no scroll through offline peers is needed.
        compose.onNodeWithContentDescription("หยุดรับการควบคุมจากคอมพิวเตอร์").assertIsDisplayed()
        compose.onNodeWithContentDescription("หยุดแชร์เสียงมือถือ").assertIsDisplayed().performClick()
        compose.onNodeWithText("ปรับแต่ง").performClick()
        compose.onNodeWithContentDescription("หยุดแชร์เสียงมือถือ").assertIsDisplayed().performClick()
        compose.onNodeWithContentDescription("พักการเชื่อมต่อทั้งหมด").assertIsDisplayed().performClick()
        compose.onNodeWithText("วิธีใช้").performClick()
        compose.onNodeWithContentDescription("หยุดรับการควบคุมจากคอมพิวเตอร์").assertIsDisplayed()
        compose.onNodeWithContentDescription("หยุดแชร์เสียงมือถือ").assertIsDisplayed().performClick()
        compose.onNodeWithContentDescription("พักการเชื่อมต่อทั้งหมด").assertIsDisplayed().performClick()
        compose.runOnIdle { assertEquals(3, stops); assertEquals(2, pauses) }
    }

    @Test fun connectedOnlyDashboardHasNoEmptyDeviceSection() {
        compose.setContent {
            Frame { PhoneDashboard(PhoneState(running = true, accessibility = true, peers = listOf(laptop)),
                connect = {}, share = {}, pause = {}, stopAudio = {}, accessibility = {}, rotate = { _, _ -> }) }
        }
        compose.onNodeWithText("อุปกรณ์ของคุณ").assertDoesNotExist()
        compose.onNodeWithText("เสียงมือถือ → คอมพิวเตอร์").assertIsDisplayed()
    }

    private fun scrollToText(text: String) {
        compose.onNode(hasScrollToIndexAction()).performScrollToNode(hasText(text))
        compose.onNodeWithText(text).assertIsDisplayed()
    }

    @Test fun eachTabKeepsItsOwnReadingPositionAndNewTabsStartAtTheTop() {
        val peers = listOf(laptop) + (1..30).map {
            laptop.copy(id = "offline-$it", name = "Offline $it", connected = false, online = false)
        }
        compose.setContent {
            Frame { PhoneDashboard(PhoneState(running = true, accessibility = true, peers = peers),
                connect = {}, share = {}, pause = {}, stopAudio = {}, accessibility = {}, rotate = { _, _ -> }) }
        }
        scrollToText("Offline 15")
        compose.onNodeWithText("ปรับแต่ง").performClick()
        compose.onNodeWithText("ปรับแต่งเมาส์บน Android").assertIsDisplayed()
        scrollToText("ค้นหาแอปบนมือถือ")
        compose.onNodeWithText("วิธีใช้").performClick()
        compose.onNodeWithText("เริ่มใช้งาน MineShare").assertIsDisplayed()
        scrollToText("Ctrl+Alt+Space")
        compose.onNodeWithText("อุปกรณ์").performClick()
        compose.onNodeWithText("Offline 15").assertIsDisplayed()
        compose.onNodeWithText("ปรับแต่ง").performClick()
        compose.onNodeWithText("ค้นหาแอปบนมือถือ").assertIsDisplayed()
        compose.onNodeWithText("วิธีใช้").performClick()
        compose.onNodeWithText("Ctrl+Alt+Space").assertIsDisplayed()
    }

    @Test fun restoringDashboardKeepsSelectedTabAndReadingPositions() {
        val restoration = StateRestorationTester(compose)
        val peers = listOf(laptop) + (1..30).map {
            laptop.copy(id = "offline-$it", name = "Offline $it", connected = false, online = false)
        }
        restoration.setContent {
            Frame { PhoneDashboard(PhoneState(running = true, accessibility = true, peers = peers),
                connect = {}, share = {}, pause = {}, stopAudio = {}, accessibility = {}, rotate = { _, _ -> }) }
        }
        scrollToText("Offline 15")
        compose.onNodeWithText("วิธีใช้").performClick()
        scrollToText("Ctrl+Alt+Space")
        restoration.emulateSavedInstanceStateRestore()
        compose.onNodeWithText("วิธีใช้").assertIsSelected()
        compose.onNodeWithText("Ctrl+Alt+Space").assertIsDisplayed()
        compose.onNodeWithText("อุปกรณ์").performClick()
        compose.onNodeWithText("Offline 15").assertIsDisplayed()
    }

    @Test fun largeTextInShallowViewportKeepsNavigationAndSessionActionsUsable() {
        var stops = 0
        var pauses = 0
        compose.setContent {
            val density = LocalDensity.current
            CompositionLocalProvider(LocalDensity provides Density(density.density, fontScale = 2f)) {
                MineShareTheme {
                    // Uses up to 800x360 dp on a landscape device, and stays inside a smaller host.
                    BoxWithConstraints {
                        Box(Modifier.width(minOf(maxWidth, 800.dp)).height(minOf(maxHeight, 360.dp))) {
                            PhoneDashboard(PhoneState(running = true, accessibility = true, sharingAudio = true,
                                peers = listOf(laptop.copy(inputActive = true, sendAudio = true))),
                                connect = {}, share = {}, pause = { pauses++ }, stopAudio = { stops++ },
                                accessibility = {}, rotate = { _, _ -> })
                        }
                    }
                }
            }
        }
        listOf("ปรับแต่ง", "วิธีใช้", "อุปกรณ์").forEach { tab ->
            compose.onNodeWithText(tab).assertIsDisplayed().performClick().assertIsSelected()
            compose.onNodeWithContentDescription("หยุดรับการควบคุมจากคอมพิวเตอร์")
                .performScrollTo().assertIsDisplayed()
            compose.onNodeWithContentDescription("หยุดแชร์เสียงมือถือ")
                .performScrollTo().assertIsDisplayed().performClick()
            compose.onNodeWithContentDescription("พักการเชื่อมต่อทั้งหมด")
                .performScrollTo().assertIsDisplayed().performClick()
        }
        compose.runOnIdle { assertEquals(3, stops); assertEquals(3, pauses) }
    }

    @Test fun connectionFailureOffersRecoveryAndKeepsOriginalDetails() {
        val previous = BridgeState.state.value
        val diagnostic = "Connection refused (os error 111)"
        try {
            BridgeState.state.value = PhoneState(error = diagnostic)
            compose.setContent {
                val state by BridgeState.state.collectAsState()
                Frame { PhoneDashboard(state, connect = {}, share = {}, pause = {}, stopAudio = {},
                    accessibility = {}, rotate = { _, _ -> }) }
            }
            compose.onNodeWithText("ยังเชื่อมต่อไม่ได้").assertIsDisplayed()
            compose.onNodeWithText(diagnostic).assertDoesNotExist()
            compose.onNodeWithText("ดูรายละเอียด").performScrollTo().performClick()
            compose.onNodeWithText(diagnostic).assertIsDisplayed()
            compose.onNodeWithText("ซ่อนรายละเอียด").performClick()
            compose.onNodeWithText(diagnostic).assertDoesNotExist()
            compose.onNodeWithText("ปิดข้อความ").performScrollTo().performClick()
            compose.onNodeWithText("ยังเชื่อมต่อไม่ได้").assertDoesNotExist()
        } finally {
            compose.runOnIdle { BridgeState.state.value = previous }
        }
    }

    @Test fun landscapeShowsBothAudioDirectionsWithoutScrolling() {
        org.junit.Assume.assumeTrue("Run this matrix case on a wide landscape emulator",
            androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().targetContext.resources.configuration.screenWidthDp >= 800)
        compose.setContent {
            MineShareTheme {
                BoxWithConstraints {
                    Box(Modifier.width(minOf(maxWidth, 800.dp)).height(minOf(maxHeight, 360.dp))) {
                        PhoneDashboard(PhoneState(running = true, accessibility = true, peers = listOf(laptop)),
                            connect = {}, share = {}, pause = {}, stopAudio = {}, accessibility = {}, rotate = { _, _ -> })
                    }
                }
            }
        }
        compose.onNodeWithText("เสียงคอมพิวเตอร์ → มือถือ").assertIsDisplayed()
        compose.onNodeWithText("เสียงมือถือ → คอมพิวเตอร์").assertIsDisplayed()
    }
}
