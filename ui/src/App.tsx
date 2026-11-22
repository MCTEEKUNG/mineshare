import { useEffect, useState, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import FilesPage from "./pages/Files";
import HomePage from "./pages/Home";
import AudioDevicesPage from "./pages/AudioDevices";
import SettingsPage from "./pages/Settings";
import PairingModal from "./PairingModal";
import LinuxSetup from "./LinuxSetup";
import { useConnectionControl } from "./connectionState";
import WorkspaceIcon from "./components/WorkspaceIcon";
import { LanguageToggle, useT } from "./i18n";
import appIcon from "./assets/mineshare-app-icon.svg";
import {
  IconVolume, IconFile, IconSliders,
  IconArrowDown, IconArrowUpRight, IconArrowDownLeft,
  IconX, IconAlertTriangle,
} from "./icons";
import type { Status, Latency, Transfer, ToastEntry } from "./types";

type Tab = "home" | "audio_devices" | "files" | "settings";

export default function App() {
  const [tab, setTab] = useState<Tab>("home");
  const [status, setStatus] = useState<Status | null>(null);
  const [latency, setLatency] = useState<Latency | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dropOverlay, setDropOverlay] = useState(false);
  const [toasts, setToasts] = useState<ToastEntry[]>([]);
  const [activeTransfers, setActiveTransfers] = useState(0);
  const { t } = useT();
  const { control } = useConnectionControl();
  const paused = control?.paused ?? false;

  // Status poll — paused when the window is hidden to tray so
  // we don't churn IPC + React renders for an invisible UI. The
  // WebView keeps running JS even when minimised, so without this
  // every minute on the tray costs us 60 invoke round-trips for
  // nothing. `visibilitychange` re-arms the timer on restore.
  useEffect(() => {
    let id: ReturnType<typeof setInterval> | undefined;
    const tick = () =>
      invoke<Status>("get_status")
        .then((s) => {
          setStatus(s);
          setError(null);
        })
        .catch((e) => setError(String(e)));
    const start = () => {
      if (id !== undefined) return;
      tick();
      id = setInterval(tick, 1000);
    };
    const stop = () => {
      if (id !== undefined) {
        clearInterval(id);
        id = undefined;
      }
    };
    const onVis = () => {
      if (document.hidden) stop();
      else start();
    };
    if (!document.hidden) start();
    document.addEventListener("visibilitychange", onVis);
    return () => {
      document.removeEventListener("visibilitychange", onVis);
      stop();
    };
  }, []);

  // Latency / RTT histogram — only polled while the Home tab is
  // active AND the window is visible (same hide-to-tray rationale
  // as `get_status` above). The daemon's ping task fires every
  // 500 ms so a 1 s GUI poll is plenty to keep the bars live.
  useEffect(() => {
    if (tab !== "home") return;
    let id: ReturnType<typeof setInterval> | undefined;
    const tick = () =>
      invoke<Latency>("get_latency").then(setLatency).catch(() => {});
    const start = () => {
      if (id !== undefined) return;
      tick();
      id = setInterval(tick, 1000);
    };
    const stop = () => {
      if (id !== undefined) {
        clearInterval(id);
        id = undefined;
      }
    };
    const onVis = () => {
      if (document.hidden) stop();
      else start();
    };
    if (!document.hidden) start();
    document.addEventListener("visibilitychange", onVis);
    return () => {
      document.removeEventListener("visibilitychange", onVis);
      stop();
    };
  }, [tab]);

  // GLOBAL native drag-drop. Lives at the App level (not in
  // FilesPage) so users can drop a file on ANY tab and it just
  // works — the killer-feature flow is "drag, drop, done".
  // Tauri 2 emits these as webview-scoped events, so we listen
  // via `getCurrentWebview().onDragDropEvent` rather than the
  // global event bus (which silently doesn't deliver them).
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    try { getCurrentWebview() } catch { return; }
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setDropOverlay(true);
        } else if (event.payload.type === "leave") {
          setDropOverlay(false);
        } else if (event.payload.type === "drop") {
          setDropOverlay(false);
          for (const p of event.payload.paths) {
            invoke("send_file", { path: p }).catch((e) => {
              setError(String(e));
            });
          }
          // Auto-jump to Files tab so users immediately see the
          // progress bar of what they just kicked off.
          setTab("files");
        }
      })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // Watch transfers for completed / failed ones the user hasn't
  // seen yet → pop a toast for each. Tracking via a ref of "last
  // seen ids" so completed transfers only toast once even though
  // we poll forever. Paused when the window is hidden to tray (same
  // rationale as the get_status/get_latency polls above) so we don't
  // churn IPC + React renders every 700 ms for an invisible UI.
  const seenTerminalIds = useRef<Set<number>>(new Set());
  useEffect(() => {
    let id: ReturnType<typeof setInterval> | undefined;
    const tick = async () => {
      try {
        const list = await invoke<Transfer[]>("get_transfers");
        const inFlight = list.filter((tt) =>
          ["pending", "active", "verifying"].includes(tt.status),
        ).length;
        setActiveTransfers(inFlight);

        const terminal = list.filter(
          (tt) =>
            (tt.status === "done" ||
              tt.status === "failed" ||
              tt.status === "cancelled") &&
            !seenTerminalIds.current.has(tt.id),
        );
        if (terminal.length > 0) {
          for (const tt of terminal) {
            seenTerminalIds.current.add(tt.id);
          }
          setToasts((prev) => [
            ...prev,
            ...terminal.map((tt) => ({
              id: tt.id,
              direction: tt.direction,
              status: tt.status as "done" | "failed" | "cancelled",
              name: tt.name,
              final_path: tt.final_path,
            })),
          ]);
        }
      } catch {
        /* daemon not ready yet */
      }
    };
    const start = () => {
      if (id !== undefined) return;
      tick();
      id = setInterval(tick, 700);
    };
    const stop = () => {
      if (id !== undefined) {
        clearInterval(id);
        id = undefined;
      }
    };
    const onVis = () => {
      if (document.hidden) stop();
      else start();
    };
    if (!document.hidden) start();
    document.addEventListener("visibilitychange", onVis);
    return () => {
      document.removeEventListener("visibilitychange", onVis);
      stop();
    };
  }, []);

  function dismissToast(id: number) {
    setToasts((prev) => prev.filter((tt) => tt.id !== id));
  }

  const tabTitle = t(`nav_${tab}`);

  return (
    <div className="app-shell">
      <PairingModal />
      <LinuxSetup />
      <DropOverlay visible={dropOverlay} />
      <ToastStack toasts={toasts} onDismiss={dismissToast} />

      {/* Sidebar */}
      <aside className="app-sidebar">
        {/* Logo */}
        <div className="px-2 mb-8 mt-3">
          <div className="flex items-center gap-2.5">
            <img src={appIcon} alt="" className="size-7 shrink-0" />
            <div>
              <span className="block text-lg font-semibold text-ds-text tracking-tight">MineShare</span>
            </div>
          </div>
        </div>

        {/* Navigation */}
        <nav aria-label={t("nav_home")} className="flex-1 flex flex-col gap-1">
          <NavItem active={tab === "home"} onClick={() => setTab("home")} icon={<WorkspaceIcon kind="home" className="size-4" />}>
            {t("nav_home")}
          </NavItem>
          <NavItem active={tab === "audio_devices"} onClick={() => setTab("audio_devices")} icon={<IconVolume className="size-4" />}>
            {t("nav_audio_devices")}
          </NavItem>
          <NavItem active={tab === "files"} onClick={() => setTab("files")} icon={<IconFile className="size-4" />} badge={activeTransfers > 0 ? activeTransfers : undefined}>
            {t("nav_files")}
          </NavItem>
          <NavItem active={tab === "settings"} onClick={() => setTab("settings")} icon={<IconSliders className="size-4" />}>
            {t("nav_settings")}
          </NavItem>
        </nav>

        {/* Footer */}
        <div className="sidebar-session mb-5">
          <span className="flex items-center gap-2 text-xs"><span className="status-dot" style={paused || !status?.peer_connected ? { background: "var(--ds-text-muted)" } : undefined} />{t(paused ? "sidebar_paused" : status?.peer_connected ? "sidebar_connected" : "sidebar_waiting")}</span>
          <p className="text-xs text-ds-text-muted leading-relaxed mt-2">{t(paused ? "connections_paused_hint" : "sidebar_hint")}</p>
        </div>
        <div className="px-2 pt-4 border-t border-ds-border">
          <div className="flex items-center justify-between"><LanguageToggle /><span className="text-[11px] text-ds-text-muted">v{status?.local_version.split(" · ")[0] || "…"}</span></div>
          <VersionFooter status={status} />
        </div>
      </aside>

      {/* Main content */}
      <main className="app-main" key={tab}>
        <div className="page-content">
          <header className="mb-7">
            <div className="flex items-center justify-between gap-4"><h1 className="page-heading">{tab === "home" ? t("home_headline") : tabTitle}</h1><ConnectionPill status={status} error={error} paused={paused} /></div>
            <p className="page-subtitle">{t(tab === "home" ? "home_subtitle" : tab === "audio_devices" ? "audio_subtitle" : `${tab}_subtitle`)}</p>
          </header>

          {tab === "home" ? (
            status ? <HomePage status={status} latency={latency} onNavigate={setTab} /> : <div role="status" className="panel p-6"><p className="font-medium">{t(error ? "workspace_unavailable" : "loading_workspace")}</p>{error ? <p className="muted mt-2">{t("workspace_unavailable_hint")}</p> : null}</div>
          ) : null}
          {tab === "audio_devices" ? <AudioDevicesPage /> : null}
          {tab === "files" ? <FilesPage /> : null}
          {tab === "settings" ? <SettingsPage status={status} /> : null}
        </div>
      </main>
    </div>
  );
}

