# MineShare — Plan for Developer

> KVM-over-IP + Bidirectional Audio/Mic Bridge สำหรับ Windows ↔ Ubuntu Linux
> เวอร์ชันแผน: **v0.3** (final pre-implementation; ทุกข้อตัดสินใจ confirm แล้ว)

---

## 0. Changelog

| Ver | สิ่งที่เปลี่ยน |
|---|---|
| v0.1 | initial plan |
| v0.2 | + microphone forward, + FPS gaming budget, + Wayland evdev/uinput path, + multi-monitor, rename → MineShare |
| **v0.3** | Topology = **Auto-detect hardware-source** (Model B), drop EV cert/code-signing, drop custom virtual mic driver (Plan A only), lock Q3/Q6/Q7/Q8 defaults |

---

## 1. เป้าหมาย (Goal)

ซอฟต์แวร์เดสก์ท็อป **MineShare** ที่ทำให้พีซี Windows + Ubuntu (แต่ละเครื่องอาจมีหลายจอ) ทำงานเหมือนต่อเป็น virtual desktop เดียว โดย user มี mouse/keyboard/mic/headset แค่ชุดเดียวก็ใช้งานได้ทั้งสอง PC:

1. **Mouse seamless ระดับ FPS** — cursor ไหลข้ามจอ + cross-PC, latency < 10 ms
2. **Keyboard follows cursor** — keystroke ไปยัง PC ที่ cursor อยู่
3. **Auto hardware-source detection** — ตรวจเองว่าฮาร์ดแวร์เสียบอยู่ PC ไหนตอนนี้ ไม่ต้องตั้งค่าใหม่ตอนย้าย
4. **Pointer-lock pass-through** — เกมจับเมาส์ → ส่ง relative delta ตรง
5. **Clipboard sync** (text เริ่มก่อน)
6. **Bidirectional system audio bridge** — ได้ยินเสียง output ของอีกเครื่อง
7. **Bidirectional microphone forward** — mic ของ PC ที่หูฟังเสียบอยู่ → เป็น input ของแอปบนอีก PC
8. **GUI ตั้งค่าจอ multi-monitor** สไตล์ Windows Display Settings
9. **Zero-config pairing** — เปิด 2 เครื่องบน LAN → discover → PIN ครั้งเดียว → ใช้ได้
10. **Cross-platform** — Windows 11 + Ubuntu 22.04/24.04 (X11 + Wayland)

**Non-goals (v1):**
- ไม่ทำ macOS
- LAN-only (ไม่มี NAT traversal/relay)
- ไม่ทำ video/screen mirror
- ไม่รับประกันใช้งานได้กับเกม kernel-anti-cheat (Vanguard/EAC) — มี warning
- ไม่ทำ file transfer
- **ไม่เซ็น code-signing cert** (hobby project, ผ่าน SmartScreen ด้วยมือ)

---

## 2. ข้อสรุปการตัดสินใจ (ล็อคแล้ว)

| ID | หัวข้อ | ข้อสรุป |
|---|---|---|
| Q1 | Topology | **Model B — Auto-detect hardware-source** (ดู §5.1) |
| Q2 | Audio | Bidirectional sysout + bidirectional mic forward |
| Q3 | Network | LAN-only (v2 พิจารณา internet) |
| Q4 | Latency budget | < 10 ms input (FPS-grade), < 60 ms sysout, < 40 ms mic |
| Q5 | Wayland | Fast path: evdev + uinput + group `input`. Fallback: libei via portal |
| Q6 | GUI | Tauri 2 + React + TS + Tailwind + shadcn/ui |
| Q7 | Pairing | PIN 6 หลัก + Noise XX |
| Q8 | License | MIT (เปลี่ยนได้ภายหลัง) |
| Q9 | ชื่อ | MineShare |
| Q10 | Multi-monitor | รองรับเต็ม — unified canvas |
| Q11 | Code signing | ❌ ไม่เซ็น (hobby, SmartScreen warning OK) |
| Q12 | Virtual mic Win | ✅ Plan A only — bundle workflow VB-CABLE (donationware free for personal use) |

---

## 3. สถาปัตยกรรมระบบ (High-level)

