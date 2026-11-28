package com.mineshare.mobile

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.result.ActivityResultLauncher
import android.os.Handler
import android.os.Looper
import java.util.UUID
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.LaunchedEffect
import kotlinx.coroutines.delay
import androidx.compose.ui.Modifier
import org.json.JSONObject

class MainActivity : ComponentActivity() {
    private var pendingAudioPeerId by mutableStateOf<String?>(null)
    private var pendingAudioStage by mutableStateOf("")
    private var pendingAudioToken by mutableStateOf<String?>(null)
    private var externalAudioWindowDeparted = false
    private var destroyed = false
    private val audioResultsHandler = Handler(Looper.getMainLooper())
    private var audioPermissionLauncher: ActivityResultLauncher<String>? = null
    private var audioProjectionLauncher: ActivityResultLauncher<Intent>? = null
    private val permissions = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { granted ->
        if (Build.VERSION.SDK_INT >= 37 && granted["android.permission.ACCESS_LOCAL_NETWORK"] == false) {
            BridgeState.error("อนุญาตเข้าถึงเครือข่ายภายใน เพื่อเชื่อมต่อคอมพิวเตอร์ผ่าน Wi-Fi")
        } else startBridge()
    }
    private fun registerAudioResults(token: String) {
        audioPermissionLauncher = activityResultRegistry.register("audio-permission:$token",
            ActivityResultContracts.RequestPermission()) { granted ->
            audioResultsHandler.post {
                if (!acceptsAudioResult(token, "permission")) return@post
                if (granted) requestProjection(token) else {
                    clearPendingAudio()
                    BridgeState.error("ยังไม่ได้อนุญาตบันทึกเสียง จึงยังไม่เริ่มแชร์เสียงมือถือ ลองเปิดแชร์อีกครั้งเมื่อพร้อม")
                }
            }
        }
        audioProjectionLauncher = activityResultRegistry.register("audio-projection:$token",
            ActivityResultContracts.StartActivityForResult()) { result ->
            audioResultsHandler.post {
                if (!acceptsAudioResult(token, "projection")) return@post
                val peer = BridgeState.state.value.peers.firstOrNull { it.id == pendingAudioPeerId && it.connected }
                if (result.resultCode == RESULT_OK && result.data != null && peer != null) {
                    pendingAudioStage = "starting"
                    externalAudioWindowDeparted = false
                    runCatching {
                        startService(Intent(this, BridgeService::class.java).setAction("capture")
                            .putExtra("result", result.resultCode).putExtra("data", result.data)
                            .putExtra("peer", peer.id).putExtra("receive", peer.receiveAudio))
                    }.onFailure { clearPendingAudio(); BridgeState.error("เริ่มแชร์เสียงไม่ได้: ${it.message}") }
                } else {
                    clearPendingAudio()
                    BridgeState.error(if (peer == null) "อุปกรณ์ปลายทางไม่เชื่อมต่อแล้ว เชื่อมต่อใหม่ก่อนแชร์เสียง"
                        else "ยกเลิกการอนุญาตแล้ว ยังไม่เริ่มแชร์เสียงมือถือ")
                }
            }
        }
    }

