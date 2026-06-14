import { useEffect, useState, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import LayoutPage from "./pages/Layout";
import FilesPage from "./pages/Files";
import HomePage from "./pages/Home";
import AudioDevicesPage from "./pages/AudioDevices";
import SettingsPage from "./pages/Settings";
import PairingModal from "./PairingModal";
import { LanguageToggle, useT } from "./i18n";
import {
  IconActivity, IconGrid, IconVolume, IconFile, IconSliders,
  IconArrowDown, IconArrowUp, IconArrowUpRight, IconArrowDownLeft,
  IconX, IconAlertTriangle,
} from "./icons";
import type { Status, Latency, Transfer, ToastEntry } from "./types";

type Tab = "home" | "layout" | "audio_devices" | "files" | "settings";

export default function App() {
  const [tab, setTab] = useState<Tab>("home");
  const [status, setStatus] = useState<Status | null>(null);
  const [latency, setLatency] = useState<Latency | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dropOverlay, setDropOverlay] = useState(false);
  const [toasts, setToasts] = useState<ToastEntry[]>([]);
  const [activeTransfers, setActiveTransfers] = useState(0);
  const { t } = useT();

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
    <div className="h-screen flex bg-ds-bg text-slate-100 overflow-hidden">
      <PairingModal />
      <DropOverlay visible={dropOverlay} />
      <ToastStack toasts={toasts} onDismiss={dismissToast} />
      <ActiveTransfersBadge count={activeTransfers} onClick={() => setTab("files")} />

      {/* Sidebar */}
      <aside className="w-[220px] shrink-0 bg-ds-sidebar border-r border-white/[0.06] flex flex-col py-5">
        {/* Logo */}
        <div className="px-4 mb-7">
          <div className="flex items-center gap-2.5">
            <div className="size-7 rounded-lg bg-emerald-500/20 border border-emerald-500/30 flex items-center justify-center shrink-0">
              <IconActivity className="size-3.5 text-emerald-400" />
            </div>
            <span className="text-sm font-semibold text-white tracking-tight">MineShare</span>
          </div>
        </div>

        {/* Navigation */}
        <nav className="flex-1 px-2 flex flex-col gap-0.5">
          <NavItem active={tab === "home"} onClick={() => setTab("home")} icon={<IconActivity className="size-4" />}>
            {t("nav_home")}
          </NavItem>
          <NavItem active={tab === "layout"} onClick={() => setTab("layout")} icon={<IconGrid className="size-4" />}>
            {t("nav_layout")}
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
        <div className="px-4 pt-4 border-t border-white/[0.06]">
          <LanguageToggle />
          <VersionFooter status={status} />
        </div>
      </aside>

      {/* Main content */}
      <main className="flex-1 overflow-y-auto">
        <div className="px-8 py-6">
          <header className="mb-6 flex items-center justify-between">
            <h2 className="text-xl font-semibold text-white">{tabTitle}</h2>
            <ConnectionPill status={status} error={error} />
          </header>

          {tab === "home" && status ? <HomePage status={status} latency={latency} /> : null}
          {tab === "layout" ? <LayoutPage /> : null}
          {tab === "audio_devices" ? <AudioDevicesPage /> : null}
          {tab === "files" ? <FilesPage /> : null}
          {tab === "settings" ? <SettingsPage /> : null}
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
    <div className="mt-3 text-[10px] leading-snug text-slate-400 select-text break-all">
      <div title={local}>MineShare {local || "…"}</div>
      {connected && peer ? (
        same ? (
          <div className="text-emerald-500">✓ {t("ver_same_build")}</div>
        ) : (
          <div className="text-amber-500" title={peer}>
            ⚠ {t("ver_diff_build")} ({t("ver_peer")}: {peer})
          </div>
        )
      ) : null}
    </div>
  );
}

function DropOverlay({ visible }: { visible: boolean }) {
  if (!visible) return null;
  return (
    <div className="fixed inset-0 z-40 pointer-events-none flex items-center justify-center bg-emerald-500/10 backdrop-blur-sm">
      <div className="rounded-2xl border-2 border-dashed border-emerald-400/60 bg-ds-surface/95 px-12 py-10 shadow-2xl flex flex-col items-center gap-4">
        <div className="size-14 rounded-full bg-emerald-500/20 flex items-center justify-center">
          <IconArrowDown className="size-7 text-emerald-400" />
        </div>
        <div className="text-center">
          <p className="text-base font-semibold text-emerald-300">Drop to send to peer</p>
          <p className="text-xs text-slate-400 mt-1">Encrypted · auto-saves to Downloads/MineShare</p>
        </div>
      </div>
    </div>
  );
}

