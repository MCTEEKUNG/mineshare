import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import HotkeysPage from "./Hotkeys";
import AdvancedPage, { InputPrefsCard } from "./Advanced";
import { useT } from "../i18n";
import { ThemeSwitcher } from "../theme";
import LockEffectSection from "./LockEffect";
import StartupSection from "./Startup";
import LayoutPage from "./Layout";
import Disclosure from "../components/Disclosure";
import type { Status } from "../types";
import { useConnectionControl } from "../connectionState";

export default function SettingsPage({ status }: { status: Status | null }) {
  const { t } = useT();
  const { control } = useConnectionControl();
  return <div className="settings-stack">
    <section className="panel p-5"><h2 className="section-title">{t("settings_screens")}</h2><LayoutPage peerName={(status?.peer_connected && !control?.paused ? status.peer_name : control?.selected_peer?.display_name) ?? undefined} connected={status?.peer_connected && !control?.paused} status={status} /></section>
    <section className="panel p-5 flex flex-wrap justify-between items-center gap-4">
      <h2 className="text-sm font-semibold">{t("section_appearance")}</h2><ThemeSwitcher />
    </section>
    <StartupSection />
    <Disclosure title={t("settings_input")}><InputPrefsCard /></Disclosure>
    <Disclosure title={t("settings_games")}><ManualControls status={status} /></Disclosure>
    <Disclosure title={t("section_hotkeys")}><HotkeysPage /></Disclosure>
    <Disclosure title={t("settings_effect")}><LockEffectSection /></Disclosure>
    <Disclosure title={t("settings_diagnostics")}>
      <LinuxSetupButton />
      <div className="mb-5 text-xs leading-relaxed break-all"><p>{status?.local_version}</p><p className="mt-2 text-ds-text-muted">{t("ver_peer")}: {status?.peer_version ?? "—"}</p><p className="mt-2 text-ds-text-muted">{t("workspace_network")}: {status?.peer_addr ?? "—"}</p></div>
      <AdvancedPage />
    </Disclosure>
  </div>;
}

function LinuxSetupButton() {
  const [linux, setLinux] = useState(false);
  const { locale } = useT();
  useEffect(() => { invoke<{linux: boolean}>("get_linux_setup_status").then(s => setLinux(s.linux)).catch(() => {}); }, []);
  if (!linux) return null;
  return <button className="button-secondary mb-5" onClick={() => window.dispatchEvent(new Event("mineshare-linux-setup"))}>{locale === "th" ? "ตรวจการตั้งค่า Linux" : "Check Linux setup"}</button>;
}

function ManualControls({ status }: { status: Status | null }) {
  const { t } = useT();
  const { control } = useConnectionControl();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  async function command(name: string, args?: Record<string, unknown>) {
    setBusy(true); setError("");
    try { await invoke(name, args); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  return <fieldset disabled={!status || busy} className="space-y-5">
    <div>
      <label className="block text-sm font-medium" htmlFor="keyboard-target">{t("settings_keyboard")}</label>
      <p className="muted mt-2 mb-3">{t("settings_keyboard_hint")}</p>
      <select id="keyboard-target" className="w-full rounded-xl border border-ds-border bg-ds-elevated px-3 py-2.5 text-sm" value={status?.keyboard_target ?? "smart"} onChange={e => command("set_keyboard_target", { target: e.target.value })}>
        {["smart", "auto", "force_peer", "force_local"].map(target => <option disabled={target === "force_peer" && (!status?.peer_connected || control?.paused)} value={target} key={target}>{t(`keyboard_${target}`)}</option>)}
      </select>
    </div>
    <div className="pt-5 border-t border-ds-border">
      <p className="text-sm font-medium">{t("settings_game_lock")}</p><p className="muted mt-2">{t("settings_game_lock_hint")}</p>
      <button className="button-secondary mt-3" onClick={() => command("set_input_lock", { locked: !status?.input_locked })}>{t(status?.input_locked ? "action_unlock" : "action_lock")}</button>
      {status?.remote_input_locked && <p className="muted mt-3">{t("shortcut_lock_hint")} <kbd>Ctrl + Shift + L</kbd></p>}
    </div>
    <div className="pt-5 border-t border-ds-border">
      <p className="text-sm font-medium">Game Drive</p><p className="muted mt-2">{t("settings_game_drive_hint")}</p>
      <button className="button-secondary mt-3" disabled={!status?.peer_connected || control?.paused} onClick={() => command("toggle_game_drive")}>{t(status?.game_drive === "off" ? "action_start" : "action_stop")}</button>
    </div>
    {error && <p role="alert" className="text-sm text-ds-danger">{error}</p>}
  </fieldset>;
}
