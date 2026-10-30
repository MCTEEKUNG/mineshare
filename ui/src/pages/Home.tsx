import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "../i18n";
import type { Status, Latency } from "../types";
import ConnectionControls from "../ConnectionControls";
import { selectedPeerLabel, useConnectionControl } from "../connectionState";
import WorkspaceIcon from "../components/WorkspaceIcon";
import { IconArrowUpRight, IconFile, IconVolume } from "../icons";
import { ShortcutRows } from "./Hotkeys";
import LayoutPage from "./Layout";

export default function HomePage({ status, latency, onNavigate }: {
  status: Status; latency: Latency | null; onNavigate: (tab: "audio_devices" | "files" | "settings") => void;
}) {
  const { t } = useT();
  const { control, busy } = useConnectionControl();
  const [restoring, setRestoring] = useState(false);
  const [error, setError] = useState("");
  async function restoreSmart() {
    setRestoring(true); setError("");
    try { await invoke("set_keyboard_target", { target: "smart" }); }
    catch (e) { setError(String(e)); }
    finally { setRestoring(false); }
  }
  const paused = control?.paused ?? false;
  const connected = status.peer_connected && !paused;
  const peer = (connected ? status.peer_name : control?.selected_peer?.display_name) ?? t("workspace_peer");
  const keyboardOverride = status.keyboard_target === "force_peer" || status.keyboard_target === "force_local";
  const ms = latency?.p50_ms ?? latency?.last_ms;
  const hasLatency = connected && ms != null && Number.isFinite(ms) && ms >= 0;
  const cursor = !connected ? t("workspace_local") : status.local_in_remote ? peer : status.peer_in_remote ? t("workspace_peer_driving") : t("workspace_local");

  return <div className="home-grid">
    <div className="workspace-surface space-y-6">
      {status.anticheat_warning ? <div role="status" className="panel p-5 text-sm">
        <p className="font-medium text-ds-warning">{status.anticheat_warning}</p>
        <p className="muted mt-1">{t("ac_desc")}</p>
      </div> : null}
      <section className="panel p-5" aria-labelledby="workspace-title">
        <h2 id="workspace-title" className="section-title">{t("workspace_title")}</h2>
        <LayoutPage peerName={peer} connected={connected} status={status} />
        {!connected && control?.selected_peer && <div className="mt-4 text-xs leading-relaxed">
          <p role="status" className="break-words">{t(paused || control.pairing_enabled ? "workspace_selected_computer" : "workspace_waiting_for")} {selectedPeerLabel(control.selected_peer)}</p>
          <p className="mt-1 text-ds-text-muted">{t("workspace_selected_hint")}</p>
        </div>}
      </section>

      <section className="panel px-5 pt-5 pb-1" aria-labelledby="workspace-controls-title">
        <h2 id="workspace-controls-title" className="text-sm font-semibold">{t("workspace_controls")}</h2>
        <p className="text-xs text-ds-text-muted mt-2 mb-1 leading-relaxed" role="status">
          {paused ? t("connections_paused") : !connected ? t("workspace_waiting") : `${t("workspace_cursor")} ${cursor}`}
        </p>
        {(status.input_locked || status.remote_input_locked || status.game_drive !== "off" || keyboardOverride) && <div className="mb-4 rounded-xl bg-ds-hover p-3 text-xs leading-relaxed">
          <p>{keyboardOverride ? t("workspace_keyboard_pinned") : status.game_drive !== "off" ? t("workspace_game_active") : t("workspace_locked")}</p>
          <button disabled={busy || restoring} className="text-link mt-2 underline underline-offset-4" onClick={() => keyboardOverride ? restoreSmart() : onNavigate("settings")}>{keyboardOverride ? t("workspace_restore_smart") : t("workspace_manage")}</button>
          {error && <p role="alert" className="mt-2 text-ds-danger">{error}</p>}
        </div>}
        <ShortcutRows compact />
      </section>

      <section aria-labelledby="workflow-title">
        <h2 id="workflow-title" className="section-title">{t("workspace_everyday")}</h2>
        <div className="feature-grid">
          <article className="feature-card">
            <WorkspaceIcon kind="copy" className="size-6 mb-5 text-ds-accent" />
            <h3 className="text-sm font-semibold">{t("workspace_clipboard")}</h3>
            <p>{t("workspace_clipboard_hint")}</p>
            <span className="mt-auto pt-5 text-[11px] text-ds-text-muted">{t("workspace_automatic")}</span>
          </article>
          <button className="feature-card hover:bg-ds-elevated transition-colors group" onClick={() => onNavigate("audio_devices")}>
            <IconVolume className="size-6 mb-5 text-ds-accent" />
            <span className="text-sm font-semibold">{t("workspace_audio")}</span>
            <span className="feature-description">{t("workspace_audio_hint")}</span>
            <span className="mt-auto pt-5 text-[11px] text-ds-text-muted inline-flex items-center gap-1">{t("workspace_manage")} <IconArrowUpRight className="size-3" /></span>
          </button>
          <button className="feature-card hover:bg-ds-elevated transition-colors" onClick={() => onNavigate("files")}>
            <IconFile className="size-6 mb-5 text-ds-accent" />
            <span className="text-sm font-semibold">{t("workspace_files")}</span>
            <span className="feature-description">{t("workspace_files_hint")}</span>
            <span className="mt-auto pt-5 text-[11px] text-ds-text-muted inline-flex items-center gap-1">{t("workspace_view_files")} <IconArrowUpRight className="size-3" /></span>
          </button>
        </div>
      </section>
    </div>

    <aside className="home-aside space-y-4">
      <section className="panel p-5" aria-labelledby="connection-title">
        <h2 id="connection-title" className="text-sm font-semibold mb-5">{t("workspace_latency")}</h2>
        <p className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
          <span className="text-5xl font-semibold tracking-tight tabular-nums text-ds-accent">{hasLatency ? ms.toFixed(1) : "—"}</span>
          {hasLatency && <span className="text-lg text-ds-text-muted">ms</span>}
        </p>
        {!hasLatency && <p className="muted mt-3" role="status">{t(paused ? "connections_paused" : connected ? "workspace_measuring" : "workspace_reconnecting")}</p>}
        <ConnectionControls connected={connected} />
      </section>
      <section className="rounded-[22px] bg-ds-hover p-5">
        <WorkspaceIcon kind="shield" className="size-5 mb-3 text-ds-accent" />
        <h2 className="text-sm font-semibold">{t("workspace_private")}</h2>
        <p className="mt-2 text-xs text-ds-text-muted leading-relaxed">{t("workspace_private_hint")}</p>
      </section>
    </aside>
  </div>;
}