function VersionFooter({ status }: { status: Status | null }) {
  const { t } = useT();
  const local = status?.local_version ?? "";
  const peer = status?.peer_version ?? null;
  const connected = status?.peer_connected ?? false;
  const same = connected && peer != null && peer === local;
  return (
    <div className="mt-3 text-[10px] leading-relaxed text-ds-text-muted select-text break-words">
      {connected && peer && !same ? <p className="text-ds-warning mb-2">⚠ {t("ver_diff_build")}</p> : null}
      <details><summary className="cursor-pointer hover:text-ds-text">{t("build_details")}</summary>
      <p className="mt-2 break-all">MineShare {local || "…"}</p>
      {connected && peer ? (
        same ? (
          <div className="text-ds-success">✓ {t("ver_same_build")}</div>
        ) : (
          <div className="mt-1 break-all" title={peer}>
            {t("ver_peer")}: {peer}
          </div>
        )
      ) : null}</details>
    </div>
  );
}

function DropOverlay({ visible }: { visible: boolean }) {
  const { t } = useT();
  if (!visible) return null;
  return (
    <div className="fixed inset-0 z-40 pointer-events-none flex items-center justify-center bg-ds-accent-soft">
      <div className="rounded-2xl border-2 border-dashed border-ds-accent-border bg-ds-surface px-12 py-10 shadow-2xl flex flex-col items-center gap-4">
        <div className="size-14 rounded-full bg-ds-accent-soft flex items-center justify-center">
          <IconArrowDown className="size-7 text-ds-accent" />
        </div>
        <div className="text-center">
          <p className="text-base font-semibold text-ds-accent">{t("drop_title")}</p>
          <p className="text-xs text-ds-text-muted mt-1">{t("drop_hint")}</p>
        </div>
      </div>
    </div>
  );
}

