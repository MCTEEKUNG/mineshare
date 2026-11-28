package com.mineshare.mobile

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import kotlin.math.roundToInt

@Composable
fun PointerSettingsPanel() {
    val options by PointerPreferences.state.collectAsState()
    val previewHeight = if (LocalConfiguration.current.orientation == android.content.res.Configuration.ORIENTATION_LANDSCAPE) 64.dp else 120.dp
    var open by rememberSaveable { mutableStateOf(false) }
    var advanced by rememberSaveable { mutableStateOf(false) }
    var reset by rememberSaveable { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("เคอร์เซอร์และการเลื่อน", style = MaterialTheme.typography.titleMedium)
        Text("ขนาด ${(options.size * 100).roundToInt()}% · ปรับสี รูปแบบ และความลื่นได้",
            color = MaterialTheme.colorScheme.onSurfaceVariant)
        OutlinedButton(onClick = { open = true }) { Text("ปรับแต่งเมาส์บน Android") }
    }
    if (!open) return
    AlertDialog(onDismissRequest = { open = false },
        title = { Text("ปรับแต่งเมาส์") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                AndroidView(factory = { PointerArtwork(it).apply { preview = true } },
                    update = { it.options = options }, modifier = Modifier.fillMaxWidth().height(previewHeight)
                        .glass(MaterialTheme.shapes.medium))
                Text("รูปแบบเคอร์เซอร์", style = MaterialTheme.typography.labelLarge)
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf(PointerShape.ARROW to "ลูกศร", PointerShape.CIRCLE to "วงกลม", PointerShape.CROSSHAIR to "เป้า").forEach { (shape, label) ->
                        FilterChip(selected = options.shape == shape, onClick = { PointerPreferences.update(options.copy(shape = shape)) }, label = { Text(label) })
                    }
                }
                Text("สีเคอร์เซอร์", style = MaterialTheme.typography.labelLarge)
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    listOf(0xff4da3ff.toInt() to "ฟ้า", 0xfff5f1e8.toInt() to "ขาว", 0xff42d6df.toInt() to "เขียวน้ำทะเล",
                        0xffff80ba.toInt() to "ชมพู", 0xff0a0c0f.toInt() to "ดำ").forEach { (color, label) ->
                        Box(Modifier.size(48.dp).selectable(selected = options.color == color, role = Role.RadioButton,
                            onClick = { PointerPreferences.update(options.copy(color = color)) })
                            .border(if (options.color == color) 3.dp else 1.dp,
                            if (options.color == color) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline, CircleShape)
                            .padding(5.dp).background(Color(color), CircleShape)
                            .semantics { contentDescription = "สี$label" })
                    }
                }
                PointerSlider("ขนาดเคอร์เซอร์", "${(options.size * 100).roundToInt()}%", options.size, 0.75f..3f) {
                    PointerPreferences.update(options.copy(size = it))
                }
                PointerToggle("เส้นขอบ", options.outline) { PointerPreferences.update(options.copy(outline = it)) }
                PointerToggle("เงาเคอร์เซอร์", options.shadow) { PointerPreferences.update(options.copy(shadow = it)) }
                HorizontalDivider()
                TextButton(onClick = { advanced = !advanced }, modifier = Modifier.fillMaxWidth()
                    .semantics { stateDescription = if (advanced) "ขยายอยู่" else "ยุบอยู่" }) {
                    Text(if (advanced) "ซ่อนค่าการเลื่อนและการลาก ▴" else "ปรับการเลื่อนและการลาก ▾")
                }
                if (advanced) {
                PointerSlider("ความนุ่มของเคอร์เซอร์", "${(options.motionSmoothing * 100).roundToInt()}%", options.motionSmoothing, 0f..1f) {
                    PointerPreferences.update(options.copy(motionSmoothing = it))
                }
                Text("ซ้ายตอบสนองทันที · ขวาเคลื่อนที่นุ่มขึ้น", style = MaterialTheme.typography.bodySmall)
                PointerSlider("ความเร็วการเลื่อน", "${(options.scrollSpeed * 100).roundToInt()}%", options.scrollSpeed, 0.25f..3f) {
                    PointerPreferences.update(options.copy(scrollSpeed = it))
                }
                PointerSlider("ความลื่นของการเลื่อน", "${(options.scrollSmoothing * 100).roundToInt()}%", options.scrollSmoothing, 0f..1f) {
                    PointerPreferences.update(options.copy(scrollSmoothing = it))
                }
                PointerSlider("ความลื่นของการลาก", "${(options.dragSmoothing * 100).roundToInt()}%", options.dragSmoothing, 0f..1f) {
                    PointerPreferences.update(options.copy(dragSmoothing = it))
                }
                Text("ความลื่นสูงทำให้การเคลื่อนไหวนุ่มขึ้น เลือกค่าต่ำเมื่อต้องการตอบสนองไว",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Text("ใช้ได้ทันทีและบันทึกให้อัตโนมัติ", style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        },
        confirmButton = { TextButton(onClick = { open = false }) { Text("เสร็จ") } },
        dismissButton = { TextButton(onClick = { reset = true }) { Text("คืนค่าเริ่มต้น") } })
    if (reset) AlertDialog(onDismissRequest = { reset = false },
        title = { Text("คืนค่าเมาส์เริ่มต้น?") },
        text = { Text("ขนาด สี รูปแบบ และค่าการเลื่อนทั้งหมดจะกลับเป็นค่าเริ่มต้น") },
        confirmButton = { TextButton(onClick = { PointerPreferences.update(PointerOptions()); reset = false }) { Text("คืนค่าทั้งหมด") } },
        dismissButton = { TextButton(onClick = { reset = false }) { Text("เก็บค่าเดิม") } })
}

@Composable private fun PointerSlider(label: String, valueLabel: String, value: Float, range: ClosedFloatingPointRange<Float>, change: (Float) -> Unit) {
    Column {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            Text(label, Modifier.weight(1f), style = MaterialTheme.typography.labelLarge)
            Text(valueLabel, color = MaterialTheme.colorScheme.primary)
        }
        Slider(value = value, onValueChange = change, valueRange = range,
            modifier = Modifier.semantics { contentDescription = label; stateDescription = valueLabel })
    }
}

@Composable private fun PointerToggle(label: String, value: Boolean, change: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).toggleable(value = value, role = Role.Switch, onValueChange = change),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        Text(label, Modifier.weight(1f))
        Switch(checked = value, onCheckedChange = null)
    }
}
