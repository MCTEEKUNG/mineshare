package com.mineshare.mobile

/** One media level follows the user between local and remote playback. */
class SharedVolumeLevel(val maximum: Int, initial: Int) {
    init { require(maximum > 0) }
    var level = initial.coerceIn(0, maximum)
        private set
    var routed = false
        private set
    private var beforeMute = level.coerceAtLeast(1)
    val gain: Float get() = level.toFloat() / maximum

    fun route(localLevel: Int) { level = localLevel.coerceIn(0, maximum); routed = true }
    fun restore(): Int { routed = false; return level }
    fun adjust(direction: Int) { level = (level + direction).coerceIn(0, maximum) }
    fun setLevel(value: Int) { level = value.coerceIn(0, maximum) }
    fun toggleMute() {
        if (level > 0) { beforeMute = level; level = 0 }
        else level = beforeMute.coerceIn(1, maximum)
    }
}
