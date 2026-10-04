# รายงานผลการปรับปรุงประสิทธิภาพ MineShare v0.0.11

วันที่จัดทำ: 19 กรกฎาคม 2026
ระบบที่ทดสอบ: Laptop `LAPTOP-4E1649F3` และ PC `MCTEEKUNG-PC` (`192.168.1.104`)
ขอบเขต: Input latency, audio stability, CPU/RAM, ความพร้อมใช้งานต่อเนื่อง และ Windows application identity

## Executive Summary

MineShare v0.0.11 ลดความหน่วงของเส้นทางรับ–ฉีด mouse input อย่างมีนัยสำคัญ โดยค่า native injection p99 ลดจาก **7.20 ms เหลือ 3.06 ms** หรือดีขึ้นประมาณ **57.5%** ขณะเดียวกัน scheduler ใหม่ลดจำนวนการปลุก thread จากเดิมสูงสุดหนึ่งครั้งต่อ hardware event เหลือไม่เกิน 4 ครั้งต่อ burst 1,000 events หรือคิดเป็นการลด scheduler wake-up อย่างน้อย **99.6%** ใน regression gate

การปรับปรุงไม่ได้ลด mouse polling rate, Opus quality, encryption หรือ feature ของระบบ แต่เปลี่ยนวิธีจัดคิวและกำหนดเวลาให้ทำงานน้อยลงและตรงเวลามากขึ้น ผลทดสอบบน LAN พบ RTT ปกติอยู่ที่ **0–2 ms** จึงยืนยันได้ว่าปัญหากระตุกเดิมไม่ได้เกิดจาก network เป็นหลัก แต่เกิดจาก local scheduling, การสะสมของ input queue และต้นทุนของ Windows input injection

ด้านเสียง ระบบรับ audio probe จาก PC มายัง Laptop ได้ **132 Opus frames** โดยไม่พบ audio underrun, queue overflow, decrypt error หรือ inject error ในช่วงทดสอบ การเลือกอุปกรณ์เสียงยังคงติดตาม Windows default output ที่ผู้ใช้กำลังใช้งาน

Release เดียวกันถูกติดตั้งบนทั้งสองเครื่องและเชื่อมต่อกันเป็น session เดียว โดย binary ปัจจุบันมี SHA-256:

`47929809BD0D3D80DD7E42A0D104DC958769A6C74E35B7E868C7CD37A7CE2C3F`

## ผลการวัด Before / After

| ตัวชี้วัด | ก่อนปรับปรุง | หลังปรับปรุง | ผลลัพธ์ |
|---|---:|---:|---:|
| Native Windows mouse injection p99 | 7.20 ms | 3.06 ms | ดีขึ้นประมาณ 57.5% |
| Native Windows mouse injection max | 8.92 ms โดยประมาณ | 11.63 ms | p99 ดีขึ้นชัดเจน; max ยังต่ำกว่าเกณฑ์ stall 50 ms |
| Scheduler unparks ต่อ mouse burst 1,000 events | 1,000 ครั้ง | ≤ 4 ครั้ง | ลดลง ≥ 99.6% |
| Mouse distance หลัง coalescing | ไม่มี gate รับรองครบถ้วน | 1,000 / 1,000 units | ระยะไม่สูญหาย |
| Keyboard หลัง mouse burst | มีความเสี่ยงติดหลัง stale moves | รอไม่เกิน 2 mouse injections ใน regression | ลด key starvation |
| LAN RTT | 0–2 ms | 0–2 ms | ยืนยันว่า network ไม่ใช่ bottleneck หลัก |
| Audio probe PC → Laptop | ไม่มีกลไก probe ที่ชัดเจน | 132 frames, ไม่พบ underrun | เส้นทางเสียงทำงานจริง |
| Idle CPU — Laptop | ไม่มี baseline ที่เทียบแบบ controlled | 0.125% | ใช้เป็นค่าปัจจุบัน ไม่กล่าวอ้างเป็น delta |
| Idle CPU — PC | ไม่มี baseline ที่เทียบแบบ controlled | 0.709% | ใช้เป็นค่าปัจจุบัน ไม่กล่าวอ้างเป็น delta |
| Working Set — Laptop | ไม่มี baseline ที่เทียบแบบ controlled | 36.6 MB | คงที่ใน sampling window |
| Working Set — PC | ไม่มี baseline ที่เทียบแบบ controlled | 31.7 MB | คงที่ใน sampling window |
| Automated regression | — | Rust 57 + UI 2 = 59 tests ผ่าน | ไม่พบ feature regression ในชุดทดสอบ |