```
+-------------------- PC-A (Windows, 2 monitors) --------------------+      +------------------ PC-B (Ubuntu, 1 monitor) ---------------------+
|                                                                    |      |                                                                  |
|  +---------+    +---------------------+    +-------------------+   |      |   +-------------------+   +---------------------+   +---------+ |
|  |  GUI    |<-->|     Core Daemon     |<-->|  Input Capture    |<--|------|-->|  Input Capture    |<->|     Core Daemon     |<->|  GUI    | |
|  | (Tauri) |    |  - Layout (multi-   |    |  Raw Input        |   |      |   |  evdev            |   |                     |   | (Tauri) | |
|  +---------+    |    monitor canvas)  |    +-------------------+   |      |   +-------------------+   |                     |   +---------+ |
|                 |  - Source FSM       |    +-------------------+   |      |   +-------------------+   |                     |               |
|                 |    (auto-detect)    |    |  Input Inject     |   |      |   |  Input Inject     |   |                     |               |
|                 |  - Routing FSM      |    |  SendInput        |   |      |   |  uinput           |   |                     |               |
|                 |  - Clipboard sync   |    +-------------------+   |      |   +-------------------+   |                     |               |
|                 |                     |    +-------------------+   |      |   +-------------------+   |                     |               |
|                 |                     |<-->| SysOut + Mic      |<--|------|-->| SysOut + Mic      |<->|                     |               |
|                 |                     |    | Capture (WASAPI)  |   |      |   | Capture (PipeWire)|   |                     |               |
|                 |                     |    +-------------------+   |      |   +-------------------+   |                     |               |
|                 |                     |    +-------------------+   |      |   +-------------------+   |                     |               |
|                 |                     |<-->| Playback +        |   |      |   | Playback +        |<->|                     |               |
|                 |                     |    | Virtual Mic       |   |      |   | Virtual Mic       |   |                     |               |
|                 |                     |    | (VB-CABLE on Win) |   |      |   | (PipeWire null)   |   |                     |               |
|                 +----------+----------+    +-------------------+   |      |   +-------------------+   +----------+----------+               |
|                            |                                       |      |                                      |                          |
+----------------------------+---------------------------------------+      +--------------------------------------+--------------------------+
                             |                                                                                     |
                             |  TCP+TLS (control) / UDP+ChaCha20-Poly1305 (input/audio/mic, prio'd)                 |
                             +-------------------------------------------------------------------------------------+
                                                              LAN (mDNS _mineshare._tcp)
```

**สำคัญ:** ทั้งสอง PC มี input capture **เปิดทำงาน** ตลอด — แต่จะ "อ่าน + ส่งต่อ" เฉพาะตอนที่ตัวเองเป็น `hardware-source` (ดู §5.1)

---

## 4. Technology Stack

| Layer | เลือก | เหตุผล |
|---|---|---|
| Language | **Rust** edition 2024 | safety + low-level + ecosystem |
| GUI | **Tauri 2** + React + TS + Tailwind + shadcn/ui | binary เล็ก, ทำธีม Windows Settings ง่าย |
| IPC GUI↔Daemon | Tauri command + Unix socket / named pipe | one-process M0–M3, แยก service ใน M4 |
| Discovery | `mdns-sd` (`_mineshare._tcp.local`) | zero-config |
| Pairing | Noise XX (`snow`) + PIN 6 หลัก | mutual auth + OOB confirm |
| Transport | TCP+rustls (control) + UDP+ChaCha20-Poly1305 (data) | reliable state + low-latency stream |
| Input capture Win | **Raw Input** (`WM_INPUT`) + low-level hook + `ClipCursor` | 1000Hz, raw deltas |
| Input inject Win | **SendInput** (relative `MOUSEEVENTF_MOVE`) | เกมส่วนใหญ่ยอมรับ |
| Input capture Linux | **evdev** ตรงจาก `/dev/input/event*` (group `input`) + `EVIOCGRAB` | bypass display server, latency ต่ำสุด, ทั้ง X11/Wayland |
| Input inject Linux | **uinput** virtual device | ระบบมองเป็น HID จริง |
| Wayland fallback | **libei** ผ่าน `ashpd` + RemoteDesktop portal | กรณี user ไม่ตั้ง group `input` |
| Sys audio capture Win | **WASAPI loopback** | API มาตรฐาน |
| Sys audio capture Linux | **PipeWire monitor** ผ่าน `pipewire-rs` (PA fallback) | default Ubuntu |
| Mic capture Win | WASAPI capture default endpoint | |
| Mic capture Linux | PipeWire default source | |
| Playback | `cpal` ทั้งสอง OS | API เดียว |
| Codec | **Opus** — 20 ms (sysout), 10 ms (mic low-latency) | เกรดเสียงดี + bitrate flexible |
| **Virtual mic Win** | **bundle workflow VB-CABLE** (user install ครั้งเดียว) | ไม่มี driver dev cost |
| Virtual mic Linux | PipeWire null-sink + virtual source | ตรงไปตรงมา |
| Logging | `tracing` | structured |
| CI | GitHub Actions (windows-latest + ubuntu-22.04) | |

