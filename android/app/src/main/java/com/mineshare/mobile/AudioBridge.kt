package com.mineshare.mobile

import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioPlaybackCaptureConfiguration
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Handler
import android.os.Looper
import android.os.Process
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/** PCM crosses JNI as 48 kHz stereo frames; Rust owns Opus and peer routing. */
class AudioBridge(private val context: Context, private val captureChanged: (Boolean) -> Unit) {
    private var track: AudioTrack? = null
    private var recorder: AudioRecord? = null
    private var projection: MediaProjection? = null
    private var captureJob: Job? = null
    private var playbackJob: Job? = null
    private val audioManager = context.getSystemService(AudioManager::class.java)
    private val volume = SharedMediaVolume(context)
    @Volatile private var capturing = false
    private val attributes = AudioAttributes.Builder()
        .setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
        .setAllowedCapturePolicy(AudioAttributes.ALLOW_CAPTURE_BY_NONE).build()
    @Volatile private var focusGranted = false
    private val focus = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK)
        .setAudioAttributes(attributes).setOnAudioFocusChangeListener { change ->
            focusGranted = change == AudioManager.AUDIOFOCUS_GAIN
        }.build()

    fun startPlayback(scope: CoroutineScope) {
        playbackJob = scope.launch {
            val cadence = AudioPollCadence()
            var playing = false
            var lastFrame = 0L
            var nextVolumePoll = 0L
            while (isActive) {
                val now = android.os.SystemClock.elapsedRealtime()
                if (now >= nextVolumePoll) { refreshVolume(); nextVolumePoll = now + if (capturing) 100 else 500 }
                val samples = NativeBridge.pollAudio()
                if (samples == null || samples.isEmpty()) {
                    if (playing && android.os.SystemClock.elapsedRealtime() - lastFrame > 500) {
                        track?.pause()
                        track?.flush()
                        audioManager.abandonAudioFocusRequest(focus)
                        focusGranted = false
                        playing = false
                    }
                    delay(cadence.emptyDelay(now))
                    continue
                }
                lastFrame = android.os.SystemClock.elapsedRealtime()
                cadence.received(lastFrame)
                if (!playing) {
                    focusGranted = audioManager.requestAudioFocus(focus) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
                    if (!focusGranted) { delay(100); continue }
                    if (track == null) {
                        val minimum = AudioTrack.getMinBufferSize(48_000, AudioFormat.CHANNEL_OUT_STEREO, AudioFormat.ENCODING_PCM_FLOAT)
                        track = AudioTrack.Builder().setAudioAttributes(attributes).setAudioFormat(
                            AudioFormat.Builder().setSampleRate(48_000).setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                                .setEncoding(AudioFormat.ENCODING_PCM_FLOAT).build()
                        ).setBufferSizeInBytes(maxOf(minimum, 1920 * 4 * 4)).setTransferMode(AudioTrack.MODE_STREAM).build()
                    }
                    track?.play()
                    playing = true
                }
                if (focusGranted) track?.write(samples, 0, samples.size, AudioTrack.WRITE_BLOCKING)
            }
        }
    }

    @Suppress("MissingPermission")
    fun startCapture(result: Int, data: Intent, scope: CoroutineScope) {
        stopCapture()
        val media = checkNotNull(context.getSystemService(MediaProjectionManager::class.java).getMediaProjection(result, data)) {
            "Phone audio sharing was not approved."
        }
        projection = media
        media.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() { stopCapture() }
        }, Handler(Looper.getMainLooper()))
        val configuration = AudioPlaybackCaptureConfiguration.Builder(media)
            .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
            .addMatchingUsage(AudioAttributes.USAGE_GAME)
            .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
            .excludeUid(Process.myUid()).build()
        val format = AudioFormat.Builder().setSampleRate(48_000).setChannelMask(AudioFormat.CHANNEL_IN_STEREO)
            .setEncoding(AudioFormat.ENCODING_PCM_FLOAT).build()
        val minimum = AudioRecord.getMinBufferSize(48_000, AudioFormat.CHANNEL_IN_STEREO, AudioFormat.ENCODING_PCM_FLOAT)
        val record = AudioRecord.Builder().setAudioFormat(format).setBufferSizeInBytes(maxOf(minimum, 1920 * 4 * 4))
            .setAudioPlaybackCaptureConfig(configuration).build()
        check(record.state == AudioRecord.STATE_INITIALIZED) { "Phone audio capture could not initialize." }
        recorder = record
        record.startRecording()
        capturing = true
        captureChanged(true)
        captureJob = scope.launch {
            val frame = FloatArray(1920)
            var offset = 0
            while (isActive) {
                val count = record.read(frame, offset, frame.size - offset, AudioRecord.READ_BLOCKING)
                if (count < 0) {
                    if (isActive) {
                        BridgeState.error("Phone audio capture stopped ($count). Start sharing audio again.")
                        stopCapture()
                    }
                    break
                }
                offset += count
                if (offset == frame.size) {
                    val gain = volume.gain
                    for (index in frame.indices) frame[index] *= gain
                    NativeBridge.sendPcm(frame)
                    offset = 0
                }
            }
        }
    }

    @Synchronized fun stopCapture() {
        capturing = false
        volume.updateRouting(false)
        val previous = projection
        projection = null
        captureJob?.cancel()
        captureJob = null
        recorder?.let { runCatching { it.stop() }; it.release() }
        recorder = null
        if (previous != null) {
            previous.stop()
            BridgeState.state.value.peers.filter { it.sendAudio }.forEach { peer ->
                BridgeState.command("audio_route") {
                    put("device_id", peer.id).put("send", false).put("receive", peer.receiveAudio)
                }
            }
            captureChanged(false)
        }
    }

    fun close() {
        stopCapture()
        playbackJob?.cancel()
        track?.let { runCatching { it.stop() }; it.release() }
        track = null
        audioManager.abandonAudioFocusRequest(focus)
        volume.close()
    }

    fun handleVolumeKey(event: android.view.KeyEvent): Boolean = volume.handleKey(event)

    @Synchronized private fun refreshVolume() {
        volume.updateRouting(capturing && MineShareAccessibility.instance != null &&
            BridgeState.state.value.peers.any { it.connected && it.sendAudio })
    }
}
