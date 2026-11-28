package com.mineshare.mobile

/** The pairing protocol accepts six ASCII digits, never locale-specific digits. */
object PairingCode {
    fun filter(value: String) = value.filter { it in '0'..'9' }.take(6)
    fun valid(value: String) = value.length == 6 && value.all { it in '0'..'9' }
}