function ToastStack({
  toasts,
  onDismiss,
}: {
  toasts: ToastEntry[];
  onDismiss: (id: number) => void;
}) {
  return (
    <div className="fixed top-4 right-4 z-40 flex flex-col gap-2 max-w-sm">
      {toasts.map((t) => (
        <Toast key={t.id} entry={t} onDismiss={() => onDismiss(t.id)} />
      ))}
    </div>
  );
}

function Toast({
  entry,
  onDismiss,
}: {
  entry: ToastEntry;
  onDismiss: () => void;
}) {
  useEffect(() => {
    const id = setTimeout(onDismiss, 6_000);
    return () => clearTimeout(id);
  }, [onDismiss]);

  const [shown, setShown] = useState(false);
  useEffect(() => {
    const id = requestAnimationFrame(() => setShown(true));
    return () => cancelAnimationFrame(id);
  }, []);

  const tone =
    entry.status === "done"
      ? entry.direction === "sending"
        ? "border-ds-success-border bg-ds-success-soft"
        : "border-ds-info-border bg-ds-info-soft"
      : entry.status === "cancelled"
        ? "border-ds-border bg-ds-surface"
        : "border-ds-danger-border bg-ds-danger-soft";

  const iconEl =
    entry.status === "done" ? (
      entry.direction === "sending" ? (
        <IconArrowUpRight className="size-4 text-ds-success" />
      ) : (
        <IconArrowDownLeft className="size-4 text-ds-info" />
      )
    ) : entry.status === "cancelled" ? (
      <IconX className="size-4 text-ds-text-muted" />
    ) : (
      <IconAlertTriangle className="size-4 text-ds-danger" />
    );

  const title =
    entry.status === "done"
      ? entry.direction === "sending"
        ? "Sent to peer"
        : "Received from peer"
      : entry.status === "cancelled"
        ? "Cancelled"
        : "Failed";

  const canOpen = entry.status === "done" && entry.direction === "receiving";
  const onClick = () => {
    if (canOpen) invoke("open_downloads_dir").catch(() => {});
    onDismiss();
  };

  return (
    <div
      onClick={onClick}
      className={
        "rounded-xl border p-3 shadow-xl cursor-pointer transition-all duration-200 " +
        tone +
        (shown ? " translate-x-0 opacity-100" : " translate-x-full opacity-0")
      }
    >
      <div className="flex items-start gap-2.5">
        <span className="shrink-0 mt-0.5">{iconEl}</span>
        <div className="min-w-0 flex-1">
          <p className="text-xs font-semibold text-ds-text">{title}</p>
          <p className="text-sm font-medium truncate text-ds-text">{entry.name}</p>
          {canOpen ? (
            <p className="text-[11px] text-ds-text-muted mt-0.5">Click to open Downloads/MineShare</p>
          ) : null}
        </div>
        <button
          onClick={(e) => { e.stopPropagation(); onDismiss(); }}
          aria-label="Dismiss"
          className="text-ds-text-muted hover:text-ds-text transition-colors text-xs px-1 shrink-0"
        >
          <IconX className="size-3" />
        </button>
      </div>
    </div>
  );
}

