package com.mineshare.mobile

import java.util.Locale

data class ConnectionErrorText(val title: String, val message: String, val details: String? = null)

/** Keep user-facing Thai guidance intact and retain every original diagnostic for support. */
fun connectionErrorText(original: String): ConnectionErrorText {
    if (original.any { it in '\u0E00'..'\u0E7F' }) {
        return ConnectionErrorText("ตรวจสอบอีกครั้ง", original)
    }
    val error = original.lowercase(Locale.ROOT)
    val pairing = Regex("\\b(pairing|pair|pin)\\b").containsMatchIn(error)
    val expired = error.contains("expired") || error.contains("expiry") || error.contains("timed out") || error.contains("timeout")
    val (title, message) = when {
        pairing && expired -> "เริ่มจับคู่อีกครั้ง" to
            "ขั้นตอนจับคู่อาจหมดเวลาแล้ว ยกเลิกการจับคู่ที่ค้างอยู่ เปิด MineShare ทั้งสองเครื่อง แล้วเริ่มจับคู่ใหม่ด้วยรหัสใหม่"
        error.contains("connection refused") || error.contains("econnrefused") -> "ยังเชื่อมต่อไม่ได้" to
            "ตรวจสอบว่าเปิด MineShare บนคอมพิวเตอร์แล้ว และทั้งสองเครื่องใช้ Wi-Fi เดียวกัน จากนั้นลองเชื่อมต่ออีกครั้ง"
        error.contains("unreachable") || error.contains("no route to host") -> "ยังติดต่ออุปกรณ์ไม่ได้" to
            "ตรวจสอบการเชื่อมต่อ Wi-Fi ของทั้งสองเครื่อง เปิด MineShare บนเครื่องปลายทาง แล้วลองเชื่อมต่ออีกครั้ง"
        error.contains("offline") -> "อุปกรณ์ยังไม่พร้อมเชื่อมต่อ" to
            "เปิด MineShare บนเครื่องปลายทาง ตรวจสอบว่าเชื่อมต่อ Wi-Fi เดียวกัน แล้วรอให้อุปกรณ์แสดงว่าอยู่ใกล้เคียงก่อนลองอีกครั้ง"
        error.contains("timed out") || error.contains("timeout") -> "ยังไม่ได้รับคำตอบ" to
            "ตรวจสอบว่า MineShare ยังเปิดอยู่บนทั้งสองเครื่องและใช้ Wi-Fi เดียวกัน แล้วลองทำรายการอีกครั้ง"
        else -> "ยังทำรายการไม่สำเร็จ" to
            "ลองทำรายการอีกครั้ง หากยังไม่สำเร็จ ให้เปิดรายละเอียดเพื่อใช้ตรวจสอบปัญหา"
    }
    return ConnectionErrorText(title, message, original)
}