---

## 5. Component Breakdown

Cargo workspace:

```
mineshare/                          # workspace
├── crates/
│   ├── mineshare-core/             # layout, FSMs, clipboard
│   ├── mineshare-net/              # protocol, discovery, pairing, transport
│   ├── mineshare-input/            # capture + inject + source detect
│   ├── mineshare-audio/            # sysout + mic + virtual sink
│   ├── mineshare-ipc/              # GUI ↔ daemon
│   └── mineshare-daemon/           # binary
├── ui/                             # Tauri app
├── installer/{windows,linux}
└── docs/
```

### 5.1 `mineshare-core` — Auto-detect Hardware Source (Model B)

**แนวคิดหลัก:**
- ทุก daemon **เปิด input capture แบบ passive** ตลอดเวลา (อ่าน raw event แต่ยังไม่ส่งต่อ)
- เมื่อตรวจพบ "real local HW input" → claim บทบาท `hardware-source`
- daemon ที่เป็น source = ตัวอ่าน input ปัจจุบัน → forward ไปยัง active screen ตาม layout

**Data model:**
```rust
struct DeviceId(Uuid);
struct MonitorId { device: DeviceId, index: u32 }

struct UnifiedLayout {
    monitors: Vec<PlacedMonitor>,    // ทุกจอของทุก PC ใน canvas เดียว
}

enum SourceState {
    LocalSource,                     // เครื่องนี้มีฮาร์ดแวร์ active
    RemoteSource(DeviceId),          // อีกเครื่องเป็น source
    Idle,                            // ยังไม่มีใคร active
}

enum CursorState {
    OnLocalMonitor(MonitorId),
    OnRemoteMonitor(MonitorId),      // (อยู่บนจอของอีกเครื่อง)
}
```

**Source FSM (ทำงานบนทุก daemon):**
```
                          local HW event (และ idle > 300ms)
                      ┌────────────────────────────────────┐
                      ↓                                    │
   [Idle] ────local HW──→ [LocalSource] ←──no input 2s──── │
     ↑                          │                          │
     │                          │ peer claims primary      │
     │                          ↓                          │
     │                    [RemoteSource]                   │
     │                          │                          │
     └──peer disconnects────────┘                          │
                                                           │
   [LocalSource] ──ignored peer claim──────────────────────┘
                   (because local just had input)
```

**Tie-break เมื่อทั้งสอง PC อ้าง source พร้อมกัน:**
- เปรียบ timestamp ของ event แรก → คนเร็วกว่าชนะ
- ถ้าเท่ากัน → DeviceId ที่ hash ต่ำกว่าชนะ (deterministic)
- 200 ms hysteresis กันสลับไปมาเร็วเกินไป

**Routing FSM (จัดการ cursor):**
```
[OnLocalMonitor] --cursor hits bridge edge → handover ack--> [OnRemoteMonitor]
[OnRemoteMonitor] --cursor hits bridge edge--> [OnLocalMonitor]
[OnRemoteMonitor] --hotkey "return home" / heartbeat 1.5s timeout--> [OnLocalMonitor]
```

**ขณะ `OnRemoteMonitor`:**
- ถ้าตัวเองเป็น `LocalSource` → `ClipCursor` lock + hide local cursor + ส่ง `dx, dy` ไป peer
- ถ้าตัวเองเป็น `RemoteSource` → inject relative move เข้า OS local

**Edge handling:**
- ขอบจอ "ในเครื่องเดียวกัน" → ไม่ intercept (OS จัดการ)
- ขอบจอ "ข้ามเครื่อง" → derive จาก `UnifiedLayout` เป็น `Vec<BridgeEdge>` ตอน layout เปลี่ยน

### 5.2 `mineshare-net`

(ดู §6)

### 5.3 `mineshare-input`

