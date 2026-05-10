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
  id: number;             // transfer id
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
  // we poll forever.
  const seenTerminalIds = useRef<Set<number>>(new Set());
  useEffect(() => {
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
    tick();
    const id = setInterval(tick, 700);
    return () => clearInterval(id);
  }, []);

  function dismissToast(id: number) {
    setToasts((prev) => prev.filter((tt) => tt.id !== id));
  }

  // Tab title comes from i18n nav_* keys so the header heading
  // tracks the locale toggle alongside the sidebar labels.
  const tabTitle = t(`nav_${tab}`);

  return (
    <div className="min-h-screen flex">
      <PairingModal />
      <DropOverlay visible={dropOverlay} />
      <ToastStack toasts={toasts} onDismiss={dismissToast} />
      <ActiveTransfersBadge count={activeTransfers} onClick={() => setTab("files")} />
      <aside className="w-56 shrink-0 border-r border-neutral-200 dark:border-neutral-800 px-3 py-6 flex flex-col">
        <h1 className="text-lg font-semibold mb-6 px-3">MineShare</h1>
        <nav className="flex flex-col gap-1 text-sm">
          <NavItem active={tab === "status"} onClick={() => setTab("status")}>
            {t("nav_status")}
          </NavItem>
          <NavItem active={tab === "layout"} onClick={() => setTab("layout")}>
            {t("nav_layout")}
          </NavItem>
          <NavItem active={tab === "devices"} onClick={() => setTab("devices")}>
            {t("nav_devices")}
          </NavItem>
          <NavItem active={tab === "audio"} onClick={() => setTab("audio")}>
            {t("nav_audio")}
          </NavItem>
          <NavItem active={tab === "files"} onClick={() => setTab("files")}>
            {t("nav_files")}
          </NavItem>
          <NavItem active={tab === "hotkeys"} onClick={() => setTab("hotkeys")}>
            {t("nav_hotkeys")}
          </NavItem>
          <NavItem active={tab === "advanced"} onClick={() => setTab("advanced")}>
            {t("nav_advanced")}
          </NavItem>
        </nav>
        <div className="mt-auto px-3 pt-4">
          <LanguageToggle />
        </div>
      </aside>
      <main className="flex-1 px-10 py-8">
        <header className="mb-6 flex items-baseline justify-between">
          <h2 className="text-2xl font-semibold">{tabTitle}</h2>
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
      </main>
    </div>
  );
}

/**
 * Full-window overlay shown when the user is dragging files
 * over the MineShare window. Strong visual cue that "yes, you
 * can drop here" without making them aim at a small target.
 * Pointer-events:none so the underlying drag-drop event still
 * lands on the webview.
 */
function DropOverlay({ visible }: { visible: boolean }) {
  if (!visible) return null;
  return (
    <div className="fixed inset-0 z-40 pointer-events-none flex items-center justify-center bg-emerald-500/15 backdrop-blur-[2px]">
      <div className="rounded-2xl border-4 border-dashed border-emerald-500 bg-white/95 dark:bg-neutral-900/95 px-12 py-10 shadow-2xl">
        <p className="text-6xl text-center mb-3">📥</p>
        <p className="text-lg font-semibold text-emerald-700 dark:text-emerald-300 text-center">
          Drop to send to peer
        </p>
        <p className="text-xs text-neutral-500 mt-1 text-center">
          Encrypted · auto-saves to Downloads/MineShare on the other side
        </p>
      </div>
    </div>
  );
}

/**
 * Always-visible badge in the bottom-right when one or more
 * transfers are in flight. Click → jump to Files tab. Lets the
 * user kick off a transfer, navigate elsewhere in the app, and
 * still see at-a-glance whether their file is still going.
 */
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
      className="fixed bottom-4 left-4 z-30 flex items-center gap-2 rounded-full bg-emerald-600 hover:bg-emerald-700 text-white px-3 py-1.5 shadow-lg text-xs font-medium transition-colors"
    >
      <span className="inline-block size-2 rounded-full bg-white animate-pulse" />
      📤 {count} transfer{count === 1 ? "" : "s"} in progress
    </button>
  );
}

