package com.mineshare.mobile

/** Keep the existing 5 ms cadence during playback, back off only after silence. */
class AudioPollCadence {
    private var lastFrame = Long.MIN_VALUE
    private var idleDelay = 5L
    fun received(now: Long) { lastFrame = now; idleDelay = 5 }
    fun emptyDelay(now: Long): Long {
        if (lastFrame != Long.MIN_VALUE && now - lastFrame <= 500) return 5
        val result = idleDelay
        idleDelay = (idleDelay * 2).coerceAtMost(40)
        return result
    }
}
