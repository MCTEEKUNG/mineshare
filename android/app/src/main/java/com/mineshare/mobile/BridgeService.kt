package com.mineshare.mobile

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.content.res.Configuration
import android.os.IBinder
import android.view.WindowManager
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

data class PhonePeer(
    val id: String,
    val name: String,
    val os: String,
    val connected: Boolean,
    val inputActive: Boolean,
    val sendAudio: Boolean,
    val receiveAudio: Boolean,
    val compatible: Boolean = true,
    val online: Boolean = false,
    val paired: Boolean = false,
)
data class PhoneState(
    val running: Boolean = false,
    val deviceId: String = "",
    val peers: List<PhonePeer> = emptyList(),
    val pairing: JSONObject? = null,
    val error: String? = null,
    val accessibility: Boolean = false,
    val sharingAudio: Boolean = false,
    val mediaVolume: Int = 0,
    val mediaVolumeMax: Int = 15,
    val remoteVolume: Boolean = false,
    val paused: Boolean = false,
)

object BridgeState {
    val state = MutableStateFlow(PhoneState())
    val peerSides = java.util.concurrent.ConcurrentHashMap<String, String>()
    @Volatile var service: BridgeService? = null
    fun error(message: String) { state.update { it.copy(error = message) } }
    fun command(type: String, configure: JSONObject.() -> Unit = {}) {
        service?.send(JSONObject().put("type", type).apply(configure))
    }
}

