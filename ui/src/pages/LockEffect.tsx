import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type LockEffectConfig = {
  rainbow: boolean;
  color: [number, number, number];
  animation: "flow" | "pulse" | "fade";
  duration_ms: number;
  thickness: number;
};
export const lockPresets: Record<string, LockEffectConfig> = {
  Rainbow: { rainbow: true, color: [40, 210, 190], animation: "flow", duration_ms: 1150, thickness: 6 },
  Aurora: { rainbow: false, color: [40, 210, 190], animation: "pulse", duration_ms: 1400, thickness: 8 },
  Amber: { rainbow: false, color: [255, 180, 50], animation: "fade", duration_ms: 900, thickness: 4 },
};
export function matchingPreset(effect: LockEffectConfig) {
  return Object.entries(lockPresets).find(([, preset]) =>
    (Object.keys(preset) as (keyof LockEffectConfig)[]).every(key => JSON.stringify(preset[key]) === JSON.stringify(effect[key]))
  )?.[0] ?? "Custom";
}

export default function LockEffectSection() {
  const [effect, setEffect] = useState<LockEffectConfig | null>(null);
  const [preset, setPreset] = useState("Custom");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  useEffect(() => {
    invoke<{ lock_effect: LockEffectConfig }>("get_settings").then(s => {
      setEffect(s.lock_effect);
      setPreset(matchingPreset(s.lock_effect));
    }).catch(e => setError(String(e)));
  }, []);
  function edit(patch: Partial<LockEffectConfig>) {
    setEffect(current => current && ({ ...current, ...patch }));
    setPreset("Custom"); setMessage(""); setError("");
  }
  async function save() {
    setBusy(true); setError(""); setMessage("");
    try {
      const result = await invoke<{ lock_effect: LockEffectConfig }>("patch_settings", { patch: { lock_effect: effect } });
      setEffect(result.lock_effect); setMessage("Saved on this PC / บันทึกแล้ว");
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function preview(locked: boolean) {
    setError("");
    try { await invoke("preview_lock_effect", { locked, effect }); }
    catch (e) { setError(String(e)); }
  }
  const field = "rounded-lg border border-ds-border bg-ds-bg px-3 py-2 text-sm text-ds-text";
  const button = `${field} hover:bg-ds-hover disabled:opacity-50`;
  return <section className="rounded-xl border border-ds-border bg-ds-surface p-5">
    <h3 className="text-sm font-semibold text-ds-text">Game Lock effect</h3>
    <p className="text-xs text-ds-text-muted mt-1 mb-5">ปรับเอฟเฟกต์ Ctrl+Shift+L ของเครื่องนี้ · Preview ไม่เปลี่ยนสถานะล็อก</p>
    {effect ? <fieldset disabled={busy} className="space-y-4">
      <label className="flex flex-col gap-2 text-sm">Preset
        <select className={field} value={preset} onChange={e => {
          setPreset(e.target.value); setMessage("");
          if (e.target.value !== "Custom") setEffect({ ...lockPresets[e.target.value] });
        }}>
          {[...Object.keys(lockPresets), "Custom"].map(name => <option key={name}>{name}</option>)}
        </select>
      </label>
      <div className="flex items-center gap-5 flex-wrap">
        <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={effect.rainbow} onChange={e => edit({ rainbow: e.target.checked })} />Rainbow</label>
        <label className="flex items-center gap-2 text-sm">Color
          <input type="color" disabled={effect.rainbow} value={`#${effect.color.map(c => c.toString(16).padStart(2, "0")).join("")}`}
            onChange={e => edit({ color: [1, 3, 5].map(i => parseInt(e.target.value.slice(i, i + 2), 16)) as [number, number, number] })} />
        </label>
      </div>
      <label className="flex flex-col gap-2 text-sm">Animation
        <select className={field} value={effect.animation} onChange={e => edit({ animation: e.target.value as LockEffectConfig["animation"] })}>
          <option value="flow">Flow — เคลื่อนและคลาย</option><option value="pulse">Pulse — เรืองแสงเป็นจังหวะ</option><option value="fade">Fade — จางเรียบ ๆ</option>
        </select>
      </label>
      <label className="flex flex-col gap-2 text-sm">Duration: {(effect.duration_ms / 1000).toFixed(2)} s
        <input type="range" min={300} max={3000} step={50} value={effect.duration_ms} onChange={e => edit({ duration_ms: Number(e.target.value) })} />
      </label>
      <label className="flex flex-col gap-2 text-sm">Border width: {effect.thickness} px
        <input type="range" min={2} max={16} value={effect.thickness} onChange={e => edit({ thickness: Number(e.target.value) })} />
      </label>
      <p className="text-xs text-ds-text-muted">ขอบหายเองเสมอ · ตอนปลดล็อกใช้เวลาประมาณ 61% ของค่าด้านบน</p>
      <div className="flex flex-wrap gap-2">
        <button type="button" className={button} onClick={() => preview(true)}>Preview lock</button>
        <button type="button" className={button} onClick={() => preview(false)}>Preview unlock</button>
        <button type="button" className={button} onClick={save}>{busy ? "Saving…" : "Save effect"}</button>
      </div>
    </fieldset> : <p className="text-sm text-ds-text-muted">Loading…</p>}
    {message && <p role="status" className="text-sm text-emerald-500 mt-3">{message}</p>}
    {error && <p role="alert" className="text-sm text-red-400 mt-3">{error}</p>}
  </section>;
}
