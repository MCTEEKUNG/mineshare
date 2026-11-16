import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "./i18n";

import { ConnectionContext, useConnectionControl, type Control } from "./connectionState";
import DevicePicker from "./DevicePicker";

export function ConnectionProvider({ children }: { children: ReactNode }) {
  const [control, setControl] = useState<Control | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pollError, setPollError] = useState<string | null>(null);
  const revision = useRef(0);
  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setInterval> | undefined;
    let loading = false;
    const tick = async () => {
      if (loading) return;
      loading = true;
      const requestedAt = revision.current;
      try {
        const next = await invoke<Control>("get_connection_control");
        if (active && requestedAt === revision.current) { setControl(next); setPollError(null); }
      } catch (e) { if (active) setPollError(String(e)); }
      finally { loading = false; }
    };
    const syncVisibility = () => {
      if (timer !== undefined) clearInterval(timer);
      timer = undefined;
      if (!document.hidden) { void tick(); timer = setInterval(tick, 500); }
    };
    syncVisibility();
    document.addEventListener("visibilitychange", syncVisibility);
    return () => { active = false; clearInterval(timer); document.removeEventListener("visibilitychange", syncVisibility); };
  }, []);

  async function change(command: string, args: Record<string, unknown>) {
    revision.current++;
    setBusy(true);
    setError(null);
    try {
      await invoke(command, args);
      setControl(await invoke<Control>("get_connection_control"));
      return true;
    } catch (e) { setError(String(e)); return false; }
    finally { setBusy(false); }
  }

  return <ConnectionContext.Provider value={{ control, busy, error: error ?? pollError, change }}>{children}</ConnectionContext.Provider>;
}

export default function ConnectionControls({ connected = false }: { connected?: boolean }) {
  const { t } = useT();
  const { control, busy, error, change } = useConnectionControl();
  const [picking, setPicking] = useState(false);
  useEffect(() => { if (control?.pairing_enabled) setPicking(false); }, [control?.pairing_enabled]);
  return <div className="mt-5 pt-5 border-t border-ds-border">
    <p className="text-xs text-ds-text-muted leading-relaxed">{t(control?.paused ? "connections_paused_hint" : "connections_description")}</p>
    <div className="mt-4 flex flex-col gap-2">
      <button disabled={!control || busy} className={control?.paused ? "button-primary" : "button-secondary"}
        onClick={() => change("set_connections_paused", { paused: !control?.paused })}>
        {t(control?.paused ? "connections_resume" : "connections_pause")}
      </button>
      <button disabled={!control || control.paused || busy} className={connected || control?.pairing_enabled || control?.paused ? "button-secondary" : "button-primary"}
        onClick={() => control?.pairing_enabled ? change("set_pairing_enabled", { enabled: false }) : setPicking(true)}>
        {t(control?.pairing_enabled ? "pair_stop" : "pair_start")}
      </button>
    </div>
    {control?.pairing_enabled ? <p role="status" className="mt-2 text-xs text-ds-accent">{t("pair_window")}</p> : null}
    {error && !picking ? <p role="alert" className="mt-2 text-sm text-ds-danger">{error}</p> : null}
    {picking && <DevicePicker connected={connected} onClose={() => setPicking(false)} />}
  </div>;
}