**Trait:**
```rust
trait InputCapture {
    fn start(&mut self, sink: EventSink) -> Result<()>;
    fn set_grab(&mut self, grab: bool);    // ขณะ source + cursor on remote
}

trait InputInject {
    fn mouse_move_rel(&self, dx: i32, dy: i32);
    fn mouse_button(&self, btn: Button, down: bool);
    fn key(&self, scan: ScanCode, down: bool);
    fn scroll(&self, dx: f32, dy: f32);
}
```

**Windows:**
- Raw Input (`WM_INPUT`) ใน hidden message-only window thread
- `RIDEV_INPUTSINK` รับแม้ window ไม่ focus
- block local: `ClipCursor` + `WH_MOUSE_LL` hook
- inject: `SendInput` flag `MOUSEEVENTF_MOVE`

**Linux evdev (fast path):**
- enumerate `/dev/input/event*` filter เอา device ที่เป็น mouse/keyboard
- อ่าน `EV_REL`, `EV_KEY`, `EV_ABS` raw
- block local: `EVIOCGRAB` ขณะ cursor on remote

**Linux libei (fallback):**
- `ashpd` → request RemoteDesktop session → portal prompt user
- ใช้กรณี evdev permission ไม่ผ่าน

**Linux uinput inject:**
- สร้าง virtual mouse + virtual keyboard
- ส่ง EV_REL / EV_KEY events
- ทุกเกมยอมรับ (ยกเว้น kernel anti-cheat)

**FPS pointer-lock relay:**
- ปลายทาง detect ClipCursor (Win) / pointer-constraints (Wayland) → ส่ง `PointerLocked { center }` กลับไปยัง source
- source: lock cursor ที่จุดเดียว, ส่ง raw delta ตรงไปปลายทาง (ไม่ทำ edge detection ใน mode นี้)

**Latency budget:**
```
mouse HID poll       : 1 ms (1000Hz)
read (Raw/evdev)     : <0.5 ms
encrypt + UDP send   : <1 ms
LAN RTT/2            : 1-3 ms (Ethernet) | 3-8 ms (Wi-Fi)
recv + decrypt       : <1 ms
inject (uinput/Send) : <1 ms
─────────────────────────────────────
total one-way        : 4.5-7.5 ms (Ethernet) ✓
                     : 7.5-13 ms (Wi-Fi)  marginal
```

→ **Ethernet จำเป็นสำหรับ FPS** — UI แสดง latency live + warn เมื่อ >10 ms

### 5.4 `mineshare-audio` — 4-Stream Bridge

| Stream | Source | Sink | Codec | Frame |
|---|---|---|---|---|
| A→B sysout | A: WASAPI loopback (default render) | B: cpal default playback (mix) | Opus 64–128 kbps | 20 ms |
| B→A sysout | B: PipeWire monitor (default sink) | A: cpal default playback (mix) | Opus 64–128 kbps | 20 ms |
| A→B mic | A: WASAPI default capture | B: virtual mic source | Opus 32–48 kbps | 10 ms |
| B→A mic | B: PipeWire default source | A: virtual mic source | Opus 32–48 kbps | 10 ms |

**Virtual mic implementation:**

*Linux:* PipeWire null-sink + virtual source สร้างตอน daemon start → ปรากฏเป็น `MineShare Virtual Mic` ใน Discord/OBS

*Windows:* bundle workflow ติดตั้ง **VB-CABLE**:
- ตอน install หรือ first-run, GUI prompt: "MineShare ต้องการ VB-CABLE สำหรับ mic forwarding [ดาวน์โหลด]"
- เปิด browser ไปที่ vb-audio.com/Cable
- หลัง user install เอง → MineShare detect device "CABLE Input" → เลือกอัตโนมัติ
- user เลือก "CABLE Output" เป็น mic ใน Discord/OBS

**Echo loop guard:**
- ทุก audio frame ติด `source_id` (DeviceId hash)
- capture filter discard frame ที่ source = ตัวเอง
- ทาง Windows สร้าง dedicated MineShare playback session แยก เพื่อ loopback จับเฉพาะของตัวเอง

**Mic optimization:**
- Opus inband-FEC + DTX
- Adaptive jitter buffer 20–60 ms
- ไม่มี echo cancellation ใน v1 (สมมติ user ใช้หูฟัง)

### 5.5 `mineshare-daemon`

- Windows: service ผ่าน `windows-service`
- Linux: systemd **user** unit (ไม่ใช่ system unit — ไม่ต้อง root)
- supervisor pattern + auto-restart

