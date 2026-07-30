import { invoke } from "@tauri-apps/api/core";
import { useT } from "../i18n";
import type { Status, Latency } from "../types";

export default function HomePage({ status, latency }: { status: Status; latency: Latency | null }) {
  return (
    <>
      {status.anticheat_warning ? <AntiCheatBanner game={status.anticheat_warning} /> : null}
      <ConnectionHero status={status} latency={latency} />
      <ModePill s={status} />
      <GameLockCard s={status} onChange={(v) => invoke("set_input_lock", { locked: v })} />
      <GameDriveCard s={status} />
      <StatusGrid s={status} />
      {status.peer_connected ? <LatencyCard latency={latency} /> : null}
    </>
  );
}

function ConnectionHero({ status, latency }: { status: Status; latency: Latency | null }) {
  const connected = status.peer_connected;
  const headlineMs = latency?.p50_ms ?? latency?.last_ms ?? null;
  return (
    <div className="rounded-2xl border border-ds-border bg-ds-surface p-6 mb-6">
      <div className="flex items-center justify-between gap-6">
        <div className="flex items-center gap-4">
          <MachineBadge label="This PC" active />
          <span className="text-ds-accent font-mono text-lg">━━●━━▶</span>
          <MachineBadge label={status.peer_name ?? "Peer"} active={connected} />
        </div>
        <div className="text-right">
          <p className="text-[10px] uppercase tracking-widest text-ds-text-muted">Latency</p>
          <p className="text-2xl font-semibold text-ds-text">
            {connected && headlineMs != null ? `${headlineMs < 10 ? headlineMs.toFixed(1) : headlineMs.toFixed(0)} ms` : "—"}
          </p>
          <p className="text-xs text-ds-text-muted">{connected ? `paired with ${status.peer_addr ?? "peer"}` : "waiting for a peer on the LAN…"}</p>
        </div>
      </div>
    </div>
  );
}

function MachineBadge({ label, active }: { label: string; active: boolean }) {
  return (
    <div className={"rounded-xl border px-4 py-3 text-sm font-medium " + (active ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-200" : "border-ds-border bg-ds-hover text-ds-text-muted")}>
      {label}
    </div>
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
          : "border-ds-border bg-ds-surface")
      }
    >
      <div>
        <p className={"text-sm font-semibold " + (locked ? "text-amber-300" : "text-ds-text")}>
          {locked ? t("game_mode_on_title") : t("game_mode_off_title")}
        </p>
        <p className="text-xs text-ds-text-muted mt-1 max-w-prose leading-relaxed">
          {t("game_mode_desc")}{" "}
          <span className="text-ds-text-muted">
            {t("game_mode_shortcut")} <kbd className="font-mono text-ds-text bg-ds-hover px-1 py-0.5 rounded text-[10px]">Ctrl+Alt+L</kbd>.
          </span>
        </p>
      </div>
      <button
        onClick={() => onChange(!locked)}
        className={
          "rounded-lg px-4 py-2 text-sm font-medium transition-all duration-150 shrink-0 ml-4 " +
          (locked
            ? "bg-amber-500 hover:bg-amber-400 text-white shadow-lg shadow-amber-500/20"
            : "border border-ds-border bg-ds-hover hover:bg-ds-hover text-ds-text")
        }
      >
        {locked ? t("game_mode_unlock") : t("game_mode_lock")}
      </button>
    </div>
  );
}

function GameDriveCard({ s }: { s: Status }) {
  const { t } = useT();
  const state = s.game_drive;
  const active = state !== "off";
  return (
    <div
      className={
        "rounded-xl border p-4 mb-6 flex items-center justify-between transition-all duration-150 " +
        (active
          ? "border-purple-500/30 bg-purple-500/10"
          : "border-ds-border bg-ds-surface")
      }
    >
      <div className="min-w-0">
        <p className={"text-sm font-semibold " + (active ? "text-purple-300" : "text-ds-text")}>
          {state === "driving"
            ? "🎮 " + t("gd_driving")
            : state === "receiving"
              ? "🎮 " + t("gd_receiving")
              : t("gd_title")}
        </p>
        <p className="text-xs text-ds-text-muted mt-1 max-w-prose leading-relaxed">
          {t("gd_desc")}{" "}
          <kbd className="font-mono text-ds-text bg-ds-hover px-1 py-0.5 rounded text-[10px]">Ctrl+Alt+G</kbd>.
          {active ? " " + t("gd_anticheat_note") : ""}
        </p>
      </div>
      <button
        onClick={() => invoke("toggle_game_drive")}
        disabled={!s.peer_connected}
        className={
          "rounded-lg px-4 py-2 text-sm font-medium transition-all duration-150 shrink-0 ml-4 " +
          (active
            ? "bg-purple-500 hover:bg-purple-400 text-white shadow-lg shadow-purple-500/20"
            : "border border-ds-border bg-ds-hover hover:bg-ds-hover text-ds-text disabled:opacity-40")
        }
      >
        {active ? t("gd_stop") : t("gd_start")}
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
    <dl className="grid grid-cols-2 md:grid-cols-3 gap-x-8 gap-y-5 rounded-xl border border-ds-border bg-ds-surface p-5 mb-6">
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
    tone = "border-ds-border bg-ds-surface text-ds-text";
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
      tone = "border-ds-border bg-ds-surface text-ds-text";
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
      <div className="mt-6 rounded-xl border border-ds-border bg-ds-surface p-5">
        <p className="text-sm font-semibold text-ds-text mb-1">Network latency</p>
        <p className="text-xs text-ds-text-muted">waiting for the first round-trip…</p>
      </div>
    );
  }
  const fmt = (n: number | null) =>
    n === null ? "—" : n < 10 ? n.toFixed(1) : n.toFixed(0);
  const p95 = latency.p95_ms ?? 0;
  const headlineColor =
    p95 >= 100 ? "text-red-400" : p95 >= 50 ? "text-amber-400" : "text-emerald-400";

  return (
    <div className="mt-6 rounded-xl border border-ds-border bg-ds-surface p-5">
      <div className="flex items-baseline justify-between mb-4">
        <p className="text-sm font-semibold text-ds-text">Network latency</p>
        <p className="text-[11px] text-ds-text-muted">
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
      <p className="text-[10px] uppercase tracking-widest text-ds-text-muted mb-0.5">{label}</p>
      <p className={"text-sm font-mono font-medium " + (accent ?? (muted ? "text-ds-text-muted" : "text-ds-text"))}>
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
          <span key={i} className="flex-1 text-center text-[9px] font-mono text-ds-text-muted">
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
      <dt className="text-[10px] uppercase tracking-widest text-ds-text-muted mb-1">{label}</dt>
      <dd className={mono ? "font-mono text-sm text-ds-text" : "text-sm font-medium text-ds-text"}>
        {value}
      </dd>
    </div>
  );
}