/**
 * Slide-in toast stack for completed transfers. Each toast
 * auto-dismisses after 6 s. Click on a "done" toast to open the
 * file in the OS file manager. Stacks vertically in the top-
 * right so a burst of small files doesn't overwhelm.
 */
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
  // Auto-dismiss after 6 seconds.
  useEffect(() => {
    const id = setTimeout(onDismiss, 6_000);
    return () => clearTimeout(id);
  }, [onDismiss]);

  // Slide-in from the right on mount. Cheap CSS keyframe via
  // the `animate-` Tailwind classes would need a config tweak,
  // so we use an inline transition trick: render with
  // translate-x-full → flip to 0 on next tick.
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const id = requestAnimationFrame(() => setShown(true));
    return () => cancelAnimationFrame(id);
  }, []);

  const tone =
    entry.status === "done"
      ? entry.direction === "sending"
        ? "border-emerald-300 bg-emerald-50 dark:bg-emerald-950/60 dark:border-emerald-800"
        : "border-blue-300 bg-blue-50 dark:bg-blue-950/60 dark:border-blue-800"
      : entry.status === "cancelled"
        ? "border-neutral-300 bg-neutral-50 dark:bg-neutral-900 dark:border-neutral-700"
        : "border-red-300 bg-red-50 dark:bg-red-950/60 dark:border-red-800";

  const icon =
    entry.status === "done"
      ? entry.direction === "sending"
        ? "✅ ↗"
        : "✅ ↘"
      : entry.status === "cancelled"
        ? "⊘"
        : "⚠";

  const title =
    entry.status === "done"
      ? entry.direction === "sending"
        ? "Sent to peer"
        : "Received from peer"
      : entry.status === "cancelled"
        ? "Cancelled"
        : "Failed";

  // Click on a successful incoming transfer → open the file
  // location in the OS file manager. Clicking a sent or failed
  // toast just dismisses.
  const canOpen = entry.status === "done" && entry.direction === "receiving";
  const onClick = () => {
    if (canOpen) {
      invoke("open_downloads_dir").catch(() => {});
    }
    onDismiss();
  };

  return (
    <div
      onClick={onClick}
      className={
        "rounded-lg border-2 p-3 shadow-lg cursor-pointer transition-all duration-200 " +
        tone +
        (shown ? " translate-x-0 opacity-100" : " translate-x-full opacity-0")
      }
    >
      <div className="flex items-start gap-2">
        <span className="text-lg shrink-0">{icon}</span>
        <div className="min-w-0 flex-1">
          <p className="text-xs font-semibold">{title}</p>
          <p className="text-sm font-medium truncate">{entry.name}</p>
          {canOpen ? (
            <p className="text-[11px] text-neutral-500 mt-0.5">
              Click to open Downloads/MineShare
            </p>
          ) : null}
        </div>
        <button
          onClick={(e) => {
            e.stopPropagation();
            onDismiss();
          }}
          className="text-neutral-400 hover:text-neutral-700 dark:hover:text-neutral-200 text-xs px-1"
        >
          ×
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
    return <p className="text-xs text-red-600">{t("conn_offline")} {error}</p>;
  }
  if (!status) {
    return <p className="text-xs text-neutral-400">{t("conn_connecting")}</p>;
  }
  if (!status.peer_connected) {
    return (
      <span className="inline-flex items-center gap-2 text-xs text-neutral-500">
        <span className="size-2 rounded-full bg-neutral-400" />
        {t("conn_no_peer")}
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-2 text-xs text-emerald-600">
      <span className="size-2 rounded-full bg-emerald-500" />
      {t("conn_paired_with")} {status.peer_addr}
    </span>
  );
}