function ConnectionPill({
  status,
  error,
  paused,
}: {
  status: Status | null;
  error: string | null;
  paused: boolean;
}) {
  const { t } = useT();
  const connected = Boolean(status?.peer_connected && !paused && !error);
  return (
    <span className="quiet-badge shrink-0 whitespace-nowrap" role="status">
      <span className="status-dot" aria-hidden="true" style={!connected ? { background: "var(--ds-text-muted)" } : undefined} />
      {t(connected ? "workspace_connected" : "workspace_disconnected")}
    </span>
  );
}

function NavItem({
  children,
  active,
  onClick,
  icon,
  badge,
}: {
  children: React.ReactNode;
  active?: boolean;
  onClick?: () => void;
  icon: React.ReactNode;
  badge?: number;
}) {
  return (
    <button
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      className={
        "relative w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-left transition-all duration-150 text-sm group " +
        (active
          ? "bg-ds-accent-soft text-ds-accent font-medium"
          : "text-ds-text-muted hover:bg-ds-hover hover:text-ds-text")
      }
    >
      <span className={active ? "text-ds-accent" : "group-hover:text-ds-text transition-colors"}>
        {icon}
      </span>
      <span className="flex-1">{children}</span>
      {badge ? (
        <span className="text-[10px] bg-ds-accent text-ds-on-accent rounded-full min-w-[18px] h-[18px] flex items-center justify-center px-1 font-medium">
          {badge}
        </span>
      ) : null}
    </button>
  );
}