### 5.6 `ui/`

(ดู §7)

---

## 6. Network Protocol

**Frame format:**
```
[u8 ver=1][u8 channel][u16 type][u32 seq][u64 ts_ns][u32 len][payload]
```

**Channels:**
| # | Transport | Use |
|---|---|---|
| 0 control | TCP+TLS | hello, layout, clipboard, source claim, ping |
| 1 input | UDP+AEAD | mouse/key (batched up to 4/packet) |
| 2 audio-sysout | UDP+AEAD | sysout Opus |
| 3 audio-mic | UDP+AEAD | mic Opus |

**Control messages:**
- `Hello { device, os, monitors[], pubkey }`
- `LayoutSync { unified_layout }`
- `ClipboardUpdate { mime, data }`
- `ClaimSource { ts_ns, hw_event_summary }` ← Model B
- `ReleaseSource {}`
- `EnableInput { enter_at, target_monitor }` (active screen change)
- `DisableInput {}`
- `PointerLocked { active, center }`
- `Ping / Pong`
- `Heartbeat` (ทุก 500 ms)

**Input messages (UDP):**
- `MouseMove { dx, dy }`
- `MouseButton { btn, down }`
- `Key { scancode, down, mods }`
- `Scroll { dx, dy }`

**Audio messages:**
- `OpusFrame { stream_id, source_id, opus_bytes }`
- `Silence { duration_ms }` (DTX)

**Crypto:**
- Noise XX → derive `Tx_key`, `Rx_key` per direction → ChaCha20-Poly1305
- replay window 256 packets/channel
- rekey ทุก 30 นาที

---

## 7. GUI — Multi-monitor Layout

### Tab "Layout" (หลัก)
```
+---------------------------------------------------------------+
|  MineShare                                ● Connected · 4 ms  |
+---------------------------------------------------------------+
|  Drag your displays to match physical arrangement              |
|                                                                |
|     +-------+ +-------+         +-------+                      |
|     | PC-A  | | PC-A  |         | PC-B  |                      |
|     | Mon 1 | | Mon 2 |         | Mon 1 |                      |
|     | 1920× | | 2560× |         | 2560× |                      |
|     |  1080 | |  1440 |         |  1440 |                      |
|     +-------+ +-------+         +-------+                      |
|     ●hardware src                                              |
|                                                                |
|  Bridge: PC-A/Mon2 ⇄ PC-B/Mon1 (right edge)                    |
|                                                                |
|  [Identify displays] [Reset] [Apply]                           |
+---------------------------------------------------------------+
```

- Canvas เดียว ลากจอจัด snap-to-edge
- จอจาก PC ต่างกัน border สีต่างกัน
- "● hardware src" = badge แสดง PC ที่กำลังเป็น source
- "Identify displays" overlay เลขใหญ่บนจอจริง 2 วิ
- snap จอข้าม PC → mark `BridgeEdge` อัตโนมัติ

### Tabs อื่น
- **Devices**: peer ที่ pair, online + latency, unpair
- **Audio**: 4 stream toggle + device picker + volume + level meter + ปุ่ม "Install VB-CABLE" (Win) ถ้ายังไม่มี
- **Hotkeys**: 
  - "Force return cursor" (`Ctrl+Alt+Home`)
  - "Toggle bridge"
- **Advanced**: ports, MTU, Opus bitrate, Wayland mode (auto/evdev/libei), log level
- **Pair new device**: list mDNS + PIN flow

### Style
- light/dark ตาม OS
- typography: Segoe UI Variable (Win), Inter (Linux)
- shadcn/ui Card / Slider / Switch
- ไม่มี animation เกินจำเป็น

---

## 8. ความปลอดภัย

| Risk | Mitigation |
|---|---|
| Inject ปลอมจาก LAN | Noise XX mutual auth + per-session key |
| Replay | sequence + ts window |
| Eavesdrop | ChaCha20-Poly1305 ทุก packet |
| IPC abuse | Unix socket user-only / named pipe + DACL |
| Daemon ต้อง elevated | ❌ — Linux ใช้ group `input` (ติดตั้งโดย postinst) |
| Clipboard leak | sync เฉพาะที่ user toggle ON; v2 พิจารณา exclude password manager |

---

## 9. ข้อจำกัดที่ต้องสื่อสารกับผู้ใช้

### 9.1 Wayland Permission