function AntiCheatBanner({ game }: { game: string }) {
  const { t } = useT();
  return (
    <div className="rounded-lg border-2 border-red-400 bg-red-50 dark:bg-red-950/40 p-4 mb-6">
      <p className="text-sm font-semibold text-red-700 dark:text-red-400 flex items-center gap-2">
        {t("ac_title")}{" "}
        <code className="font-mono">{game}</code>
      </p>
      <p className="text-xs text-red-700 dark:text-red-300 mt-1.5 leading-relaxed max-w-prose">
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
        "rounded-lg border p-4 mb-6 flex items-center justify-between transition-colors " +
        (locked
          ? "border-amber-300 bg-amber-50/60 dark:border-amber-900 dark:bg-amber-950/30"
          : "border-neutral-200 dark:border-neutral-800")
      }
    >
      <div>
        <p className="text-sm font-semibold flex items-center gap-2">
          {locked ? t("game_mode_on_title") : t("game_mode_off_title")}
        </p>
        <p className="text-xs text-neutral-500 mt-1 max-w-prose">
          {t("game_mode_desc")}{" "}
          <span className="text-neutral-400">
            {t("game_mode_shortcut")} <kbd className="font-mono">Ctrl+Alt+L</kbd>.
          </span>
        </p>
      </div>
      <button
        onClick={() => onChange(!locked)}
        className={
          "rounded-md px-4 py-2 text-sm font-medium transition-colors " +
          (locked
            ? "bg-amber-500 hover:bg-amber-600 text-white"
            : "border border-neutral-300 dark:border-neutral-700 hover:bg-neutral-50 dark:hover:bg-neutral-900")
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
    <dl className="grid grid-cols-2 md:grid-cols-3 gap-x-8 gap-y-5">
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
 *
 * This pill makes the current routing direction unmistakable:
 *   • emerald + arrow → "you're driving the peer, keys cross"
 *   • blue + arrow    → "peer is driving you, your keys are
 *                        forwarded back to you locally and
 *                        therefore probably useless to type"
 *   • neutral         → "both machines independent, normal"
 */
function ModePill({ s }: { s: Status }) {
  let label: string;
  let detail: string;
  let tone: string;
  if (s.local_in_remote) {
    label = "→ Driving peer";
    detail = "Cursor is on " + (s.peer_name ?? "peer") + ".";
    tone = "border-emerald-400 bg-emerald-50 dark:bg-emerald-950/40 text-emerald-700 dark:text-emerald-300";
  } else if (s.peer_in_remote) {
    label = "← Peer is driving";
    detail = (s.peer_name ?? "Peer") + " is controlling this machine. Your local input is paused.";
    tone = "border-blue-400 bg-blue-50 dark:bg-blue-950/40 text-blue-700 dark:text-blue-300";
  } else {
    label = "● Local";
    detail = "Cursor on this machine. Cross to the peer to start driving them.";
    tone = "border-neutral-300 dark:border-neutral-700 text-neutral-700 dark:text-neutral-300";
  }
  return (
    <div className="mb-6 grid gap-3 grid-cols-1 md:grid-cols-2">
      <div className={"rounded-lg border-2 p-4 flex items-center justify-between gap-4 " + tone}>
        <div className="min-w-0">
          <p className="text-[10px] uppercase tracking-wide opacity-60 mb-1">Mouse / cursor</p>
          <p className="text-base font-semibold leading-tight">{label}</p>
          <p className="text-xs opacity-80 mt-0.5 max-w-prose">{detail}</p>
        </div>
      </div>
      <KeyboardPill s={s} />
    </div>
  );
}

/**
 * Shows where keystrokes will land. By default this follows the
 * mouse cursor, but the user can pin keys to either side via the
 * Ctrl+Alt+K hotkey or the click-to-cycle button on this pill,
 * which is what makes "leave mouse on Win, type into Ubuntu"
 * possible.
 */
function KeyboardPill({ s }: { s: Status }) {
  // For Smart we don't know exactly where keys will land at this
  // very millisecond (it depends on activity timestamps the GUI
  // doesn't poll), so we just describe the mode and let the
  // counters confirm flow.
  let label: string;
  let detail: string;
  let tone: string;
  if (s.keyboard_target === "force_peer") {
    label = "🔒 → Pinned to peer";
    detail =
      "Every key you press goes to " +
      (s.peer_name ?? "peer") +
      ", no matter what the cursor or mouse is doing. Click to cycle.";
    tone = "border-amber-400 bg-amber-50 dark:bg-amber-950/40 text-amber-800 dark:text-amber-300";
  } else if (s.keyboard_target === "force_local") {
    label = "🔒 ← Pinned local";
    detail =
      "Keys stay on this machine even when the cursor crosses to the peer.";
    tone = "border-amber-400 bg-amber-50 dark:bg-amber-950/40 text-amber-800 dark:text-amber-300";
  } else if (s.keyboard_target === "auto") {
    if (s.local_in_remote) {
      label = "→ Auto (cursor on peer)";
      detail = "Strict cursor mode: keys are following the cursor to " + (s.peer_name ?? "peer") + ".";
      tone = "border-emerald-400 bg-emerald-50 dark:bg-emerald-950/40 text-emerald-700 dark:text-emerald-300";
    } else {
      label = "● Auto (cursor on local)";
      detail = "Strict cursor mode: keys land on whichever machine the cursor is on. Click to switch back to Smart.";
      tone = "border-neutral-300 dark:border-neutral-700 text-neutral-700 dark:text-neutral-300";
    }
  } else {
    // smart (default)
    label = "✨ Smart";
    detail = s.local_in_remote
      ? "Cursor is on " + (s.peer_name ?? "peer") + " — keys follow it. Smart also auto-routes to whichever machine's mouse is currently in use."
      : "Auto-routes to whichever side's mouse is in use. Use the " + (s.peer_name ?? "peer") + " mouse → Win keys land there. Use the local mouse → keys come back here.";
    tone = "border-emerald-300 bg-emerald-50/60 dark:bg-emerald-950/30 text-emerald-700 dark:text-emerald-300";
  }

  return (
    <button
      onClick={() => invoke("cycle_keyboard_target")}
      className={
        "rounded-lg border-2 p-4 flex items-center justify-between gap-4 text-left transition-colors hover:brightness-95 cursor-pointer " +
        tone
      }
      title="Click to cycle: Auto → Pinned-to-peer → Pinned-local → Auto (or press Ctrl+Alt+K)"
    >
      <div className="min-w-0">
        <p className="text-[10px] uppercase tracking-wide opacity-60 mb-1">Keyboard</p>
        <p className="text-base font-semibold leading-tight">{label}</p>
        <p className="text-xs opacity-80 mt-0.5 max-w-prose">{detail}</p>
      </div>
      <div className="text-right text-[11px] font-mono opacity-70 shrink-0">
        <p>sent {s.keys_forwarded.toLocaleString()}</p>
        <p>recv {s.keys_injected.toLocaleString()}</p>
      </div>
    </button>
  );
}

function LatencyCard({ latency }: { latency: Latency | null }) {
  if (!latency || latency.samples === 0) {
    return (
      <div className="mt-8 rounded-lg border border-neutral-200 dark:border-neutral-800 p-5">
        <p className="text-sm font-semibold mb-1">Network latency</p>
        <p className="text-xs text-neutral-500">
          waiting for the first round-trip…
        </p>
      </div>
    );
  }
  const fmt = (n: number | null) =>
    n === null ? "—" : n < 10 ? n.toFixed(1) : n.toFixed(0);
  // Tone the headline number red when p95 crosses 100 ms — the
  // band where input feels noticeably laggy on a LAN.
  const p95 = latency.p95_ms ?? 0;
  const headlineColor =
    p95 >= 100
      ? "text-red-600"
      : p95 >= 50
        ? "text-amber-600"
        : "text-emerald-600";

  return (
    <div className="mt-8 rounded-lg border border-neutral-200 dark:border-neutral-800 p-5">
      <div className="flex items-baseline justify-between mb-3">
        <p className="text-sm font-semibold">Network latency</p>
        <p className="text-[11px] text-neutral-500">
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
      <p className="text-[10px] uppercase tracking-wide text-neutral-500 mb-0.5">
        {label}
      </p>
      <p
        className={
          "text-sm font-mono font-medium " +
          (accent ?? (muted ? "text-neutral-400" : ""))
        }
      >
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
  // Bars are 24 px tall max; smallest non-zero count still draws a
  // visible nub so empty-vs-rare is distinguishable.
  return (
    <div>
      <div className="flex items-end gap-1.5 h-7">
        {histogram.map((count, i) => {
          const frac = count === 0 ? 0 : Math.max(0.12, count / max);
          const tone =
            i >= edges.length // overflow bin (>= last edge)
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
              style={{ height: `${frac * 100}%`, opacity: count === 0 ? 0.18 : 1 }}
              title={`${labelFor(i, edges)}: ${count}`}
            />
          );
        })}
      </div>
      <div className="flex gap-1.5 mt-1">
        {histogram.map((_, i) => (
          <span
            key={i}
            className="flex-1 text-center text-[9px] font-mono text-neutral-400"
          >
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
      <dt className="text-[11px] uppercase tracking-wide text-neutral-500 mb-1">
        {label}
      </dt>
      <dd className={mono ? "font-mono text-sm" : "text-sm font-medium"}>
        {value}
      </dd>
    </div>
  );
}

function NavItem({
  children,
  active,
  onClick,
}: {
  children: React.ReactNode;
  active?: boolean;
  onClick?: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={
        "text-left px-3 py-2 rounded-md transition-colors " +
        (active
          ? "bg-neutral-100 dark:bg-neutral-800 font-medium"
          : "hover:bg-neutral-50 dark:hover:bg-neutral-900")
      }
    >
      {children}
    </button>
  );
}
