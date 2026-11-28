package com.mineshare.mobile

import android.content.Context
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.VolumeProvider
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Handler
import android.os.Looper
import android.view.KeyEvent
import kotlinx.coroutines.flow.update

/** Mute the local speaker only while an enabled destination receives capture. */
class SharedMediaVolume(private val context: Context) {
    private val manager = context.getSystemService(AudioManager::class.java)
    private val saved = context.getSharedPreferences("shared_media_volume", Context.MODE_PRIVATE)
    private val handler = Handler(Looper.getMainLooper())
    private val consumed = mutableSetOf<Int>()
    private val model = SharedVolumeLevel(manager.getStreamMaxVolume(AudioManager.STREAM_MUSIC), current())
    private var session: MediaSession? = null
    private var provider: VolumeProvider? = null
    @Volatile var gain = model.gain
        private set

    init {
        // Recover the latest logical level if Android killed the process while
        // the speaker was muted. Never leave the user's phone stuck at zero.
        if (saved.getBoolean("restore_pending", false)) {
            if (current() == 0) manager.setStreamVolume(AudioManager.STREAM_MUSIC, saved.getInt("level", current()).coerceIn(0, model.maximum), 0)
            saved.edit().remove("restore_pending").apply()
        }
    }

    private fun current() = manager.getStreamVolume(AudioManager.STREAM_MUSIC)

    @Synchronized fun updateRouting(remote: Boolean) {
        if (remote && !model.routed) {
            model.route(current())
            persist(true)
            manager.setStreamVolume(AudioManager.STREAM_MUSIC, 0, 0)
            openSystemVolume()
        } else if (!remote && model.routed) {
            session?.release()
            session = null
            provider = null
            manager.setStreamVolume(AudioManager.STREAM_MUSIC, model.restore(), 0)
            saved.edit().remove("restore_pending").apply()
        }
        val local = if (model.routed) null else current()
        gain = if (model.routed) model.gain else local!!.toFloat() / model.maximum
        publish(local)
    }

    @Synchronized fun handleKey(event: KeyEvent): Boolean {
        if (event.keyCode != KeyEvent.KEYCODE_VOLUME_UP && event.keyCode != KeyEvent.KEYCODE_VOLUME_DOWN && event.keyCode != KeyEvent.KEYCODE_VOLUME_MUTE) return false
        if (event.action == KeyEvent.ACTION_UP) return consumed.remove(event.keyCode)
        if (event.action != KeyEvent.ACTION_DOWN || !model.routed || manager.mode != AudioManager.MODE_NORMAL) return false
        consumed.add(event.keyCode)
        val controller = session?.controller ?: return false
        if (event.keyCode == KeyEvent.KEYCODE_VOLUME_MUTE) {
            if (event.repeatCount == 0) {
                model.toggleMute()
                controller.setVolumeTo(model.level, AudioManager.FLAG_SHOW_UI)
            }
        } else {
            val direction = if (event.keyCode == KeyEvent.KEYCODE_VOLUME_UP) AudioManager.ADJUST_RAISE else AudioManager.ADJUST_LOWER
            controller.adjustVolume(direction, AudioManager.FLAG_SHOW_UI)
        }
        return true
    }

    private fun openSystemVolume() {
        val remote = object : VolumeProvider(VOLUME_CONTROL_ABSOLUTE, model.maximum, model.level) {
            override fun onAdjustVolume(direction: Int) = changeVolume {
                when (direction) {
                    AudioManager.ADJUST_RAISE -> model.adjust(1)
                    AudioManager.ADJUST_LOWER -> model.adjust(-1)
                    AudioManager.ADJUST_TOGGLE_MUTE -> model.toggleMute()
                    AudioManager.ADJUST_MUTE -> model.setLevel(0)
                    AudioManager.ADJUST_UNMUTE -> if (model.level == 0) model.toggleMute()
                }
            }
            override fun onSetVolumeTo(volume: Int) = changeVolume { model.setLevel(volume) }
        }
        provider = remote
        session = MediaSession(context, "MineShare phone audio").apply {
            setCallback(object : MediaSession.Callback() {}, handler)
            setMetadata(MediaMetadata.Builder().putString(MediaMetadata.METADATA_KEY_TITLE, "Phone audio").build())
            setPlaybackToRemote(remote)
            setPlaybackState(PlaybackState.Builder().setState(PlaybackState.STATE_PLAYING, PlaybackState.PLAYBACK_POSITION_UNKNOWN, 1f).build())
            isActive = true
        }
    }

    @Synchronized private fun changeVolume(change: () -> Unit) {
        if (!model.routed) return
        change()
        gain = model.gain
        provider?.setCurrentVolume(model.level)
        persist()
        publish()
    }

    // The first recovery marker must reach disk before we mute the speaker.
    @android.annotation.SuppressLint("ApplySharedPref")
    private fun persist(sync: Boolean = false) {
        val edit = saved.edit().putBoolean("restore_pending", true).putInt("level", model.level)
        if (sync) edit.commit() else edit.apply()
    }

    private fun publish(local: Int? = null) {
        val level = if (model.routed) model.level else local ?: current()
        BridgeState.state.update {
            if (it.mediaVolume == level && it.mediaVolumeMax == model.maximum && it.remoteVolume == model.routed) it
            else it.copy(mediaVolume = level, mediaVolumeMax = model.maximum, remoteVolume = model.routed)
        }
    }

    fun close() { updateRouting(false) }
}
