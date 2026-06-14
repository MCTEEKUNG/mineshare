import { useEffect, useState, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import LayoutPage from "./pages/Layout";
import AudioPage from "./pages/Audio";
import DevicesPage from "./pages/Devices";
import HotkeysPage from "./pages/Hotkeys";
import AdvancedPage from "./pages/Advanced";
import FilesPage from "./pages/Files";
import PairingModal from "./PairingModal";
import { LanguageToggle, useT } from "./i18n";

type Status = {
  peer_connected: boolean;
  peer_addr: string | null;
  peer_name: string | null;
  sent_pkts: number;
  recv_pkts: number;
  injected: number;
  audio_recv: number;
  inject_errs: number;
  decrypt_errs: number;
  local_in_remote: boolean;
  peer_in_remote: boolean;
  input_locked: boolean;
  anticheat_warning: string | null;
  keys_forwarded: number;
  keys_injected: number;
  keyboard_target: "smart" | "auto" | "force_peer" | "force_local";
  local_version: string;
  peer_version: string | null;
};

type Latency = {
  samples: number;
  last_ms: number | null;
  min_ms: number | null;
  avg_ms: number | null;
  p50_ms: number | null;
  p95_ms: number | null;
  max_ms: number | null;
  histogram: number[];
  bin_edges_ms: number[];
};

type Tab = "status" | "layout" | "devices" | "audio" | "files" | "hotkeys" | "advanced";

type Transfer = {
  id: number;
  direction: "sending" | "receiving";
  status: "pending" | "active" | "verifying" | "done" | "cancelled" | "failed";
  name: string;
  size_bytes: number;
  bytes_so_far: number;
  final_path: string | null;
  error: string | null;
  seconds_elapsed: number;
};

type ToastEntry = {
  id: number;
  direction: "sending" | "receiving";
  status: "done" | "failed" | "cancelled";
  name: string;
  final_path: string | null;
};

export default function App() {
  const [tab, setTab] = useState<Tab>("status");
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

  // Latency / RTT histogram — only polled while the Status tab is
  // active AND the window is visible (same hide-to-tray rationale
  // as `get_status` above). The daemon's ping task fires every
  // 500 ms so a 1 s GUI poll is plenty to keep the bars live.
  useEffect(() => {
    if (tab !== "status") return;
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
          <NavItem active={tab === "status"} onClick={() => setTab("status")} icon={<IconActivity className="size-4" />}>
            {t("nav_status")}
          </NavItem>
          <NavItem active={tab === "layout"} onClick={() => setTab("layout")} icon={<IconGrid className="size-4" />}>
            {t("nav_layout")}
          </NavItem>
          <NavItem active={tab === "devices"} onClick={() => setTab("devices")} icon={<IconHeadphones className="size-4" />}>
            {t("nav_devices")}
          </NavItem>
          <NavItem active={tab === "audio"} onClick={() => setTab("audio")} icon={<IconVolume className="size-4" />}>
            {t("nav_audio")}
          </NavItem>
          <NavItem active={tab === "files"} onClick={() => setTab("files")} icon={<IconFile className="size-4" />} badge={activeTransfers > 0 ? activeTransfers : undefined}>
            {t("nav_files")}
          </NavItem>
          <NavItem active={tab === "hotkeys"} onClick={() => setTab("hotkeys")} icon={<IconKeyboard className="size-4" />}>
            {t("nav_hotkeys")}
          </NavItem>
          <NavItem active={tab === "advanced"} onClick={() => setTab("advanced")} icon={<IconSliders className="size-4" />}>
            {t("nav_advanced")}
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

          {tab === "status" && status ? (
            <>
              {status.anticheat_warning ? (
                <AntiCheatBanner game={status.anticheat_warning} />
              ) : null}
              <ModePill s={status} />
              <GameLockCard s={status} onChange={(v) => invoke("set_input_lock", { locked: v })} />
              <StatusGrid s={status} />
              {status.peer_connected ? <LatencyCard latency={latency} /> : null}
            </>
          ) : null}
          {tab === "layout" ? <LayoutPage /> : null}
          {tab === "devices" ? <DevicesPage /> : null}
          {tab === "audio" ? <AudioPage /> : null}
          {tab === "files" ? <FilesPage /> : null}
          {tab === "hotkeys" ? <HotkeysPage /> : null}
          {tab === "advanced" ? <AdvancedPage /> : null}
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

function AntiCheatBanner({ game }: { game: string }) {
  const { t } = useT();
  return (
    <div className="rounded-xl border border-red-500/30 bg-red-500/10 p-4 mb-6">
      <p className="text-sm font-semibold text-red-400">
        {t("ac_title")} <code className="font-mono">{game}</code>
      </p>
      <p className="text-xs text-red-300/80 mt-1.5 leading-relaxed max-w-prose">
        {t("ac_desc")}
      </p>
    </div>
  );
}

function GameLockCard({
  s,
  onChange,
}: {
  s: Status;
  onChange: (v: boolean) => void;
}) {
  const { t } = useT();
  const locked = s.input_locked;
  return (
    <div
      className={
        "rounded-xl border p-4 mb-6 flex items-center justify-between transition-all duration-150 " +
        (locked
          ? "border-amber-500/30 bg-amber-500/10"
          : "border-white/[0.08] bg-ds-surface")
      }
    >
      <div>
        <p className={"text-sm font-semibold " + (locked ? "text-amber-300" : "text-slate-100")}>
          {locked ? t("game_mode_on_title") : t("game_mode_off_title")}
        </p>
        <p className="text-xs text-slate-400 mt-1 max-w-prose leading-relaxed">
          {t("game_mode_desc")}{" "}
          <span className="text-slate-400">
            {t("game_mode_shortcut")} <kbd className="font-mono text-slate-300 bg-white/[0.08] px-1 py-0.5 rounded text-[10px]">Ctrl+Alt+L</kbd>.
          </span>
        </p>
      </div>
      <button
        onClick={() => onChange(!locked)}
        className={
          "rounded-lg px-4 py-2 text-sm font-medium transition-all duration-150 shrink-0 ml-4 " +
          (locked
            ? "bg-amber-500 hover:bg-amber-400 text-white shadow-lg shadow-amber-500/20"
            : "border border-white/[0.10] bg-white/[0.04] hover:bg-white/[0.08] text-slate-300")
        }
      >
        {locked ? t("game_mode_unlock") : t("game_mode_lock")}
      </button>
    </div>
  );
}

function StatusGrid({ s }: { s: Status }) {
  const { t } = useT();
  const cursor = s.local_in_remote
    ? t("cursor_driving_peer")
    : s.peer_in_remote
      ? t("cursor_driven_by_peer")
      : t("cursor_local");
  return (
    <dl className="grid grid-cols-2 md:grid-cols-3 gap-x-8 gap-y-5 rounded-xl border border-white/[0.08] bg-ds-surface p-5 mb-6">
      <Stat label={t("stat_cursor")} value={cursor} />
      <Stat label={t("stat_peer_addr")} value={s.peer_addr ?? "—"} mono />
      <Stat label={t("stat_sent_pkts")} value={s.sent_pkts.toLocaleString()} />
      <Stat label={t("stat_recv_pkts")} value={s.recv_pkts.toLocaleString()} />
      <Stat label={t("stat_audio_frames")} value={s.audio_recv.toLocaleString()} />
      <Stat label={t("stat_injected")} value={s.injected.toLocaleString()} />
    </dl>
  );
}

/**
 * Big visible mode indicator. The bridge's #1 confusion vector
 * is "I typed but nothing happened" — usually because the user
 * thought their cursor was on the peer but the local machine is
 * actually still in MODE_LOCAL, so keystrokes go to local apps.
 * Or the reverse: peer is driving us and the local OS swallowed
 * a key that the user expected to type into a peer app.
 */
function ModePill({ s }: { s: Status }) {
  let label: string;
  let detail: string;
  let tone: string;
  if (s.local_in_remote) {
    label = "→ Driving peer";
    detail = "Cursor is on " + (s.peer_name ?? "peer") + ".";
    tone = "border-emerald-500/30 bg-emerald-500/10 text-emerald-300";
  } else if (s.peer_in_remote) {
    label = "← Peer is driving";
    detail = (s.peer_name ?? "Peer") + " is controlling this machine. Your local input is paused.";
    tone = "border-blue-500/30 bg-blue-500/10 text-blue-300";
  } else {
    label = "● Local";
    detail = "Cursor on this machine. Cross to the peer to start driving them.";
    tone = "border-white/[0.08] bg-ds-surface text-slate-300";
  }
  return (
    <div className="mb-6 grid gap-3 grid-cols-1 md:grid-cols-2">
      <div className={"rounded-xl border-2 p-4 " + tone}>
        <p className="text-[10px] uppercase tracking-widest opacity-50 mb-1">Mouse / cursor</p>
        <p className="text-base font-semibold leading-tight">{label}</p>
        <p className="text-xs opacity-75 mt-1 max-w-prose leading-relaxed">{detail}</p>
      </div>
      <KeyboardPill s={s} />
    </div>
  );
}

/**
 * Shows where keystrokes will land. By default this follows the
 * mouse cursor, but the user can pin keys to either side via the
 * Ctrl+Alt+K hotkey or the click-to-cycle button on this pill.
 */
function KeyboardPill({ s }: { s: Status }) {
  let label: string;
  let detail: string;
  let tone: string;
  if (s.keyboard_target === "force_peer") {
    label = "Pinned to peer";
    detail =
      "Every key you press goes to " +
      (s.peer_name ?? "peer") +
      ", no matter what the cursor or mouse is doing. Click to cycle.";
    tone = "border-amber-500/30 bg-amber-500/10 text-amber-300";
  } else if (s.keyboard_target === "force_local") {
    label = "Pinned local";
    detail = "Keys stay on this machine even when the cursor crosses to the peer.";
    tone = "border-amber-500/30 bg-amber-500/10 text-amber-300";
  } else if (s.keyboard_target === "auto") {
    if (s.local_in_remote) {
      label = "→ Auto (cursor on peer)";
      detail = "Strict cursor mode: keys are following the cursor to " + (s.peer_name ?? "peer") + ".";
      tone = "border-emerald-500/30 bg-emerald-500/10 text-emerald-300";
    } else {
      label = "● Auto (cursor on local)";
      detail = "Strict cursor mode: keys land on whichever machine the cursor is on. Click to switch back to Smart.";
      tone = "border-white/[0.08] bg-ds-surface text-slate-300";
    }
  } else {
    // smart (default)
    label = "✨ Smart";
    detail = s.local_in_remote
      ? "Cursor is on " + (s.peer_name ?? "peer") + " — keys follow it. Smart also auto-routes to whichever machine's mouse is currently in use."
      : "Auto-routes to whichever side's mouse is in use. Use the " + (s.peer_name ?? "peer") + " mouse → Win keys land there. Use the local mouse → keys come back here.";
    tone = "border-emerald-500/20 bg-emerald-500/[0.07] text-emerald-300";
  }

  return (
    <button
      onClick={() => invoke("cycle_keyboard_target")}
      className={
        "rounded-xl border-2 p-4 flex items-start justify-between gap-4 text-left transition-all duration-150 hover:brightness-110 cursor-pointer " +
        tone
      }
      title="Click to cycle: Auto → Pinned-to-peer → Pinned-local → Auto (or press Ctrl+Alt+K)"
    >
      <div className="min-w-0">
        <p className="text-[10px] uppercase tracking-widest opacity-50 mb-1">Keyboard</p>
        <p className="text-base font-semibold leading-tight">{label}</p>
        <p className="text-xs opacity-75 mt-1 max-w-prose leading-relaxed">{detail}</p>
      </div>
      <div className="text-right text-[11px] font-mono opacity-60 shrink-0">
        <p>sent {s.keys_forwarded.toLocaleString()}</p>
        <p>recv {s.keys_injected.toLocaleString()}</p>
      </div>
    </button>
  );
}

function LatencyCard({ latency }: { latency: Latency | null }) {
  if (!latency || latency.samples === 0) {
    return (
      <div className="mt-6 rounded-xl border border-white/[0.08] bg-ds-surface p-5">
        <p className="text-sm font-semibold text-slate-200 mb-1">Network latency</p>
        <p className="text-xs text-slate-400">waiting for the first round-trip…</p>
      </div>
    );
  }
  const fmt = (n: number | null) =>
    n === null ? "—" : n < 10 ? n.toFixed(1) : n.toFixed(0);
  const p95 = latency.p95_ms ?? 0;
  const headlineColor =
    p95 >= 100 ? "text-red-400" : p95 >= 50 ? "text-amber-400" : "text-emerald-400";

  return (
    <div className="mt-6 rounded-xl border border-white/[0.08] bg-ds-surface p-5">
      <div className="flex items-baseline justify-between mb-4">
        <p className="text-sm font-semibold text-slate-200">Network latency</p>
        <p className="text-[11px] text-slate-400">
          {latency.samples} sample{latency.samples === 1 ? "" : "s"} · ping every 500 ms
        </p>
      </div>

      <div className="grid grid-cols-2 md:grid-cols-5 gap-x-6 gap-y-3 mb-5">
        <LatencyStat label="Last" value={fmt(latency.last_ms) + " ms"} accent={headlineColor} />
        <LatencyStat label="p50" value={fmt(latency.p50_ms) + " ms"} />
        <LatencyStat label="p95" value={fmt(latency.p95_ms) + " ms"} accent={headlineColor} />
        <LatencyStat label="min" value={fmt(latency.min_ms) + " ms"} muted />
        <LatencyStat label="max" value={fmt(latency.max_ms) + " ms"} muted />
      </div>

      <Histogram histogram={latency.histogram} edges={latency.bin_edges_ms} />
    </div>
  );
}

function LatencyStat({
  label,
  value,
  accent,
  muted,
}: {
  label: string;
  value: string;
  accent?: string;
  muted?: boolean;
}) {
  return (
    <div>
      <p className="text-[10px] uppercase tracking-widest text-slate-400 mb-0.5">{label}</p>
      <p className={"text-sm font-mono font-medium " + (accent ?? (muted ? "text-slate-400" : "text-slate-200"))}>
        {value}
      </p>
    </div>
  );
}

function Histogram({
  histogram,
  edges,
}: {
  histogram: number[];
  edges: number[];
}) {
  const max = Math.max(1, ...histogram);
  return (
    <div>
      <div className="flex items-end gap-1.5 h-7">
        {histogram.map((count, i) => {
          const frac = count === 0 ? 0 : Math.max(0.12, count / max);
          const tone =
            i >= edges.length
              ? "bg-red-500"
              : edges[i] >= 100
                ? "bg-red-400"
                : edges[i] >= 50
                  ? "bg-amber-400"
                  : "bg-emerald-400";
          return (
            <div
              key={i}
              className={"flex-1 rounded-sm transition-all " + tone}
              style={{ height: `${frac * 100}%`, opacity: count === 0 ? 0.15 : 1 }}
              title={`${labelFor(i, edges)}: ${count}`}
            />
          );
        })}
      </div>
      <div className="flex gap-1.5 mt-1">
        {histogram.map((_, i) => (
          <span key={i} className="flex-1 text-center text-[9px] font-mono text-slate-400">
            {labelFor(i, edges)}
          </span>
        ))}
      </div>
    </div>
  );
}

function labelFor(i: number, edges: number[]): string {
  if (i >= edges.length) {
    const last = edges[edges.length - 1];
    return last !== undefined ? `≥${last}` : "—";
  }
  return `<${edges[i]}`;
}

function Stat({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div>
      <dt className="text-[10px] uppercase tracking-widest text-slate-500 mb-1">{label}</dt>
      <dd className={mono ? "font-mono text-sm text-slate-200" : "text-sm font-medium text-slate-200"}>
        {value}
      </dd>
    </div>
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

/* ── SVG icon components ─────────────────────────────────────────── */

function IconActivity({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
    </svg>
  );
}

function IconGrid({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <rect x="3" y="3" width="7" height="7" />
      <rect x="14" y="3" width="7" height="7" />
      <rect x="14" y="14" width="7" height="7" />
      <rect x="3" y="14" width="7" height="7" />
    </svg>
  );
}

function IconHeadphones({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <path d="M3 18v-6a9 9 0 0 1 18 0v6" />
      <path d="M21 19a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3z" />
      <path d="M3 19a2 2 0 0 0 2 2h1a2 2 0 0 0 2-2v-3a2 2 0 0 0-2-2H3z" />
    </svg>
  );
}

function IconVolume({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" />
      <path d="M19.07 4.93a10 10 0 0 1 0 14.14" />
      <path d="M15.54 8.46a5 5 0 0 1 0 7.07" />
    </svg>
  );
}

function IconFile({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z" />
      <polyline points="14 2 14 8 20 8" />
    </svg>
  );
}

function IconKeyboard({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <rect x="2" y="6" width="20" height="12" rx="2" />
      <path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M8 14h8" />
    </svg>
  );
}

function IconSliders({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <line x1="4" y1="21" x2="4" y2="14" />
      <line x1="4" y1="10" x2="4" y2="3" />
      <line x1="12" y1="21" x2="12" y2="12" />
      <line x1="12" y1="8" x2="12" y2="3" />
      <line x1="20" y1="21" x2="20" y2="16" />
      <line x1="20" y1="12" x2="20" y2="3" />
      <line x1="1" y1="14" x2="7" y2="14" />
      <line x1="9" y1="8" x2="15" y2="8" />
      <line x1="17" y1="16" x2="23" y2="16" />
    </svg>
  );
}

function IconArrowDown({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <line x1="12" y1="5" x2="12" y2="19" />
      <polyline points="19 12 12 19 5 12" />
    </svg>
  );
}

function IconArrowUp({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <line x1="12" y1="19" x2="12" y2="5" />
      <polyline points="5 12 12 5 19 12" />
    </svg>
  );
}

function IconArrowUpRight({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <line x1="7" y1="17" x2="17" y2="7" />
      <polyline points="7 7 17 7 17 17" />
    </svg>
  );
}

function IconArrowDownLeft({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <line x1="17" y1="7" x2="7" y2="17" />
      <polyline points="17 17 7 17 7 7" />
    </svg>
  );
}

function IconX({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <line x1="18" y1="6" x2="6" y2="18" />
      <line x1="6" y1="6" x2="18" y2="18" />
    </svg>
  );
}

function IconAlertTriangle({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
      <line x1="12" y1="9" x2="12" y2="13" />
      <line x1="12" y1="17" x2="12.01" y2="17" />
    </svg>
  );
}
