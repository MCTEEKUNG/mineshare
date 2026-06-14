import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import HotkeysPage from "./Hotkeys";
import AdvancedPage from "./Advanced";
import { useT } from "../i18n";

export default function SettingsPage() {
  const { t } = useT();
  return (
    <div className="flex flex-col gap-10">
      <section>
        <h3 className="text-sm font-semibold text-slate-300 mb-3">{t("section_performance")}</h3>
        <PerformanceSection />
      </section>
      <section>
        <h3 className="text-sm font-semibold text-slate-300 mb-3">{t("section_hotkeys")}</h3>
        <HotkeysPage />
      </section>
      <section>
        <h3 className="text-sm font-semibold text-slate-300 mb-3">{t("section_advanced")}</h3>
        <AdvancedPage />
      </section>
    </div>
  );
}

type MouseRateStats = { set_hz: number; fwd_total: number; inj_total: number };

/**
 * Performance section — mouse forward/inject rate slider with a
 * live readout of the achieved rate.
 *
 * Self-contained settings load/write mirroring `InputPrefsCard`
 * in Advanced.tsx: `get_settings` on mount, `set_settings` on
 * each change with no debounce (matches the existing slider
 * pattern). Note: this is a second independent settings loader
 * alongside Advanced's `InputPrefsCard`; the runtime spread
 * preserves the full struct so writes from one don't drop the
 * other's fields, with the usual last-writer-wins caveat if both
 * sliders move between loads.
 */
function PerformanceSection() {
  const [settings, setSettings] = useState<{ mouse_rate_hz: number } | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const [stats, setStats] = useState<MouseRateStats | null>(null);
  const [prev, setPrev] = useState<{ fwd: number; inj: number; t: number } | null>(null);
  const [liveFwd, setLiveFwd] = useState<number>(0);
  const [liveInj, setLiveInj] = useState<number>(0);

  useEffect(() => {
    invoke<{ mouse_rate_hz: number }>("get_settings")
      .then(setSettings)
      .catch((e) => setErr(String(e)));
  }, []);

  useEffect(() => {
    const id = setInterval(async () => {
      const s = await invoke<MouseRateStats>("get_mouse_rate_stats").catch(() => null);
      if (!s) return;
      const now = performance.now();
      if (prev) {
        const dt = (now - prev.t) / 1000;
        if (dt > 0) {
          setLiveFwd(Math.round((s.fwd_total - prev.fwd) / dt));
          setLiveInj(Math.round((s.inj_total - prev.inj) / dt));
        }
      }
      setPrev({ fwd: s.fwd_total, inj: s.inj_total, t: now });
      setStats(s);
    }, 1000);
    return () => clearInterval(id);
  }, [prev]);

  async function update(next: { mouse_rate_hz: number }) {
    setErr(null);
    setSettings(next);
    try {
      const applied = await invoke<{ mouse_rate_hz: number }>("set_settings", { settings: next });
      setSettings(applied);
    } catch (e) {
      setErr(String(e));
    }
  }

  if (!settings) {
    return (
      <div className="rounded-xl border border-white/[0.08] bg-ds-surface p-5">
        <p className="text-base font-semibold text-slate-100 mb-3">Performance</p>
        <p className="text-xs text-slate-400">
          {err ? `failed to load: ${err}` : "loading…"}
        </p>
      </div>
    );
  }

  return (
    <div className="rounded-xl border border-white/[0.08] bg-ds-surface p-5">
      <div className="flex items-baseline justify-between mb-2">
        <label className="text-sm font-medium text-slate-200">Mouse rate</label>
        <span className="font-mono text-sm text-slate-400">{settings.mouse_rate_hz} Hz</span>
      </div>
      <input
        type="range" min={60} max={1000} step={5}
        value={settings.mouse_rate_hz}
        onChange={(e) => update({ ...settings, mouse_rate_hz: parseInt(e.target.value, 10) })}
        list="mouse-rate-ticks"
        className="w-full accent-emerald-500"
      />
      <datalist id="mouse-rate-ticks">
        <option value="125" /><option value="250" /><option value="500" /><option value="1000" />
      </datalist>
      <p className="text-xs text-slate-400 mt-2">
        Live: forwarding ~{liveFwd} Hz · injecting ~{liveInj} Hz
        {stats ? ` · set ${stats.set_hz} Hz` : ""}
      </p>
      <p className="text-[11px] text-slate-400 mt-1">
        Higher rates feel smoother but use more network/CPU. If the live rate stays below your setting,
        your hardware or the peer's inject path is the limit. (Some anti-cheats ignore injected motion in games.)
      </p>
      {err ? <p className="text-xs text-red-400 mt-3">{err}</p> : null}
    </div>
  );
}
