import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "../i18n";

type UpdateInfo = { current: string; latest: string; available: boolean; installable: boolean; page: string };
type Phase = "idle" | "checking" | "installing";

export default function UpdateSection() {
  const { t } = useT();
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);

  async function check() {
    setPhase("checking"); setError(null);
    try { setInfo(await invoke<UpdateInfo>("check_for_update")); }
    catch (e) { setError(String(e)); }
    finally { setPhase("idle"); }
  }
  async function install() {
    setPhase("installing"); setError(null);
    try { await invoke("install_update"); }
    catch (e) { setError(String(e)); setPhase("idle"); }
  }

  const busy = phase !== "idle";
  return <section className="panel p-5">
    <h3 className="text-sm font-semibold text-ds-text">{t("update_title")}</h3>
    <p className="text-xs text-ds-text-muted mt-2">{t("update_current")}: {info?.current ?? "—"}</p>
    <div className="mt-4 flex flex-wrap items-center gap-3">
      <button className="button-secondary" disabled={busy} onClick={check}>
        {phase === "checking" ? t("update_checking") : t("update_check")}
      </button>
      {info?.available && info.installable
        ? <button className="button-primary" disabled={busy} onClick={install}>{t("update_install")}</button> : null}
    </div>
    {info && !info.available ? <p role="status" className="text-xs text-ds-text-muted mt-3">{t("update_latest")}</p> : null}
    {info?.available ? <p role="status" className="text-xs text-ds-text mt-3">{t("update_available").replace("{v}", info.latest)}</p> : null}
    {info?.available && !info.installable
      ? <p className="text-xs text-ds-text-muted mt-2 break-all">{t("update_manual")}: {info.page}</p> : null}
    {phase === "installing" ? <p role="status" className="text-xs text-ds-text-muted mt-3">{t("update_installing")}</p> : null}
    {error ? <p role="alert" className="text-xs text-ds-danger mt-3">{t("update_failed")}{error}</p> : null}
  </section>;
}