- **Fast path**: postinst script เพิ่ม user เข้า group `input` + udev rule `/dev/uinput` → no prompt
- **Fallback**: libei portal → user กด Allow + ติ๊ก "Always allow"
- Auto-detect ใน Advanced tab

### 9.2 Latency Wi-Fi

- Wi-Fi variance สูง 3–15 ms → FPS ทำได้แต่ stutter
- UI โชว์ latency live + warning >10 ms
- docs แนะนำ Ethernet

### 9.3 Multi-monitor edge cases

- DPI scale ต่างกัน → relative motion scale ตาม ratio ที่ inject side
- Mixed refresh rate: เฟส 2

### 9.4 Virtual Mic บน Windows

- v1: bundle workflow VB-CABLE (donationware ฟรี personal)
- ไม่ build driver เอง (out of scope)

### 9.5 Anti-Cheat

⚠ **เกมที่ใช้ kernel-level anti-cheat อาจ block หรือ ban**:

| Game / AC | Status |
|---|---|
| Vanguard (Valorant) | ❌ block virtual HID |
| EAC (Apex/Fortnite) | ⚠ บางครั้ง flag SendInput |
| BattlEye (PUBG/R6) | ✅ ส่วนใหญ่ผ่าน |
| VAC (CS2) | ✅ ผ่าน |

- Mitigation: warning dialog ครั้งแรก + per-game compat list ใน docs

### 9.6 Audio Feedback Loop

- source_id tag + dedicated MineShare audio session
- capture filter discard self-source

### 9.7 Code-signing (Windows)

- ❌ ไม่เซ็น (hobby project, no budget)
- User เจอ SmartScreen "Windows protected your PC" ครั้งแรก → คลิก "More info → Run anyway"
- docs จะมี screenshot + คำอธิบาย
- ทางเลือก: self-sign ด้วย `New-SelfSignedCertificate` + import เป็น Trusted Publisher (ทำเฉพาะเครื่องตัวเอง)

---

## 10. Phased Roadmap

ประมาณการคนเดียว เต็มเวลา (ลด scope จาก v0.2 เพราะตัด driver dev + cert):

### M0 — Spike & Skeleton (1.5 สัปดาห์)
- workspace + CI ทั้ง Win/Linux
- Tauri shell แสดงหน้าว่าง + IPC dummy
- mDNS discover + แสดง peer list
- Noise XX handshake (ยังไม่มี PIN UI)
- ✅ exit: 2 เครื่องเห็นกันใน UI + handshake สำเร็จ

### M1 — Mouse + Keyboard MVP (4 สัปดาห์)
- Raw Input (Win) + evdev (Linux) capture
- SendInput (Win) + uinput (Linux) inject
- UDP + ChaCha20 input channel
- Edge detection (1 จอ/เครื่อง, fixed layout B-ขวา-A)
- ClipCursor + EVIOCGRAB local block
- pointer-lock relay (FPS mode)
- latency overlay (debug)
- ✅ exit: เลื่อนเมาส์/พิมพ์ข้ามได้, latency <10 ms LAN, ลอง CS2 ผ่าน

### M2 — Auto Source-detect + Pairing + Multi-monitor (3.5 สัปดาห์)
- **Source FSM** (Model B) + tie-break + hysteresis
- PIN pairing UI
- Persist config (key + layout)
- Layout editor: drag, snap, identify
- Multi-monitor enumeration ทั้งสอง OS
- Bridge-edge derivation
- DPI scaling per monitor
- ✅ exit: ย้ายเมาส์เสียบเครื่องอื่น → switch source อัตโนมัติ

### M3 — Audio Bridge: SysOut + Mic Forward (3.5 สัปดาห์)
- WASAPI loopback + PipeWire monitor (sysout)
- WASAPI capture + PipeWire source (mic)
- Opus encode/decode + jitter buffer + DTX
- Linux PipeWire null-sink + virtual source
- Windows VB-CABLE detection + install workflow + GUI prompt
- Audio settings tab
- Echo loop guard
- ✅ exit: ได้ยินเสียง YouTube ของอีกเครื่อง + mic A เข้า Discord บน B

### M4 — Wayland + Service + Installer (2.5 สัปดาห์)
- libei integration (fallback)
- udev rule + group `input` postinst (.deb)
- Windows service / systemd user unit
- Auto-start on login
- MSI (WiX, **unsigned**) + .deb
- Hotkey + clipboard text sync
- Reconnect logic + offline graceful
- ✅ exit: ลง installer ใช้งาน end-to-end ไม่ต้องเปิด terminal

