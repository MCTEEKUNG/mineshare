package com.mineshare.mobile

import android.os.Build
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextOverflow

@Composable
fun PhoneDashboard(state: PhoneState, connect: () -> Unit, share: (PhonePeer) -> Unit,
    pause: () -> Unit, resume: () -> Unit = {}, stopAudio: () -> Unit, accessibility: () -> Unit, rotate: (Boolean?, Boolean) -> Unit,
    pendingAudioPeerId: String? = null, pendingAudioMessage: String = "รออนุญาตจาก Android…") {
    var tab by rememberSaveable { mutableIntStateOf(0) }
    var forget by remember { mutableStateOf<PhonePeer?>(null) }
    val deviceScroll = rememberLazyListState()
    val settingsScroll = rememberLazyListState()
    val helpScroll = rememberLazyListState()
    val connected = state.peers.count { it.connected }
    BoxWithConstraints(Modifier.fillMaxSize()) {
    val compactHeader = maxWidth >= 600.dp && maxHeight < 480.dp && LocalDensity.current.fontScale < 1.5f
    Scaffold(
        containerColor = androidx.compose.ui.graphics.Color.Transparent,
        contentWindowInsets = WindowInsets.safeDrawing,
        bottomBar = {
            Column(Modifier.glass(androidx.compose.foundation.shape.RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp), fill = 0.05f)
                .background(MaterialTheme.colorScheme.background.copy(alpha = 0.82f))) {
                if (state.running || connected > 0 || state.sharingAudio || state.peers.any { it.inputActive }) {
                    Surface(color = androidx.compose.ui.graphics.Color.Transparent) {
                        Row(Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal))
                            .horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp),
                            horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            if (state.peers.any { it.inputActive }) TextButton(
                                onClick = { BridgeState.command("release_input") },
                                modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "หยุดรับการควบคุมจากคอมพิวเตอร์" }) {
                                Text("หยุดควบคุม", maxLines = 1)
                            }
                            if (state.sharingAudio) TextButton(onClick = stopAudio,
                                modifier = Modifier.heightIn(min = 48.dp).semantics { contentDescription = "หยุดแชร์เสียงมือถือ" }) {
                                Text("หยุดเสียง", maxLines = 1)
                            }
                        }
                    }
                }
                NavigationBar(containerColor = androidx.compose.ui.graphics.Color.Transparent) {
                    listOf("อุปกรณ์", "ปรับแต่ง", "วิธีใช้").forEachIndexed { index, label ->
                        NavigationBarItem(selected = tab == index, onClick = { tab = index },
                            icon = { DashboardIcon(index) },
                            label = { Text(label) })
                    }
                }
            }
        }
    ) { padding ->
        // Each destination owns its lazy layout, while its remembered scroll state survives tab changes.
        // Lazy content is measured later. Capture the destination so a disposed
        // list cannot observe the next tab and clamp its remembered position.
        val destination = tab
        key(destination) {
        LazyColumn(Modifier.fillMaxSize().padding(padding),
            state = when (destination) { 0 -> deviceScroll; 1 -> settingsScroll; else -> helpScroll },
            contentPadding = PaddingValues(horizontal = 20.dp, vertical = if (compactHeader) 12.dp else 20.dp),
            verticalArrangement = Arrangement.spacedBy(if (compactHeader) 12.dp else 16.dp)) {
            item {
                val connectionLabel = if (connected > 0) "เชื่อมต่อ $connected เครื่อง" else if (state.running) "พร้อมจับคู่" else "ยังไม่เชื่อมต่อ"
                if (compactHeader) Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(16.dp),
                    verticalAlignment = Alignment.CenterVertically) {
                    Text("MineShare", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
                    Text(Build.MODEL, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(if (state.paused) "หยุดชั่วคราว" else connectionLabel, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                    if (state.paused) Button(onClick = resume) { Text("เชื่อมต่อต่อ") }
                    else if (state.running || connected > 0) OutlinedButton(onClick = pause) { Text("หยุดชั่วคราว") }
                } else {
                Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text("MineShare", style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.SemiBold)
                    Text(Build.MODEL, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(if (state.paused) "หยุดเชื่อมต่อชั่วคราว" else connectionLabel,
                        style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                    if (state.paused) Button(onClick = resume, modifier = Modifier.padding(top = 8.dp)) { Text("เชื่อมต่อต่อ") }
                    else if (state.running || connected > 0) OutlinedButton(onClick = pause, modifier = Modifier.padding(top = 8.dp)
                        .semantics { contentDescription = "หยุดสัญญาณชั่วคราว ตัดการเชื่อมต่อทุกเครื่อง" }) { Text("หยุดสัญญาณชั่วคราว") }
                }
                }
            }
            state.error?.let { message -> item {
                val errorText = connectionErrorText(message)
                var detailsOpen by rememberSaveable(message) { mutableStateOf(false) }
                DashboardPanel {
                    Text(errorText.title, style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.error)
                    Text(errorText.message, style = MaterialTheme.typography.bodyMedium)
                    errorText.details?.let { details ->
                        TextButton(onClick = { detailsOpen = !detailsOpen }, modifier = Modifier.semantics {
                            stateDescription = if (detailsOpen) "ขยายอยู่" else "ยุบอยู่"
                        }) { Text(if (detailsOpen) "ซ่อนรายละเอียด" else "ดูรายละเอียด") }
                        if (detailsOpen) Text(details, style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    TextButton(onClick = { BridgeState.state.value = BridgeState.state.value.copy(error = null) }) { Text("ปิดข้อความ") }
                }
            } }
            if (destination == 0) {
                // A connected computer cannot cross into the phone until control is on, so lead with it.
                if (!state.accessibility && connected > 0) item { ControlPermissionPanel(blocking = true, accessibility) }
                if (connected > 0) {
                    items(state.peers.filter { it.connected }, key = { it.id }) { peer ->
                        DevicePanel(peer, state, share, pendingAudioPeerId, pendingAudioMessage) { forget = it }
                    }
                    item {
                        Text("เลื่อนเมาส์ข้ามขอบจอตาม Layout เพื่อเข้ามือถือ", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                if (connected == 0) item {
                    DashboardPanel {
                        Text(if (connected > 0) "มือถือพร้อมทำงานกับคอมพิวเตอร์" else "ใช้เมาส์ คีย์บอร์ด และเสียงร่วมกัน",
                            style = MaterialTheme.typography.titleLarge)
                        Text(when {
                            connected > 0 -> "เลื่อนเมาส์ข้ามขอบจอ ตามตำแหน่งมือถือที่จัดไว้ใน Layout บนคอมพิวเตอร์"
                            state.running -> "เปิด MineShare บนคอมพิวเตอร์ใน Wi-Fi เดียวกัน แล้วเลือกเพิ่มอุปกรณ์"
                            else -> "เชื่อมต่อ Wi-Fi เดียวกับคอมพิวเตอร์ แล้วเริ่มเชื่อมต่อจากที่นี่"
                        }, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        if (!state.running && !state.paused) Button(onClick = connect, modifier = Modifier.fillMaxWidth()) { Text("เริ่มเชื่อมต่อผ่าน Wi-Fi") }
                    }
                }
                if (!state.accessibility && connected == 0) item { ControlPermissionPanel(blocking = false, accessibility) }
                if (state.peers.none { it.connected } || state.peers.any { !it.connected }) {
                    item { Text("อุปกรณ์ของคุณ", style = MaterialTheme.typography.titleLarge) }
                }
                if (state.peers.isEmpty()) item {
                    Text(if (state.running) "กำลังค้นหาอุปกรณ์ใกล้เคียง…\nเปิด MineShare บนคอมพิวเตอร์เพื่อเริ่มจับคู่"
                        else "อุปกรณ์ที่จับคู่และอุปกรณ์ใกล้เคียงจะแสดงตรงนี้ เมื่อเริ่มเชื่อมต่อ",
                        color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                items(state.peers.filterNot { it.connected }, key = { it.id }) { peer ->
                    DevicePanel(peer, state, share, pendingAudioPeerId, pendingAudioMessage) { forget = it }
                }
            } else if (destination == 1) {
                item { Text("ปรับให้ถนัดมือ", style = MaterialTheme.typography.headlineSmall) }
                item { DashboardPanel { PointerSettingsPanel() } }
                item {
                    DashboardPanel {
                        Text("แนวหน้าจอ", style = MaterialTheme.typography.titleMedium)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            OutlinedButton(onClick = { rotate(true, false) }) { Text("แนวตั้ง") }
                            OutlinedButton(onClick = { rotate(false, false) }) { Text("แนวนอน") }
                        }
                        TextButton(onClick = { rotate(null, true) }) { Text("หมุนอัตโนมัติ") }
                        Text("ครั้งแรกให้อนุญาตเปลี่ยนการตั้งค่าระบบ แล้วเลือกแนวจออีกครั้ง",
                            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                item {
                    DashboardPanel {
                        Text("การควบคุมและเสียง", style = MaterialTheme.typography.titleMedium)
                        Text(if (state.accessibility) "พร้อมรับเมาส์และคีย์บอร์ด" else "ยังไม่ได้เปิดสิทธิ์ควบคุม")
                        if (!state.accessibility) Text("เลือก “ควบคุมด้วย MineShare” ในหมวดแอปที่ติดตั้งหรือแอปที่ดาวน์โหลด เปิดใช้งาน แล้วกลับมาที่นี่",
                            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        OutlinedButton(onClick = accessibility) { Text("ตั้งค่าสิทธิ์ควบคุม") }
                        if (state.accessibility) TextButton(onClick = { MineShareAccessibility.instance?.openAppSearch() }) { Text("ค้นหาแอปบนมือถือ") }
                        Text(if (state.remoteVolume) "ระดับเสียง ${state.mediaVolume}/${state.mediaVolumeMax} · ออกทางคอมพิวเตอร์"
                            else "ใช้ปุ่มเสียงข้างมือถือได้ทั้งขณะแชร์และไม่แชร์เสียง", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            } else {
                item { Text("เริ่มใช้งาน MineShare", style = MaterialTheme.typography.headlineSmall) }
                item {
                    DashboardPanel {
                        Text("จับคู่ครั้งแรก", style = MaterialTheme.typography.titleMedium)
                        Text("1. เชื่อมต่อมือถือและคอมพิวเตอร์กับ Wi-Fi เดียวกัน\n2. เปิด MineShare ทั้งสองเครื่อง\n3. บนคอมพิวเตอร์ เลือกเพิ่มอุปกรณ์ แล้วเลือก ${Build.MODEL}\n4. ยอมรับบนมือถือ แล้วกรอกรหัสจับคู่บนเครื่องที่ร้องขอ")
                    }
                }
                item {
                    DashboardPanel {
                        Text("ย้ายเมาส์เข้ามือถือ", style = MaterialTheme.typography.titleMedium)
                        Text("จัดตำแหน่งมือถือใน Layout บนคอมพิวเตอร์ ถ้าวางมือถือด้านล่าง ให้เลื่อนเมาส์ลงผ่านขอบล่างของจอคอมพิวเตอร์")
                        Text("คลิกช่องข้อความก่อนพิมพ์ ส่วนคลิกขวาใช้ย้อนกลับ", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                item {
                    DashboardPanel {
                        Text("แชร์เสียงระหว่างสองเครื่อง", style = MaterialTheme.typography.titleMedium)
                        Text("มือถือ → คอมพิวเตอร์: เปิดสวิตช์ “เสียงมือถือ → คอมพิวเตอร์” ที่หน้าอุปกรณ์ อนุญาตบันทึกเสียง แล้วกดยืนยันในหน้าขอแชร์หน้าจอของ Android")
                        Text("Android ใช้หน้าขอแชร์หน้าจอเพื่ออนุญาตเข้าถึงเสียงสื่อ MineShare ส่งเฉพาะเสียงสื่อ ไม่ส่งภาพหน้าจอหรือเสียงไมโครโฟน",
                            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        Text("ใช้ปุ่มเสียงข้างมือถือปรับระดับเสียงที่ส่งไปคอมพิวเตอร์ได้ หากต้องการฟังเสียงคอมพิวเตอร์บนมือถือ ให้เปิดสวิตช์ “เสียงคอมพิวเตอร์ → มือถือ”")
                        Text("แชร์ได้จากสื่อและเกมที่อนุญาตให้บันทึกเสียง เสียงโทรและสื่อที่ป้องกันการบันทึกอาจแชร์ไม่ได้",
                            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                item {
                    DashboardPanel {
                        Text("คีย์ลัด เมื่อเมาส์อยู่บนมือถือ", style = MaterialTheme.typography.titleMedium)
                        listOf("Windows แล้วพิมพ์ชื่อแอป" to "ค้นหาแอป · Enter เพื่อเปิด", "Alt+Tab" to "สลับแอป", "Windows+D" to "หน้าหลัก", "Esc" to "ย้อนกลับ", "Ctrl+Alt+O" to "สลับแนวจอ", "Ctrl+Alt+Shift+O" to "หมุนจออัตโนมัติ", "Ctrl+Alt+Space" to "เรียกเคอร์เซอร์กลับกลางจอ").forEach { (key, action) ->
                            Column { Text(key, fontWeight = FontWeight.Medium); Text(action, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                        }
                    }
                }
            }
        }
        }
    }
    }
    forget?.let { peer ->
        AlertDialog(onDismissRequest = { forget = null }, title = { Text("ลบการจับคู่ ${peer.name}?") },
            text = { Text("การเชื่อมต่อกับเครื่องนี้จะสิ้นสุด ต้องจับคู่ใหม่ก่อนใช้อีกครั้ง") },
            confirmButton = { TextButton(onClick = { BridgeState.command("forget") { put("device_id", peer.id) }; forget = null }) { Text("ลบการจับคู่") } },
            dismissButton = { TextButton(onClick = { forget = null }) { Text("ยกเลิก") } })
    }
}

@Composable private fun DashboardPanel(content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.fillMaxWidth().glass().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp), content = content)
}

@Composable private fun ControlPermissionPanel(blocking: Boolean, accessibility: () -> Unit) {
    val explanation = (if (blocking) "ตอนนี้เมาส์จากคอมพิวเตอร์ข้ามมาที่มือถือไม่ได้ " else "") +
        "ในหน้าการช่วยเหลือการเข้าถึง เลือก “ควบคุมด้วย MineShare” เปิดใช้งาน แล้วกลับมาที่นี่ เพื่อใช้เมาส์คลิกและคีย์บอร์ดพิมพ์บนมือถือ หากไม่พบ ให้ดูหมวดแอปที่ติดตั้งหรือแอปที่ดาวน์โหลด"
    DashboardPanel {
        Text(if (blocking) "เปิดการควบคุมมือถือก่อนใช้เมาส์" else "เปิดการควบคุมมือถือ", style = MaterialTheme.typography.titleMedium,
            color = if (blocking) MaterialTheme.colorScheme.primary else androidx.compose.ui.graphics.Color.Unspecified)
        // When the warning blocks the mouse, the action sits right under the title so a shallow landscape screen never hides it.
        if (blocking) Button(onClick = accessibility) { Text("เปิดสิทธิ์ควบคุม") }
        Text(explanation, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (!blocking) OutlinedButton(onClick = accessibility) { Text("เปิดสิทธิ์ควบคุม") }
    }
}

@Composable private fun DashboardToggle(label: String, detail: String, checked: Boolean, enabled: Boolean = true, change: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).toggleable(value = checked, enabled = enabled, role = Role.Switch, onValueChange = change)
        .padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text(label, style = MaterialTheme.typography.bodyLarge)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(checked = checked, enabled = enabled, onCheckedChange = null)
    }
}

@Composable private fun DevicePanel(peer: PhonePeer, state: PhoneState, share: (PhonePeer) -> Unit, pendingAudioPeerId: String?, pendingAudioMessage: String, forget: (PhonePeer) -> Unit) {
                    DashboardPanel {
                        Text(peer.name, style = MaterialTheme.typography.titleMedium)
                        Text("${peer.os} · ${if (peer.connected) "เชื่อมต่อแล้ว" else if (peer.online) "อยู่ใกล้เคียง" else "ออฟไลน์"}",
                            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        if (!peer.compatible) Text("อัปเดต MineShare บนอุปกรณ์นี้เพื่อเชื่อมต่อ", color = MaterialTheme.colorScheme.error)
                        else if (!peer.connected) Button(enabled = peer.online,
                            onClick = { BridgeState.command("pair") { put("device_id", peer.id) } }) { Text(if (peer.paired) "เชื่อมต่อ" else "จับคู่อุปกรณ์") }
                        else {
                            BoxWithConstraints {
                                val receiveToggle: @Composable () -> Unit = {
                                    DashboardToggle("เสียงคอมพิวเตอร์ → มือถือ", "ฟังเสียงเครื่องนี้บนมือถือ", peer.receiveAudio) {
                                        BridgeState.command("audio_route") { put("device_id", peer.id).put("send", peer.sendAudio).put("receive", it) }
                                    }
                                }
                                val sendToggle: @Composable () -> Unit = {
                                    DashboardToggle("เสียงมือถือ → คอมพิวเตอร์", if (pendingAudioPeerId == peer.id) pendingAudioMessage else "ส่งเสียงสื่อและเกมไปเครื่องนี้",
                                        peer.sendAudio && state.sharingAudio, enabled = pendingAudioPeerId == null) { share(peer) }
                                }
                                if (maxWidth >= 640.dp && LocalDensity.current.fontScale < 1.5f) {
                                    Row(horizontalArrangement = Arrangement.spacedBy(24.dp)) {
                                        Column(Modifier.weight(1f)) { receiveToggle() }
                                        Column(Modifier.weight(1f)) { sendToggle() }
                                    }
                                } else Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                                    receiveToggle()
                                    sendToggle()
                                }
                            }
                            if (pendingAudioPeerId == peer.id) LinearProgressIndicator(Modifier.fillMaxWidth())
                            if (peer.inputActive) Text("กำลังควบคุมมือถือ", color = MaterialTheme.colorScheme.primary)
                        }
                        if (peer.paired) TextButton(onClick = { forget(peer) }) { Text("ลบการจับคู่") }
                    }
}

@Composable private fun DashboardIcon(index: Int) {
    val color = LocalContentColor.current
    Canvas(Modifier.size(24.dp)) {
        val unit = size.width / 24f
        val stroke = 2f * unit
        when (index) {
            0 -> {
                drawRect(color, Offset(3*unit, 4*unit), Size(18*unit, 13*unit), style = Stroke(stroke))
                drawLine(color, Offset(12*unit, 17*unit), Offset(12*unit, 21*unit), stroke)
                drawLine(color, Offset(7*unit, 21*unit), Offset(17*unit, 21*unit), stroke)
            }
            1 -> {
                drawLine(color, Offset(3*unit, 7*unit), Offset(21*unit, 7*unit), stroke)
                drawLine(color, Offset(3*unit, 17*unit), Offset(21*unit, 17*unit), stroke)
                drawCircle(color, 3*unit, Offset(8*unit, 7*unit), style = Stroke(stroke))
                drawCircle(color, 3*unit, Offset(16*unit, 17*unit), style = Stroke(stroke))
            }
            else -> {
                drawCircle(color, 9*unit, style = Stroke(stroke))
                drawCircle(color, unit, Offset(12*unit, 7*unit))
                drawLine(color, Offset(12*unit, 11*unit), Offset(12*unit, 18*unit), stroke)
            }
        }
    }
}
