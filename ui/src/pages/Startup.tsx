import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "../i18n";

type StartupState = { enabled: boolean; managed: boolean };

export default function StartupSection() {
  const { t } = useT();
  const [state, setState] = useState<StartupState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    invoke<StartupState>("get_startup_status")
      .then(value => { if (active) setState(value); })
      .catch(e => { if (active) setError(String(e)); });
    return () => { active = false; };
  }, []);
  async function change(enabled: boolean) {
    setBusy(true);
    setError(null);
    try { setState(await invoke<StartupState>("set_startup_enabled", { enabled })); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  return <section className="rounded-xl border border-ds-border bg-ds-surface p-5">
    <h3 className="text-sm font-semibold text-ds-text">{t("startup_title")}</h3>
    <p className="text-xs text-ds-text-muted mt-2">{t("startup_description")}</p>
    {state ? <label className="flex items-center gap-3 mt-4 text-sm text-ds-text">
      <input type="checkbox" className="[accent-color:var(--ds-accent)]" checked={state.enabled}
        disabled={busy || state.managed} onChange={e => change(e.target.checked)} />
      {t("startup_enabled")}
    </label> : !error ? <p className="text-xs text-ds-text-muted mt-3">{t("startup_loading")}</p> : null}
    {state?.managed ? <p className="text-xs text-ds-text-muted mt-3">{t("startup_managed")}</p> : null}
    {error ? <p role="alert" className="text-xs text-red-400 mt-3">{error}</p> : null}
  </section>;
}
