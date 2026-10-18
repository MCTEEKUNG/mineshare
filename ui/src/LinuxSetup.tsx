import {useCallback, useEffect, useRef, useState} from "react";
import {invoke} from "@tauri-apps/api/core";
import {useT} from "./i18n";
import {needsSetup, recommended, type SetupOptions, type SetupStatus} from "./linuxSetupModel";

export default function LinuxSetup() {
  const {locale} = useT();
  const th = locale === "th";
  const text = (en: string, thai: string) => th ? thai : en;
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [options, setOptions] = useState<SetupOptions>({input: false, broker: false, pointer: false});
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [done, setDone] = useState(false);
  const closeButton = useRef<HTMLButtonElement>(null);
  const dialog = useRef<HTMLDivElement>(null);
  const previousFocus = useRef<HTMLElement | null>(null);
  const refresh = useCallback(async () => {
    const next = await invoke<SetupStatus>("get_linux_setup_status");
    setStatus(next); setOptions(recommended(next));
    return next;
  }, []);
  useEffect(() => {
    let live = true;
    invoke<SetupStatus>("get_linux_setup_status").then(s => {
      if (!live) return;
      setStatus(s); setOptions(recommended(s));
      let dismissed = false;
      try { dismissed = sessionStorage.getItem("mineshare-linux-setup-later") === "1"; } catch { /* optional */ }
      setOpen(needsSetup(s) && !dismissed);
    }).catch(() => { /* Browser preview/unsupported older backend. */ });
    const show = () => { setOpen(true); setError(""); void refresh().catch(e => setError(String(e))); };
    window.addEventListener("mineshare-linux-setup", show);
    return () => { live = false; window.removeEventListener("mineshare-linux-setup", show); };
  }, [refresh]);
  useEffect(() => {
    if (!open) return;
    previousFocus.current = document.activeElement as HTMLElement;
    closeButton.current?.focus();
    return () => previousFocus.current?.focus();
  }, [open]);
  function close() {
    if (busy) return;
    try { sessionStorage.setItem("mineshare-linux-setup-later", "1"); } catch { /* optional */ }
    setOpen(false);
  }
  async function configure() {
    setBusy(true); setError(""); setDone(false);
    try {
      const next = await invoke<SetupStatus>("configure_linux_setup", {options});
      setStatus(next); setOptions(recommended(next)); setDone(true);
    } catch (e) {
      setError(String(e));
      try { await refresh(); } catch { /* Keep original setup error. */ }
    } finally { setBusy(false); }
  }
  if (!open || !status?.linux) return null;
  const selected = Object.values(options).some(Boolean);
  const state = (active: boolean, configured: boolean) => active ? text("Active", "กำลังทำงาน") : configured ? text("Configured · sign out/in required", "ตั้งค่าแล้ว · ต้องออก–เข้าใหม่") : text("Not configured", "ยังไม่ได้ตั้งค่า");
  return <div className="fixed inset-0 z-40 bg-black/60 backdrop-blur-sm flex items-center justify-center p-5">
    <div ref={dialog} tabIndex={-1} role="dialog" aria-modal="true" aria-labelledby="linux-setup-title" aria-busy={busy}
      className="w-full max-w-xl max-h-[90vh] overflow-y-auto rounded-2xl border border-ds-border bg-ds-surface p-6 shadow-2xl"
      onKeyDown={e => {
        if (e.key === "Escape") { e.preventDefault(); close(); }
        if (e.key === "Tab") {
          const buttons = dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)');
          if (!buttons?.length) { e.preventDefault(); dialog.current?.focus(); return; }
          const first = buttons[0], last = buttons[buttons.length - 1];
          if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
          else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
        }
      }}>
      <h2 id="linux-setup-title" className="text-xl font-semibold">{text("Set up MineShare on Ubuntu", "เตรียม MineShare บน Ubuntu")}</h2>
      <p className="mt-2 text-sm text-ds-text-muted">{text("Review the detected environment and choose what to configure. Nothing changes until you click Set up.", "ตรวจระบบและเลือกส่วนที่ต้องการตั้งค่า ยังไม่มีการเปลี่ยนแปลงจนกดตั้งค่า")}</p>
      <p className="mt-2 text-xs text-ds-text-muted">{status.session || "Linux"} · {status.gnome_supported ? "GNOME 45–50" : text("GNOME helper unavailable", "ไม่รองรับส่วนเสริม GNOME")}</p>
      <div className="my-5 space-y-4">
        <label className="flex gap-3"><input type="checkbox" checked={options.input} disabled={busy || (status.input_configured && status.uinput_available && !status.input_needs_repair)} onChange={e => setOptions({...options, input: e.target.checked})}/><span><strong>{text("Input device permissions", "สิทธิ์อุปกรณ์ input")}</strong><small className="block text-ds-text-muted">{status.input_needs_repair ? text("Device access needs repair", "ต้องซ่อมสิทธิ์อุปกรณ์") : state(status.input_active, status.input_configured && status.uinput_available)} · {text("May ask for administrator authentication.", "อาจต้องยืนยันสิทธิ์ผู้ดูแลระบบ")}</small></span></label>
        <label className="flex gap-3"><input type="checkbox" checked={options.broker} disabled={busy || !status.typeless || status.broker_configured} onChange={e => setOptions({...options, broker: e.target.checked})}/><span><strong>{text("Typeless keyboard compatibility", "ใช้คีย์บอร์ดร่วมกับ Typeless")}</strong><small className="block text-ds-text-muted">{status.typeless ? state(status.broker_active, status.broker_configured) : text("Typeless not detected · optional", "ไม่พบ Typeless · ไม่จำเป็นต้องเปิด")}<br/>{text("Enables ordered startup: MineShare before Typeless. Does not stop either app now.", "จัด startup ให้ MineShare เปิดก่อน Typeless ไม่ปิดแอปในขณะนี้")}</small></span></label>
        {status.wayland && <label className="flex gap-3"><input type="checkbox" checked={options.pointer} disabled={busy || !status.gnome_supported || !status.extensions_allowed || status.extension_configured} onChange={e => setOptions({...options, pointer: e.target.checked})}/><span><strong>{text("Accurate Wayland cursor crossing", "ข้ามจอ Wayland ตามตำแหน่งจริง")}</strong><small className="block text-ds-text-muted">{state(status.extension_active, status.extension_configured)}<br/>{text("Read-only GNOME extension: pointer coordinates only, no screenshots or clipboard access.", "ส่วนเสริม GNOME อ่านเฉพาะตำแหน่งเมาส์ ไม่อ่านภาพจอหรือคลิปบอร์ด")}</small></span></label>}
      </div>
      {status.wayland && (!status.gnome_supported || !status.extensions_allowed) && <p className="text-sm text-amber-500">{text("Automatic crossing needs supported GNOME with user extensions allowed. This setup will not change your global extension preference. Ctrl+Alt+R is the manual handoff.", "ข้ามจออัตโนมัติต้องใช้ GNOME ที่รองรับและอนุญาตส่วนเสริม แอปจะไม่เปลี่ยนการปิดส่วนเสริมทั้งระบบ ใช้ Ctrl+Alt+R สลับด้วยตนเองได้")}</p>}
      {!status.helpers_available && <p role="alert" className="text-sm text-red-400">{text("Setup components or authentication helper are missing. Reinstall the current .deb package with its dependencies.", "ไม่พบไฟล์ตั้งค่าหรือตัวช่วยยืนยันสิทธิ์ กรุณาติดตั้ง .deb รุ่นปัจจุบันพร้อม dependencies อีกครั้ง")}</p>}
      {(status.pending_login || done) && <p role="status" className="my-4 text-sm text-ds-accent">{status.pending_login ? text("Save your work, then sign out of Ubuntu and sign in once. Let MineShare and Typeless start automatically. Installed is not the same as active; use Check again after login.", "บันทึกงาน แล้วออกจากระบบ Ubuntu และเข้าใหม่หนึ่งครั้ง ปล่อยให้ MineShare/Typeless เปิดอัตโนมัติ ตั้งค่าแล้วไม่ได้หมายถึงกำลังทำงาน กดตรวจอีกครั้งหลังเข้าใหม่") : text("Setup completed. Check the active status above.", "ตั้งค่าเสร็จแล้ว ตรวจสถานะการทำงานด้านบนได้เลย")}</p>}
      {error && <p role="alert" className="my-3 text-sm text-red-400">{error}</p>}
      <div className="flex flex-wrap justify-end gap-3 mt-5 text-sm">
        <button ref={closeButton} disabled={busy} onClick={close} className="px-3 py-2 rounded-lg border border-ds-border">{text("Later / Close", "ภายหลัง / ปิด")}</button>
        <button disabled={busy} onClick={() => { setError(""); void refresh().catch(e => setError(String(e))); }} className="px-3 py-2 rounded-lg border border-ds-border">{text("Check again", "ตรวจอีกครั้ง")}</button>
        <button disabled={busy || !selected || !status.helpers_available} onClick={() => void configure()} className="px-4 py-2 rounded-lg bg-ds-accent text-ds-bg disabled:opacity-40">{busy ? text("Setting up…", "กำลังตั้งค่า…") : text("Set up selected", "ตั้งค่ารายการที่เลือก")}</button>
      </div>
    </div>
  </div>;
}
