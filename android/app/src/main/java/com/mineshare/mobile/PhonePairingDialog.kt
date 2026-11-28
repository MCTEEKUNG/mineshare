package com.mineshare.mobile

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay

@Composable
fun PhonePairingDialog(
    attempt: Long,
    deviceName: String,
    kind: String,
    displayedCode: String = "",
    error: String? = null,
    onConsent: () -> Unit,
    onSubmitCode: (String) -> Unit,
    onCancel: () -> Unit,
) {
    var pin by rememberSaveable(attempt, kind) { mutableStateOf("") }
    var submitted by rememberSaveable(attempt, kind) { mutableStateOf(false) }
    var waitingLong by rememberSaveable(attempt, kind) { mutableStateOf(false) }
    LaunchedEffect(attempt, kind, submitted) {
        if (submitted || kind !in setOf("consent", "enter_pin")) {
            delay(15_000)
            waitingLong = true
        }
    }
    fun submit() {
        if (!submitted && PairingCode.valid(pin)) { submitted = true; onSubmitCode(pin) }
    }
    AlertDialog(
        modifier = Modifier.imePadding(),
        onDismissRequest = onCancel,
        title = { Text(when (kind) {
            "consent" -> "จับคู่กับ $deviceName?"
            "show_pin" -> "รหัสสำหรับ $deviceName"
            "enter_pin" -> "กรอกรหัสจาก $deviceName"
            else -> "กำลังจับคู่กับ $deviceName"
        }) },
        text = {
            Column(Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(16.dp)) {
                when (kind) {
                    "consent" -> {
                        Text("ยอมรับเฉพาะอุปกรณ์ที่คุณต้องการเชื่อมต่อ")
                        Text("หลังจับคู่ เครื่องนี้จะใช้เมาส์ คีย์บอร์ด และเสียงร่วมกับมือถือได้ เมื่อเปิดใช้งานแต่ละฟีเจอร์",
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    "show_pin" -> {
                        Text("บน $deviceName ให้กรอกรหัส 6 หลักนี้เพื่อยืนยันการจับคู่")
                        if (PairingCode.valid(displayedCode)) SelectionContainer {
                            Text(displayedCode.chunked(3).joinToString(" "),
                                modifier = Modifier.semantics { contentDescription = "รหัสจับคู่ ${displayedCode.toCharArray().joinToString(" ")}" },
                                style = MaterialTheme.typography.displaySmall, fontWeight = FontWeight.SemiBold,
                                color = MaterialTheme.colorScheme.primary)
                        } else Text("รหัสยังไม่พร้อม ยกเลิกแล้วเริ่มจับคู่ใหม่", color = MaterialTheme.colorScheme.error)
                        Text("ไม่ต้องกรอกรหัสบนมือถือเครื่องนี้", style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    "enter_pin" -> {
                        Text("ดูรหัส 6 หลักบน $deviceName แล้วกรอกที่นี่")
                        OutlinedTextField(value = pin, onValueChange = { pin = PairingCode.filter(it) },
                            modifier = Modifier.fillMaxWidth(), enabled = !submitted,
                            label = { Text("รหัสจับคู่ 6 หลัก") }, singleLine = true,
                            supportingText = { Text("${pin.length}/6 · ใช้ตัวเลข 0–9") },
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword, imeAction = ImeAction.Done),
                            keyboardActions = KeyboardActions(onDone = { submit() }))
                    }
                    else -> Text("รอ $deviceName ยืนยันการจับคู่ เปิด MineShare บนเครื่องนั้นค้างไว้")
                }
                if (submitted) {
                    LinearProgressIndicator(Modifier.fillMaxWidth())
                    Text(if (kind == "consent") "ยอมรับแล้ว กำลังรอขั้นตอนยืนยันรหัส…" else "ส่งรหัสแล้ว กำลังรอยืนยัน…",
                        style = MaterialTheme.typography.bodyMedium)
                }
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                if (waitingLong) Text("ยังไม่ได้รับคำตอบ ตรวจสอบว่า $deviceName ยังเปิด MineShare และใช้ Wi-Fi เดียวกัน หากยังค้าง ให้ยกเลิกแล้วเริ่มจับคู่ใหม่",
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        },
        confirmButton = {
            when (kind) {
                "consent" -> TextButton(enabled = !submitted, onClick = { if (!submitted) { submitted = true; onConsent() } }) {
                    Text(if (submitted) "ยอมรับแล้ว" else "ยอมรับการจับคู่")
                }
                "enter_pin" -> TextButton(enabled = !submitted && PairingCode.valid(pin), onClick = ::submit) {
                    Text(if (submitted) "ส่งรหัสแล้ว" else "ยืนยันรหัส")
                }
            }
        },
        dismissButton = { TextButton(onClick = onCancel) { Text(if (waitingLong || error != null) "ยกเลิกแล้วลองใหม่" else "ยกเลิก") } },
    )
}