> หมายเหตุ: ค่า CPU/RAM ก่อนแก้ไม่ได้เก็บภายใต้ workload และ sampling window เดียวกัน จึงไม่ควรสรุปเป็นเปอร์เซ็นต์ improvement เทียบ baseline แม้ว่าค่าหลังปรับปรุงจะอยู่ในระดับต่ำและคงที่

## การเปลี่ยนแปลงที่ทำให้ Performance ดีขึ้น

### 1. เปลี่ยน scheduler เป็น monotonic absolute-deadline

ระบบเดิมอาศัยการตรวจเวลาจาก event แต่ละครั้งและปลุก watchdog บ่อย ทำให้เกิด scheduler churn และ deadline drift โดยเฉพาะเมาส์ 1,000 Hz

ระบบใหม่ใช้ monotonic high-resolution clock และบันทึก deadline แบบ absolute:

- ปลุก watchdog เฉพาะตอน queue เปลี่ยนจากว่างเป็นมีข้อมูล
- park thread ขณะ idle แทนการ polling
- คำนวณเวลาที่เหลือถึง deadline จริง ไม่สะสม drift จากรอบก่อน
- เปิด Windows high-resolution timer เฉพาะช่วงที่กำลังควบคุมเครื่องปลายทาง

ผลสำคัญคือ CPU ไม่ต้องรับ context-switch ตาม hardware event ทุกครั้ง แต่ mouse rate ที่ผู้ใช้ตั้งไว้ยังคงเดิม

### 2. Coalesce mouse ตั้งแต่ enqueue boundary

การรวม mouse event เดิมเกิดช้าเกินไป ทำให้ queue สามารถสะสม mouse moves จำนวนมากก่อนถึง keyboard หรือ click event

dispatcher ใหม่รวมเฉพาะ `MouseMove` ที่อยู่ติดกันทันทีตอน enqueue:

- รวม `dx/dy` ด้วย saturating arithmetic
- ไม่รวมข้าม click, scroll หรือ keyboard event
- รักษาลำดับ event ที่มีความหมาย
- ส่งระยะ mouse ครบ แม้หลาย packet ถูกยุบเป็น injection เดียว
- held-key reconciliation และ authoritative snapshot ยังทำงานเหมือนเดิม

จุดนี้ลดทั้ง queue depth, lock contention และเวลาที่ key ต้องรอ โดยไม่ลดความละเอียดเชิงระยะของเมาส์

### 3. ใช้ native relative `SendInput`

เส้นทางเดิมของ library ต้องอ่านตำแหน่ง cursor, อ่านขนาดหน้าจอ และแปลงเป็น absolute coordinates ต่อ event ต้นทุนดังกล่าวสูงเกินไปเมื่อมี burst

เส้นทางใหม่ส่ง relative mouse input ผ่าน Windows `SendInput` โดยตรง:

- ลด syscall และ coordinate conversion
- รองรับ pointer-lock game ได้สม่ำเสมอขึ้น
- ให้ Windows ใช้ pointer speed/acceleration ที่ผู้ใช้ตั้งไว้
- ลดโอกาสที่ receive loop จะถูก injection call ครอบครองนานเกินไป

### 4. แยก input injection ออกจาก network receive task

UDP receive task ไม่ควรถูก block ด้วย synchronous OS injection เพราะจะทำให้ packet ใหม่ทั้งหมดกลายเป็นข้อมูลเก่าใน queue

ระบบใหม่ใช้ dedicated injection worker:

- network task ทำหน้าที่ decrypt, validate และ enqueue
- injection worker ทำงานกับ OS แยกต่างหาก
- ปรับ priority เฉพาะ injection thread เป็น Above Normal บน Windows
- ไม่เปลี่ยน process priority จึงลดความเสี่ยงแย่งเวลา audio/network thread

### 5. ทำ audio pipeline ให้ bounded และ demand-aware

ระบบ capture แบบ unbounded สามารถสะสม audio เก่าและเพิ่ม memory เมื่อ consumer ช้าหรือ peer หลุด

ระบบใหม่:

- จำกัด capture queue ไว้ที่ 8 frames
- drop ข้อมูลเก่าตามธรรมชาติของ real-time audio แทนการสะสมเสียงที่หมดอายุ
- encode/capture เฉพาะเมื่อ toggle เปิดและมี receiver
- รักษา Opus bitrate, jitter target และ frame rate เดิม
- jitter reserve ผ่าน regression สำหรับ delivery gap 60 ms
- default output switching rebuild stream เฉพาะเมื่อระบบอยู่ในโหมด follow-default

สิ่งสำคัญคือความเสถียรเพิ่มขึ้นจากการควบคุม backpressure ไม่ใช่จากการลดคุณภาพเสียง

### 6. ลด log volume ใน hot path

sample logs ของ input/audio ที่เคยอยู่ระดับ `INFO` ถูกลดเป็น `DEBUG` ทำให้ production:

- ลด synchronous formatting
- ลด disk I/O
- ลด log growth สำหรับการรัน 24/7
- ยังคง error และ health telemetry ที่จำเป็น

### 7. UI theme ไม่มี animation หรือ backdrop หนัก

ธีมทอง–หินอ่อนใช้ static CSS gradients และ semantic color tokens:

- ไม่มี animated marble
- ไม่มี continuous GPU effect
- รองรับ light/dark/system เหมือนเดิม
- contrast ต่ำสุดที่ตรวจได้ 4.75:1

การ rebrand จึงไม่เพิ่ม background CPU/GPU workload อย่างมีนัยสำคัญ

## Breakthrough และสิ่งที่ได้เรียนรู้

### Breakthrough 1: LAN latency ต่ำ ไม่ได้แปลว่า end-to-end latency ใกล้ศูนย์

RTT 0–2 ms เป็นเพียงเวลาบน network แต่ประสบการณ์จริงยังรวม:

`raw input → scheduler → encryption/send → receive queue → decrypt → OS injection → display`

ในกรณีนี้ bottleneck หลักอยู่หลัง packet มาถึงแล้ว โดยเฉพาะ injection call และ queue starvation การลด network latency เพิ่มอีกเล็กน้อยจึงแทบไม่ช่วยจนกว่าจะแก้ local pipeline

### Breakthrough 2: ตำแหน่งที่ทำ coalescing สำคัญกว่าจำนวนที่ coalesce

การรวม event ที่ปลาย queue ช่วยลดจำนวน injection แต่ไม่ช่วย key ที่ติดอยู่หลัง mouse events จำนวนมาก การรวมตั้งแต่ enqueue boundary ทำให้ queue ไม่โตตั้งแต่ต้น นี่เป็นการแก้ latency ที่ต้นเหตุ ไม่ใช่แค่ทำให้ consumer เร็วขึ้น

### Breakthrough 3: Performance ที่ดีต้องรักษา semantic correctness

การลดจำนวน mouse events แบบง่ายสามารถทำให้:

- ระยะ cursor หาย
- click/key สลับลำดับ
- held key ค้าง
- game input ไม่ตรงกับ desktop

การ optimize ที่ถูกต้องจึงต้องมี invariant ชัดเจน: ระยะครบ, ordering ครบ, reconciliation ครบ และ protocol ไม่เปลี่ยน

### Breakthrough 4: Audio stability มาจาก backpressure ไม่ใช่การลด bitrate

เสียงแตกและ loop ไม่จำเป็นต้องแก้ด้วยการลดคุณภาพ Opus การใช้ bounded queue, jitter reserve และ demand-aware capture ช่วยให้ระบบทิ้งข้อมูลที่หมดอายุแทนการเล่นตามหลังหรือสะสมจนเกิด loop

### Breakthrough 5: Event-driven idle path สำคัญต่อระบบ 24/7

งานที่ดูเล็ก เช่น watchdog ตื่นทุกไม่กี่ millisecond จะกลายเป็น CPU usage และ battery drain เมื่อคูณด้วยการทำงานหลายชั่วโมง การ park ขณะ idle และ wake เฉพาะ state transition เป็น optimization ที่มีผลต่อความพร้อมใช้งานระยะยาวมากกว่าค่า benchmark ช่วงสั้นบางชนิด

### Breakthrough 6: Windows Taskbar icon มีหลาย identity layer

การเปลี่ยนไฟล์ `.ico` อย่างเดียวไม่พอ Windows แยก:

1. Bundle/executable icon
2. Shortcut `IconLocation`
3. Live window `WM_GETICON` สำหรับ `ICON_SMALL` และ `ICON_BIG`