function ActiveTransfersBadge({
  count,
  onClick,
}: {
  count: number;
  onClick: () => void;
}) {
  if (count === 0) return null;
  return (
    <button
      onClick={onClick}
      className="fixed bottom-4 left-4 z-30 flex items-center gap-2 rounded-full bg-emerald-500 hover:bg-emerald-400 text-white px-3 py-1.5 shadow-lg shadow-emerald-500/25 text-xs font-medium transition-all duration-150"
    >
      <span className="inline-block size-1.5 rounded-full bg-white animate-pulse" />
      <IconArrowUp className="size-3.5" />
      {count} transfer{count === 1 ? "" : "s"}
    </button>
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
        ? "border-emerald-500/30 bg-emerald-500/10"
        : "border-blue-500/30 bg-blue-500/10"
      : entry.status === "cancelled"
        ? "border-white/[0.08] bg-ds-surface"
        : "border-red-500/30 bg-red-500/10";

  const iconEl =
    entry.status === "done" ? (
      entry.direction === "sending" ? (
        <IconArrowUpRight className="size-4 text-emerald-400" />
      ) : (
        <IconArrowDownLeft className="size-4 text-blue-400" />
      )
    ) : entry.status === "cancelled" ? (
      <IconX className="size-4 text-slate-400" />
    ) : (
      <IconAlertTriangle className="size-4 text-red-400" />
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
          <p className="text-xs font-semibold text-slate-200">{title}</p>
          <p className="text-sm font-medium truncate text-slate-100">{entry.name}</p>
          {canOpen ? (
            <p className="text-[11px] text-slate-400 mt-0.5">Click to open Downloads/MineShare</p>
          ) : null}
        </div>
        <button
          onClick={(e) => { e.stopPropagation(); onDismiss(); }}
          aria-label="Dismiss"
          className="text-slate-500 hover:text-slate-200 transition-colors text-xs px-1 shrink-0"
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
}: {
  status: Status | null;
  error: string | null;
}) {
  const { t } = useT();
  if (error) {
    return (
      <span className="inline-flex items-center gap-1.5 text-xs text-red-400 bg-red-500/10 border border-red-500/25 rounded-full px-2.5 py-1">
        <span className="size-1.5 rounded-full bg-red-400" />
        {t("conn_offline")}
      </span>
    );
  }
  if (!status) {
    return (
      <span className="inline-flex items-center gap-1.5 text-xs text-slate-400">
        <span className="size-1.5 rounded-full bg-slate-500 animate-pulse" />
        {t("conn_connecting")}
      </span>
    );
  }
  if (!status.peer_connected) {
    return (
      <span className="inline-flex items-center gap-1.5 text-xs text-slate-400 bg-white/[0.04] border border-white/[0.06] rounded-full px-2.5 py-1">
        <span className="size-1.5 rounded-full bg-slate-500" />
        {t("conn_no_peer")}
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-1.5 text-xs text-emerald-400 bg-emerald-500/10 border border-emerald-500/25 rounded-full px-2.5 py-1">
      <span className="size-1.5 rounded-full bg-emerald-400 animate-pulse" />
      {t("conn_paired_with")} {status.peer_addr}
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
      className={
        "relative w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-left transition-all duration-150 text-sm group " +
        (active
          ? "bg-white/[0.08] text-white font-medium"
          : "text-slate-400 hover:bg-white/[0.04] hover:text-slate-200")
      }
    >
      {active && (
        <span className="absolute left-0 top-1/2 -translate-y-1/2 w-0.5 h-5 bg-emerald-400 rounded-r-full" />
      )}
      <span className={active ? "text-emerald-400" : "group-hover:text-slate-300 transition-colors"}>
        {icon}
      </span>
      <span className="flex-1">{children}</span>
      {badge ? (
        <span className="text-[10px] bg-emerald-500 text-white rounded-full min-w-[18px] h-[18px] flex items-center justify-center px-1 font-medium">
          {badge}
        </span>
      ) : null}
    </button>
  );
}