    private fun acceptsAudioResult(token: String, stage: String) =
        !destroyed && pendingAudioToken == token && pendingAudioStage == stage && pendingAudioPeerId != null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        pendingAudioPeerId = savedInstanceState?.getString("pending_audio_peer")
        pendingAudioStage = savedInstanceState?.getString("pending_audio_stage").orEmpty()
        pendingAudioToken = savedInstanceState?.getString("pending_audio_token")
        externalAudioWindowDeparted = savedInstanceState?.getBoolean("pending_audio_departed", false) ?: false
        val restoredToken = pendingAudioToken
        if (pendingAudioPeerId != null && restoredToken != null && pendingAudioStage in setOf("permission", "projection", "starting")) {
            if (pendingAudioStage != "starting") externalAudioWindowDeparted = true
            registerAudioResults(restoredToken)
        } else clearPendingAudio()
        PointerPreferences.initialize(this)
        volumeControlStream = android.media.AudioManager.STREAM_MUSIC
        setContent {
            MineShareTheme {
                val state by BridgeState.state.collectAsState()
                LaunchedEffect(state.sharingAudio, state.error, state.peers, state.running, pendingAudioToken, pendingAudioStage) {
                    if (pendingAudioToken != null) {
                        if (!state.running || state.peers.none { it.id == pendingAudioPeerId && it.connected }) {
                            clearPendingAudio()
                            BridgeState.error("อุปกรณ์ปลายทางไม่เชื่อมต่อแล้ว เชื่อมต่อใหม่ก่อนแชร์เสียง")
                        } else if (pendingAudioStage == "starting" && (state.sharingAudio || state.error != null)) {
                            clearPendingAudio()
                        }
                    }
                }
                LaunchedEffect(pendingAudioPeerId, pendingAudioStage) {
                    if (pendingAudioStage == "starting") {
                        delay(10_000)
                        if (pendingAudioPeerId != null && !BridgeState.state.value.sharingAudio) {
                            clearPendingAudio()
                            BridgeState.error("ยังเริ่มแชร์เสียงไม่ได้ ลองเปิดแชร์อีกครั้ง")
                        }
                    }
                }
                GlassBackground {
                    PhoneHome(state, ::requestStart, ::shareAudio)
                }
                state.pairing?.let { PairingDialog(it, state.error) }
            }
        }
    }

    override fun onResume() {
        super.onResume()
        BridgeState.state.value = BridgeState.state.value.copy(accessibility = MineShareAccessibility.instance != null)
    }

    override fun onSaveInstanceState(outState: Bundle) {
        outState.putString("pending_audio_peer", pendingAudioPeerId)
        outState.putString("pending_audio_stage", pendingAudioStage)
        outState.putString("pending_audio_token", pendingAudioToken)
        outState.putBoolean("pending_audio_departed", externalAudioWindowDeparted)
        super.onSaveInstanceState(outState)
    }

    private fun clearPendingAudio() {
        // Invalidate first: a queued result from an old request must never reach
        // a newly selected peer, even if Android delivers it after unregister.
        pendingAudioToken = null
        pendingAudioPeerId = null
        pendingAudioStage = ""
        externalAudioWindowDeparted = false
        audioPermissionLauncher?.unregister(); audioPermissionLauncher = null
        audioProjectionLauncher?.unregister(); audioProjectionLauncher = null
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        val token = pendingAudioToken ?: return
        val stage = pendingAudioStage
        if (stage != "permission" && stage != "projection") return
        if (!hasFocus) externalAudioWindowDeparted = true
        else if (externalAudioWindowDeparted) {
            // Results are posted first; only recover when the external UI has
            // closed and this exact request still has not received a result.
            audioResultsHandler.postDelayed({
                if (hasWindowFocus() && acceptsAudioResult(token, stage)) {
                    clearPendingAudio()
                    BridgeState.error("การขออนุญาตแชร์เสียงถูกขัดจังหวะ ลองเปิดแชร์เสียงอีกครั้ง")
                }
            }, 300)
        }
    }

    override fun onDestroy() {
        destroyed = true
        audioResultsHandler.removeCallbacksAndMessages(null)
        audioPermissionLauncher?.unregister()
        audioProjectionLauncher?.unregister()
        super.onDestroy()
    }

    private fun requestStart() {
        val wanted = mutableListOf(Manifest.permission.POST_NOTIFICATIONS)
        if (Build.VERSION.SDK_INT >= 37) wanted += "android.permission.ACCESS_LOCAL_NETWORK"
        val missing = wanted.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isEmpty()) startBridge() else permissions.launch(missing.toTypedArray())
    }

    private fun startBridge() { startForegroundService(Intent(this, BridgeService::class.java)) }

    private fun shareAudio(peer: PhonePeer) {
        if (pendingAudioPeerId != null) return
        if (peer.sendAudio && BridgeState.state.value.sharingAudio) {
            BridgeState.command("audio_route") { put("device_id", peer.id).put("send", false).put("receive", peer.receiveAudio) }
            return
        }
        if (BridgeState.state.value.sharingAudio) {
            BridgeState.command("audio_route") { put("device_id", peer.id).put("send", true).put("receive", peer.receiveAudio) }
            return
        }
        BridgeState.state.value = BridgeState.state.value.copy(error = null)
        pendingAudioPeerId = peer.id
        pendingAudioToken = UUID.randomUUID().toString()
        pendingAudioStage = "permission"
        externalAudioWindowDeparted = !hasWindowFocus()
        val token = checkNotNull(pendingAudioToken)
        registerAudioResults(token)
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) requestProjection(token)
        else runCatching { checkNotNull(audioPermissionLauncher).launch(Manifest.permission.RECORD_AUDIO) }
            .onFailure { clearPendingAudio(); BridgeState.error("เปิดหน้าขออนุญาตเสียงไม่ได้: ${it.message}") }
    }

    private fun requestProjection(token: String) {
        if (!acceptsAudioResult(token, "permission")) return
        if (BridgeState.state.value.peers.none { it.id == pendingAudioPeerId && it.connected }) {
            clearPendingAudio()
            BridgeState.error("อุปกรณ์ปลายทางไม่เชื่อมต่อแล้ว เชื่อมต่อใหม่ก่อนแชร์เสียง")
            return
        }
        externalAudioWindowDeparted = !hasWindowFocus()
        pendingAudioStage = "projection"
        runCatching {
            checkNotNull(audioProjectionLauncher).launch(getSystemService(MediaProjectionManager::class.java).createScreenCaptureIntent(
                android.media.projection.MediaProjectionConfig.createConfigForDefaultDisplay()))
        }.onFailure { clearPendingAudio(); BridgeState.error("เปิดหน้ายืนยันแชร์เสียงไม่ได้: ${it.message}") }
    }

    @Composable private fun PhoneHome(state: PhoneState, connect: () -> Unit, share: (PhonePeer) -> Unit) {
        PhoneDashboard(state, connect, share,
            pause = { startService(Intent(this, BridgeService::class.java).setAction("stop")) },
            stopAudio = { startService(Intent(this, BridgeService::class.java).setAction("stop_capture")) },
            accessibility = { startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) },
            rotate = { portrait, auto -> PhoneRotation.apply(this, portrait, auto) },
            pendingAudioPeerId = pendingAudioPeerId,
            pendingAudioMessage = if (pendingAudioStage == "starting") "กำลังเริ่มแชร์เสียง…" else "รออนุญาตจาก Android…")
    }

    @Composable private fun PairingDialog(pairing: JSONObject, error: String?) {
        val attempt = pairing.optLong("attempt_id", -1)
        PhonePairingDialog(
            attempt = attempt,
            deviceName = pairing.optString("display_name").ifBlank { "อุปกรณ์นี้" },
            kind = pairing.optString("kind"),
            displayedCode = pairing.optString("pin"),
            error = error,
            onConsent = { BridgeState.command("consent") { put("attempt_id", attempt).put("accept", true) } },
            onSubmitCode = { pin -> BridgeState.command("pin") { put("attempt_id", attempt).put("pin", pin) } },
            onCancel = {
                BridgeState.command("cancel_pairing") { put("attempt_id", attempt) }
                BridgeState.state.value = BridgeState.state.value.copy(pairing = null)
            },
        )
    }
}
