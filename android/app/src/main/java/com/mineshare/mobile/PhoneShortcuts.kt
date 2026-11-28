package com.mineshare.mobile

enum class PhoneAction { RECENTS, SWITCH_APP, HOME, BACK, ROTATE, AUTO_ROTATE, RECOVER_POINTER }
data class ShortcutDecision(val consumed: Boolean, val action: PhoneAction? = null)

/** evdev scan codes on the authenticated input stream, before Android key mapping. */
class PhoneShortcuts {
    private val held = mutableSetOf<Int>()
    private val consumed = mutableSetOf<Int>()
    private var metaUsed = false

    fun key(scan: Int, down: Boolean): ShortcutDecision {
        val wasHeld = scan in held
        val meta = 125 in held || 126 in held
        val alt = 56 in held || 100 in held
        val ctrl = 29 in held || 97 in held
        val shift = 42 in held || 54 in held
        if (down) held.add(scan) else held.remove(scan)
        if (scan == 125 || scan == 126) {
            if (down && !meta) metaUsed = false
            val action = if (!down && wasHeld && !metaUsed && 125 !in held && 126 !in held) PhoneAction.RECENTS else null
            return ShortcutDecision(true, action)
        }
        if (!down && consumed.remove(scan)) return ShortcutDecision(true)
        if (!down) return ShortcutDecision(false)
        if (meta) metaUsed = true
        val action = when {
            scan == 24 && ctrl && alt -> if (shift) PhoneAction.AUTO_ROTATE else PhoneAction.ROTATE
            scan == 57 && ctrl && alt -> PhoneAction.RECOVER_POINTER
            scan == 15 && meta -> PhoneAction.RECENTS
            scan == 15 && alt -> PhoneAction.SWITCH_APP
            scan == 32 && meta -> PhoneAction.HOME
            scan == 1 -> PhoneAction.BACK
            else -> null
        }
        if (action != null) {
            consumed.add(scan)
            return ShortcutDecision(true, if (wasHeld) null else action)
        }
        return ShortcutDecision(false)
    }

    fun reset() { held.clear(); consumed.clear(); metaUsed = false }
}