ปัญหารูปกระดาษเปล่าเกิดจาก live window มี `SmallIcon` แต่ `BigIcon=0` Taskbar จึง fallback เป็น generic document หลังแก้ runtime ให้ตั้งทั้ง BIG และ SMALL แล้ว ค่า handle เป็น non-zero และ extract กลับมาเป็น Linked‑M ที่ออกแบบไว้จริง

### Breakthrough 7: หลักฐาน regression สำคัญกว่าความรู้สึกว่า “เร็วขึ้น”

การมี red-capable tests ทำให้พิสูจน์ได้ว่า:

- implementation เดิมปลุก scheduler 1,000 ครั้งต่อ burst
- implementation ใหม่ลดเหลือไม่เกิน 4
- cursor distance ยังครบ 1,000 units
- keyboard ไม่ติดหลัง stale mouse queue

ตัวเลขเหล่านี้ช่วยแยก improvement จริงออกจาก placebo และป้องกันไม่ให้ optimization รอบถัดไปทำลาย correctness

## ความเสถียรและความพร้อมใช้งาน 24/7

กลไกที่ช่วยลดความเสี่ยงระยะยาวประกอบด้วย:

- singleton runtime ป้องกัน GUI/daemon ซ้อนกันใน Windows session เดียว
- bounded audio queue ป้องกัน memory growth
- event-driven watchdog ลด idle wake-up
- reconnection และ Noise XX encrypted session คงเดิม
- Taskbar/Startup path คงที่และมี migration สำหรับ icon path เก่า
- release binary เดียวกันบนสองเครื่อง ลด version drift
- มี backup ของ binary และ shortcut ก่อน replace

## ข้อจำกัดของผลการทดสอบ

- ยังไม่มี controlled CPU/RAM baseline จาก build ก่อนหน้าใน workload เดียวกัน จึงรายงานเฉพาะค่าหลังปรับปรุง
- native injection benchmark วัดส่วนสำคัญของ end-to-end path แต่ไม่ใช่ hardware-to-display measurement ด้วย high-speed camera
- การ rebuild เพื่อแก้ Windows icon ทำให้ continuous 12-hour soak รอบแรกถูกขัดจังหวะ จึงยังไม่ควรประกาศว่า elapsed gate 12 ชั่วโมงผ่านสมบูรณ์
- audio probe ยืนยัน frame delivery และไม่มี error ใน log แต่การประเมินคุณภาพเสียงเชิง perception ยังต้องอาศัยการฟังจริงระหว่าง workload ต่อเนื่อง

## ข้อเสนอแนะสำหรับรอบถัดไป

1. สร้าง benchmark harness ที่ timestamp ทั้ง source capture และ destination injection ด้วย clock correlation ระหว่างเครื่อง
2. เก็บ ETW trace ของ DPC/ISR, scheduler wake และ audio glitches ระหว่าง active stress 30 นาที
3. เพิ่ม telemetry แบบ bounded histogram สำหรับ queue wait โดยปิดเป็นค่าเริ่มต้นใน production
4. ทำ controlled A/B CPU/RAM benchmark โดยใช้ binary ก่อนและหลัง บน workload replay เดียวกัน
5. รัน passive soak ต่อเนื่อง 12–24 ชั่วโมงหลังหยุดเปลี่ยน binary และสรุป reconnect, memory slope และ error count

## บทสรุป

การปรับปรุง v0.0.11 ไม่ใช่การ “ลด performance เพื่อเอา latency” แต่เป็นการลดงานที่ไม่จำเป็นใน hot path และจัดลำดับงานให้ตรงกับธรรมชาติของ real-time input/audio

ผลลัพธ์ที่พิสูจน์ได้ชัดที่สุดคือ native injection p99 ดีขึ้นประมาณ 57.5%, scheduler wake-up ลดลงอย่างน้อย 99.6% ใน burst test, mouse distance และ key ordering ยังครบ, audio delivery ทำงานโดยไม่มี error ใน probe และ idle resource usage อยู่ในระดับต่ำบนทั้งสองเครื่อง

Breakthrough หลักคือการมอง latency เป็น pipeline ทั้งระบบ ไม่ใช่ตัวเลข ping เพียงค่าเดียว เมื่อแก้ scheduling, queue boundary, OS injection และ backpressure พร้อมกัน ระบบจึงตอบสนองเร็วขึ้นโดยไม่ต้องลดคุณภาพหรือ feature
