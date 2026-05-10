/**
 * Lightweight homegrown i18n — no react-intl, no i18next, just a
 * context + a `useT()` hook + a string dictionary keyed by
 * locale. Worth the ~50 lines of glue for a 2-language app where
 * most strings are short labels, and avoids dragging in a
 * thousand-line dependency.
 *
 * Locale choice persists to localStorage; the toggle in the
 * header writes there too. Default falls back to the
 * `navigator.language` prefix so a Thai system shows Thai on
 * first launch.
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

export type Locale = "en" | "th";

type Strings = Record<string, string>;

const en: Strings = {
  // Nav
  nav_status: "Status",
  nav_layout: "Layout",
  nav_devices: "Devices",
  nav_audio: "Audio",
  nav_files: "Files",
  nav_hotkeys: "Hotkeys",
  nav_advanced: "Advanced",

  // Connection pill
  conn_offline: "daemon offline:",
  conn_connecting: "connecting…",
  conn_no_peer: "no peer",
  conn_paired_with: "paired with",

  // Status grid
  stat_cursor: "Cursor",
  stat_peer_addr: "Peer addr",
  stat_sent_pkts: "Sent pkts",
  stat_recv_pkts: "Recv pkts",
  stat_audio_frames: "Audio frames",
  stat_injected: "Injected",
  cursor_local: "local",
  cursor_driving_peer: "driving peer",
  cursor_driven_by_peer: "driven by peer",

  // Game-mode card
  game_mode_on_title: "🔒 Game mode — input pinned to this PC",
  game_mode_off_title: "Game mode — off",
  game_mode_desc:
    "When on, edge crossing and auto-handover are disabled so an accidental cursor swing during a fullscreen game can't yank your keyboard to the other machine. Ctrl+Alt+R still works as a manual override.",
  game_mode_shortcut: "Toggle with",
  game_mode_lock: "Lock",
  game_mode_unlock: "Unlock",

  // Anti-cheat banner
  ac_title: "⚠ Anti-cheat-protected game detected:",
  ac_desc:
    "Input is auto-locked to this PC for safety. Kernel-level anti-cheat (BattlEye / EAC / Vanguard / RICOCHET / Hyperion) can flag SendInput-style injected events as cheating and ban accounts. The bridge will resume normally once the game is no longer in the foreground.",

  // Layout page
  layout_intro:
    "Drag the peer monitor against any edge of this machine's display to set the bridge direction. Edge detection, cursor warps, and Remote-mode sign conventions all flip when you change the side. Auto-saves on drop.",
  layout_summary_prefix: "Peer is on the",
  layout_summary_suffix: "of this display.",
  layout_saving: "saving…",
  layout_local: "this machine",
  layout_local_sub: "local",
  layout_no_peer: "no peer",
  layout_loading: "loading layout…",

  // Audio page
  audio_intro:
    "Two pipes per stream — outbound from this machine and inbound from the peer. Toggle either side off and the capture / playback keeps running for stats but the frames stop crossing the silent half. Takes effect on the next audio frame, no daemon restart.",
  audio_sysout_title: "System sound",
  audio_sysout_sub:
    "WASAPI loopback (Win) / PipeWire monitor (Linux) — captures whatever your default speakers are playing",
  audio_mic_title: "Microphone",
  audio_mic_sub: "cpal default input — your physical mic",
  audio_send: "Send to peer",
  audio_send_hint: "forward locally captured frames",
  audio_play: "Receive from peer",
  audio_play_hint: "render peer's frames here",

  // Pairing
  pair_title_request: "Pairing request from",
  pair_title_pair_with: "Pair with",
  pair_show_pin: "Read this PIN to whoever's typing on the other machine:",
  pair_enter_pin: "Enter the PIN displayed on that machine:",
  pair_button: "Pair",
  pair_button_sending: "Sending…",
  pair_after_note:
    "Once paired, this peer auto-connects forever — no PIN next time.",
  pair_cancel_note:
    "Cancel by closing this window or letting it time out (60 s).",
  pair_verifying: "Verifying…",
  pair_verifying_sub: "checking the PIN with the peer",
  pair_success_title: "✓ Paired",
  pair_success_sub: "is now trusted on this machine.",
  pair_failed_title: "Pairing failed",
};

const th: Strings = {
  nav_status: "สถานะ",
  nav_layout: "ตำแหน่งจอ",
  nav_devices: "อุปกรณ์",
  nav_audio: "เสียง",
  nav_files: "ไฟล์",
  nav_hotkeys: "คีย์ลัด",
  nav_advanced: "ขั้นสูง",

  conn_offline: "daemon ไม่ทำงาน:",
  conn_connecting: "กำลังเชื่อมต่อ…",
  conn_no_peer: "ไม่พบ peer",
  conn_paired_with: "เชื่อมต่อกับ",

  stat_cursor: "เคอร์เซอร์",
  stat_peer_addr: "ที่อยู่ peer",
  stat_sent_pkts: "ส่ง (pkts)",
  stat_recv_pkts: "รับ (pkts)",
  stat_audio_frames: "เฟรมเสียง",
  stat_injected: "Inject",
  cursor_local: "บน PC นี้",
  cursor_driving_peer: "กำลังคุม peer",
  cursor_driven_by_peer: "peer กำลังคุม",

  game_mode_on_title: "🔒 Game mode — pin input ที่ PC นี้",
  game_mode_off_title: "Game mode — ปิดอยู่",
  game_mode_desc:
    "เมื่อเปิด edge-cross และ auto-handover ถูกหยุด เผลอลากเมาส์เร็วในเกม fullscreen จะไม่ทำให้ keyboard ข้ามไปอีกเครื่อง Ctrl+Alt+R ยังใช้เป็น override ได้",
  game_mode_shortcut: "สลับด้วย",
  game_mode_lock: "ล็อก",
  game_mode_unlock: "ปลดล็อก",

  ac_title: "⚠ ตรวจพบเกมที่มีระบบ anti-cheat:",
  ac_desc:
    "Input ถูก auto-lock ไว้ที่ PC นี้เพื่อความปลอดภัย ระบบ anti-cheat ระดับ kernel (BattlEye / EAC / Vanguard / RICOCHET / Hyperion) อาจตรวจจับ injected events ว่าเป็นการโกงและแบนบัญชี bridge จะกลับมาทำงานปกติเมื่อเกมไม่ได้อยู่หน้าจอแล้ว",

  layout_intro:
    "ลาก peer monitor ไปขอบใดขอบหนึ่งของจอนี้เพื่อตั้งทิศ edge-detection, cursor warp, และ sign convention ของ Remote mode จะ flip ตามฝั่งที่เลือก บันทึกอัตโนมัติเมื่อปล่อย",
  layout_summary_prefix: "Peer อยู่ทาง",
  layout_summary_suffix: "ของจอนี้",
  layout_saving: "กำลังบันทึก…",
  layout_local: "เครื่องนี้",
  layout_local_sub: "local",
  layout_no_peer: "ไม่พบ peer",
  layout_loading: "กำลังโหลด layout…",

  audio_intro:
    "มี 2 ทิศต่อ 1 stream — outbound จากเครื่องนี้ และ inbound จาก peer ปิดฝั่งใดฝั่งหนึ่ง capture/playback ยังทำงาน แต่ frames จะไม่ข้ามเครือข่าย ผลทันทีในเฟรมถัดไป ไม่ต้องรีสตาร์ท",
  audio_sysout_title: "เสียงระบบ",
  audio_sysout_sub:
    "WASAPI loopback (Win) / PipeWire monitor (Linux) — จับเสียงจากลำโพง default",
  audio_mic_title: "ไมโครโฟน",
  audio_mic_sub: "cpal default input — ไมค์จริงของเครื่องนี้",
  audio_send: "ส่งไป peer",
  audio_send_hint: "forward frames ที่ capture ได้",
  audio_play: "รับจาก peer",
  audio_play_hint: "เล่น frames ของ peer ที่นี่",

  pair_title_request: "คำขอจับคู่จาก",
  pair_title_pair_with: "จับคู่กับ",
  pair_show_pin: "อ่าน PIN นี้ให้คนที่กำลังพิมพ์อยู่อีกเครื่องฟัง:",
  pair_enter_pin: "ใส่ PIN ที่แสดงอยู่บนเครื่องนั้น:",
  pair_button: "จับคู่",
  pair_button_sending: "กำลังส่ง…",
  pair_after_note: "หลังจับคู่แล้ว peer จะเชื่อมต่ออัตโนมัติตลอด ไม่ต้องใส่ PIN อีก",
  pair_cancel_note: "ยกเลิกได้โดยปิดหน้าต่างหรือปล่อย timeout (60 วินาที)",
  pair_verifying: "กำลังตรวจสอบ…",
  pair_verifying_sub: "เช็ค PIN กับ peer",
  pair_success_title: "✓ จับคู่สำเร็จ",
  pair_success_sub: "ถูกเพิ่มเป็น trusted บนเครื่องนี้แล้ว",
  pair_failed_title: "จับคู่ไม่สำเร็จ",
};

const dicts: Record<Locale, Strings> = { en, th };

interface I18nCtx {
  locale: Locale;
  setLocale: (l: Locale) => void;
  t: (key: string) => string;
}

const Ctx = createContext<I18nCtx>({
  locale: "en",
  setLocale: () => {},
  t: (k) => k,
});

const STORAGE_KEY = "mineshare:locale";

function detectInitialLocale(): Locale {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === "en" || saved === "th") return saved;
    const nav = navigator.language?.toLowerCase() ?? "";
    if (nav.startsWith("th")) return "th";
  } catch {
    // localStorage / navigator might be unavailable in dev tools
    // contexts; just fall through to en.
  }
  return "en";
}

export function LanguageProvider({ children }: { children: React.ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(detectInitialLocale);

  const setLocale = useCallback((l: Locale) => {
    setLocaleState(l);
    try {
      localStorage.setItem(STORAGE_KEY, l);
    } catch {}
  }, []);

  const t = useCallback(
    (key: string) => {
      const dict = dicts[locale] ?? en;
      return dict[key] ?? en[key] ?? key;
    },
    [locale],
  );

  // Reflect on <html lang="..."> for the curious.
  useEffect(() => {
    if (typeof document !== "undefined") {
      document.documentElement.lang = locale;
    }
  }, [locale]);

  const value = useMemo(() => ({ locale, setLocale, t }), [locale, setLocale, t]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useT() {
  return useContext(Ctx);
}

export function LanguageToggle() {
  const { locale, setLocale } = useT();
  return (
    <div className="inline-flex items-center gap-0 text-[11px] font-medium border border-neutral-200 dark:border-neutral-800 rounded-md overflow-hidden">
      <button
        onClick={() => setLocale("en")}
        className={
          "px-2 py-1 transition-colors " +
          (locale === "en"
            ? "bg-neutral-900 dark:bg-neutral-100 text-white dark:text-neutral-900"
            : "text-neutral-500 hover:text-neutral-900 dark:hover:text-neutral-100")
        }
      >
        EN
      </button>
      <button
        onClick={() => setLocale("th")}
        className={
          "px-2 py-1 transition-colors " +
          (locale === "th"
            ? "bg-neutral-900 dark:bg-neutral-100 text-white dark:text-neutral-900"
            : "text-neutral-500 hover:text-neutral-900 dark:hover:text-neutral-100")
        }
      >
        TH
      </button>
    </div>
  );
}