class BridgeService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var discovery: PeerDiscovery? = null
    private var audio: AudioBridge? = null
    private var started = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        BridgeState.service = this
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel("bridge", "การเชื่อมต่อ MineShare", NotificationManager.IMPORTANCE_LOW)
        )
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == "stop") {
            ConnectionPrefs.setPaused(this, true)
            stopSelf()
            return START_NOT_STICKY
        }
        if (NativeBridge.loadError != null) {
            BridgeState.error(NativeBridge.loadError!!)
            stopSelf()
            return START_NOT_STICKY
        }
        try {
            if (!started) {
                updateForeground(false)
                val bounds = getSystemService(WindowManager::class.java).maximumWindowMetrics.bounds
                val id = NativeBridge.start(filesDir.absolutePath, android.os.Build.MODEL, bounds.width(), bounds.height())
                started = true
                val wasPaused = ConnectionPrefs.paused(this)
                BridgeState.state.update { it.copy(running = true, deviceId = id, error = null, paused = wasPaused) }
                if (wasPaused) send(JSONObject().put("type", "pause").put("paused", true))
                audio = AudioBridge(this) { active ->
                    updateForeground(active)
                    BridgeState.state.update { it.copy(sharingAudio = active) }
                }.also { it.startPlayback(scope) }
                scope.launch {
                    var nextPermissionCheck = 0L
                    var lastEventAt = 0L
                    while (isActive) {
                        val now = android.os.SystemClock.elapsedRealtime()
                        if (now >= nextPermissionCheck) {
                            nextPermissionCheck = now + 1000
                            if (android.os.Build.VERSION.SDK_INT >= 37 && checkSelfPermission("android.permission.ACCESS_LOCAL_NETWORK") != android.content.pm.PackageManager.PERMISSION_GRANTED) {
                                BridgeState.error("Local network access was turned off. Allow it to reconnect over Wi-Fi.")
                                stopSelf()
                                break
                            }
                            if (BridgeState.state.value.sharingAudio && checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) != android.content.pm.PackageManager.PERMISSION_GRANTED) audio?.stopCapture()
                        }
                        var event = NativeBridge.poll()
                        var count = 0
                        while (event != null && count < 64) {
                            runCatching { handle(JSONObject(event)) }.onFailure {
                                BridgeState.error(it.message ?: "Could not update the connection.")
                                if (it is SecurityException) stopSelf()
                            }
                            count++
                            // Do not pop an event beyond this batch's budget.
                            if (count < 64) event = NativeBridge.poll()
                        }
                        // Every ms of polling delay is added pointer latency: stay at 1 ms for a moment after
                        // any event (the mouse is moving), relax to 4 ms while connected and 24 ms when idle.
                        if (count > 0) lastEventAt = now
                        delay(if (count == 64 || now - lastEventAt < 300) 1 else if (BridgeState.state.value.peers.any { it.inputActive }) 4 else 24)
                    }
                }
            }
            if (intent?.action == "capture") {
                val data = intent.getParcelableExtra("data", Intent::class.java)
                if (data != null) {
                    updateForeground(true)
                    audio?.startCapture(intent.getIntExtra("result", -1), data, scope)
                    intent.getStringExtra("peer")?.let { peer ->
                        send(JSONObject().put("type", "audio_route").put("device_id", peer)
                            .put("send", true).put("receive", intent.getBooleanExtra("receive", false)))
                    }
                }
            }
            if (intent?.action == "stop_capture") audio?.stopCapture()
        } catch (error: Exception) {
            BridgeState.error(error.message ?: "Could not start the connection.")
            if (intent?.action == "capture") audio?.stopCapture() else stopSelf()
        }
        return START_NOT_STICKY
    }

    fun send(command: JSONObject) {
        if (!started) return
        runCatching { NativeBridge.command(command.toString()) }.onFailure {
            BridgeState.error(it.message ?: "Connection command failed.")
        }
    }

    fun handleVolumeKey(event: android.view.KeyEvent): Boolean = audio?.handleVolumeKey(event) == true

    private fun handle(event: JSONObject) {
        when (event.optString("type")) {
            "identity" -> {
                discovery?.close()
                discovery = PeerDiscovery(this, ::send).also { it.start(event) }
            }
            "snapshot" -> {
                val rows = event.optJSONArray("peers") ?: JSONArray()
                val peers = (0 until rows.length()).map { index ->
                    val p = rows.getJSONObject(index)
                    PhonePeer(p.getString("device_id"), p.optString("display_name", "Computer"),
                        p.optString("os"), p.optBoolean("connected"), p.optBoolean("input_active"),
                        p.optBoolean("send_audio"), p.optBoolean("receive_audio"), p.optBoolean("compatible", true),
                        p.optBoolean("online", p.optBoolean("connected")), p.optBoolean("paired"))
                }
                BridgeState.state.update { it.copy(peers = peers, paused = event.optBoolean("paused", it.paused)) }
            }
            "pairing" -> BridgeState.state.update {
                it.copy(pairing = if (event.optString("kind") == "complete" || event.optString("kind") == "closed") null else event)
            }
            "input" -> {
                if (event.optString("kind") == "peer_side") {
                    BridgeState.peerSides[event.optString("device_id")] = event.optString("side", "left")
                    MineShareAccessibility.instance?.accept(event)
                } else MineShareAccessibility.instance?.accept(event) ?: run {
                    if (event.optString("kind") != "release") {
                        send(JSONObject().put("type", "release_input"))
                        BridgeState.error("เปิดบริการควบคุมด้วย MineShare ในการช่วยเหลือการเข้าถึง แล้วกลับมาลองใช้เมาส์และคีย์บอร์ดอีกครั้ง")
                    }
                }
            }
            "error" -> BridgeState.error(event.optString("message", "Connection failed."))
        }
    }

    private fun updateForeground(capture: Boolean) {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        val stop = PendingIntent.getService(this, 1, Intent(this, BridgeService::class.java).setAction("stop"), PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(this, "bridge")
            .setSmallIcon(com.mineshare.mobile.R.drawable.ic_mineshare_notification).setContentTitle("MineShare พร้อมใช้งาน")
            .setContentText(if (capture) "กำลังแชร์เสียงมือถือ · แตะเพื่อจัดการ" else "พร้อมเชื่อมต่อคอมพิวเตอร์ · แตะเพื่อจัดการ")
            .setContentIntent(open).setOngoing(true).addAction(Notification.Action.Builder(null, "พักการเชื่อมต่อ", stop).build()).build()
        var types = ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
        if (capture) types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION
        startForeground(1, notification, types)
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        val bounds = getSystemService(WindowManager::class.java).maximumWindowMetrics.bounds
        send(JSONObject().put("type", "display").put("width", bounds.width()).put("height", bounds.height()))
        MineShareAccessibility.instance?.refreshBounds()
    }

    override fun onDestroy() {
        discovery?.close()
        audio?.close()
        scope.cancel()
        if (started) NativeBridge.stop()
        started = false
        MineShareAccessibility.instance?.release()
        BridgeState.service = null
        BridgeState.state.update { PhoneState(accessibility = it.accessibility, error = it.error) }
        super.onDestroy()
    }
}

/** Remembers that the user paused MineShare, so reopening the app does not reconnect on its own. */
object ConnectionPrefs {
    private fun prefs(context: android.content.Context) = context.getSharedPreferences("connection", android.content.Context.MODE_PRIVATE)
    fun paused(context: android.content.Context) = prefs(context).getBoolean("paused", false)
    fun setPaused(context: android.content.Context, value: Boolean) { prefs(context).edit().putBoolean("paused", value).apply() }
}