### M5 — Polish (1.5–2 สัปดาห์)
- Anti-cheat warning dialog + per-game DB
- Self-sign installer (ตัวเอง, optional)
- Crash log local file
- Telemetry latency histogram (local-only)
- Localization TH+EN

**รวม ≈ 16 สัปดาห์** (~4 เดือน)

---

## 11. โครงสร้างโปรเจกต์

```
C:\Users\ASUS\Barrier01\
├── PLAN.md
├── README.md
├── LICENSE                            # MIT
├── Cargo.toml                         # workspace
├── .github/workflows/ci.yml
├── crates/
│   ├── mineshare-core/
│   │   └── src/{lib,layout,edge,source_fsm,routing,clipboard}.rs
│   ├── mineshare-net/
│   │   └── src/{lib,discovery,pairing,proto,transport,replay}.rs
│   ├── mineshare-input/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── windows/{capture,inject}.rs
│   │       ├── linux_evdev/{capture,inject}.rs
│   │       └── linux_libei/capture.rs
│   ├── mineshare-audio/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── capture/{wasapi_loopback,wasapi_mic,pipewire_monitor,pipewire_mic}.rs
│   │       ├── playback.rs
│   │       ├── codec.rs
│   │       ├── jitter.rs
│   │       └── virtual_mic/{linux_pw,windows_vbcable}.rs
│   ├── mineshare-ipc/
│   │   └── src/{server,client}.rs
│   └── mineshare-daemon/
│       └── src/{main,service_win,service_linux,supervisor}.rs
├── ui/
│   ├── src-tauri/{src/main.rs, tauri.conf.json}
│   └── src/
│       ├── pages/{Layout,Devices,Audio,Hotkeys,Advanced,Pair}.tsx
│       ├── components/{MonitorTile,LatencyPill,EdgeBridge,SourceBadge,...}.tsx
│       ├── hooks/
│       └── App.tsx
├── installer/
│   ├── windows/                       # WiX (unsigned)
│   └── linux/                         # debian/, postinst, udev rules
└── docs/
    ├── architecture.md
    ├── protocol.md
    ├── anti-cheat-compatibility.md
    ├── wayland-setup.md
    ├── virtual-mic-windows.md         # VB-CABLE guide
    └── smartscreen-warning.md         # ขั้นตอน Run anyway
```

---

## 12. Risks (อัปเดต)

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Anti-cheat ban | สูง | สูง (ต่อ user) | warning dialog + compat list |
| FPS Wi-Fi latency | กลาง | กลาง | live latency + แนะนำ Ethernet |
| evdev permission งง | กลาง | กลาง | postinst auto + fallback libei |
| DPI mismatch | กลาง | กลาง | per-monitor scale ใน inject |
| Cursor ค้าง remote | กลาง | สูง | hotkey return + heartbeat 1.5s timeout |
| Audio feedback loop | กลาง | สูง | source_id tag + dedicated session |
| Source FSM race | กลาง | กลาง | tie-break deterministic + hysteresis 200 ms |
| SmartScreen ทำให้ user ไม่กล้า run | ต่ำ (hobby) | ต่ำ | docs + screenshot + self-sign optional |

---

## 13. Status

✅ ทุก decision lock แล้ว — พร้อมเริ่ม **M0**

**ผลที่คาดหวังจาก M0 (1.5 สัปดาห์):**
- Cargo workspace พร้อม CI ผ่านทั้ง Win + Linux
- Tauri shell เปิดได้ทั้งสอง OS
- daemon binary เปิดได้ + bind socket
- mDNS discovery: เปิด 2 เครื่องเห็นกัน
- Noise XX handshake: ลอง pair (ยังไม่มี PIN UI ก็ได้)
- ไม่มี input/audio ใด ๆ ใน M0 — แค่ skeleton

---

## 14. References

- Barrier / Input Leap — protocol baseline
- Synergy — UX inspiration
- libei spec — Wayland emulated input
- XDG Portal RemoteDesktop spec
- WASAPI loopback (Microsoft Learn)
- PipeWire native API
- Opus codec, RFC 6716
- Noise Protocol Framework
- Linux uinput / evdev kernel docs
- VB-Audio CABLE — vb-audio.com/Cable
