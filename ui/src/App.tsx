import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import LayoutPage from "./pages/Layout";
import AudioPage from "./pages/Audio";
import DevicesPage from "./pages/Devices";
import HotkeysPage from "./pages/Hotkeys";
import AdvancedPage from "./pages/Advanced";
import PairingModal from "./PairingModal";

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
};

type Tab = "status" | "layout" | "devices" | "audio" | "hotkeys" | "advanced";

export default function App() {
  const [tab, setTab] = useState<Tab>("status");
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const tick = () =>
      invoke<Status>("get_status")
        .then((s) => {
          setStatus(s);
          setError(null);
        })
        .catch((e) => setError(String(e)));
    tick();
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, []);

  return (
    <div className="min-h-screen flex">
      <PairingModal />
      <aside className="w-56 shrink-0 border-r border-neutral-200 dark:border-neutral-800 px-3 py-6">
        <h1 className="text-lg font-semibold mb-6 px-3">MineShare</h1>
        <nav className="flex flex-col gap-1 text-sm">
          <NavItem active={tab === "status"} onClick={() => setTab("status")}>
            Status
          </NavItem>
          <NavItem active={tab === "layout"} onClick={() => setTab("layout")}>
            Layout
          </NavItem>
          <NavItem active={tab === "devices"} onClick={() => setTab("devices")}>
            Devices
          </NavItem>
          <NavItem active={tab === "audio"} onClick={() => setTab("audio")}>
            Audio
          </NavItem>
          <NavItem active={tab === "hotkeys"} onClick={() => setTab("hotkeys")}>
            Hotkeys
          </NavItem>
          <NavItem active={tab === "advanced"} onClick={() => setTab("advanced")}>
            Advanced
          </NavItem>
        </nav>
      </aside>
      <main className="flex-1 px-10 py-8">
        <header className="mb-6 flex items-baseline justify-between">
          <h2 className="text-2xl font-semibold capitalize">{tab}</h2>
          <ConnectionPill status={status} error={error} />
        </header>

        {tab === "status" && status ? (
          <>
            {status.anticheat_warning ? (
              <AntiCheatBanner game={status.anticheat_warning} />
            ) : null}
            <GameLockCard s={status} onChange={(v) => invoke("set_input_lock", { locked: v })} />
            <StatusGrid s={status} />
          </>
        ) : null}
        {tab === "layout" ? <LayoutPage /> : null}
        {tab === "devices" ? <DevicesPage /> : null}
        {tab === "audio" ? <AudioPage /> : null}
        {tab === "hotkeys" ? <HotkeysPage /> : null}
        {tab === "advanced" ? <AdvancedPage /> : null}
      </main>
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
  if (error) {
    return <p className="text-xs text-red-600">daemon offline: {error}</p>;
  }
  if (!status) {
    return <p className="text-xs text-neutral-400">connecting…</p>;
  }
  if (!status.peer_connected) {
    return (
      <span className="inline-flex items-center gap-2 text-xs text-neutral-500">
        <span className="size-2 rounded-full bg-neutral-400" />
        no peer
      </span>
    );
  }
  return (
    <span className="inline-flex items-center gap-2 text-xs text-emerald-600">
      <span className="size-2 rounded-full bg-emerald-500" />
      paired with {status.peer_addr}
    </span>
  );
}

function AntiCheatBanner({ game }: { game: string }) {
  return (
    <div className="rounded-lg border-2 border-red-400 bg-red-50 dark:bg-red-950/40 p-4 mb-6">
      <p className="text-sm font-semibold text-red-700 dark:text-red-400 flex items-center gap-2">
        ⚠ Anti-cheat-protected game detected:{" "}
        <code className="font-mono">{game}</code>
      </p>
      <p className="text-xs text-red-700 dark:text-red-300 mt-1.5 leading-relaxed max-w-prose">
        Input is auto-locked to this PC for safety. Kernel-level
        anti-cheat (BattlEye / EAC / Vanguard / RICOCHET / Hyperion)
        can flag SendInput-style injected events as cheating and ban
        accounts. The bridge will resume normally once the game is no
        longer in the foreground.
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
          {locked ? "🔒 Game mode — input pinned to this PC" : "Game mode — off"}
        </p>
        <p className="text-xs text-neutral-500 mt-1 max-w-prose">
          When on, edge crossing and auto-handover are disabled so an
          accidental cursor swing during a fullscreen game can't yank
          your keyboard to the other machine. Ctrl+Alt+R still works
          as a manual override.{" "}
          <span className="text-neutral-400">
            Toggle with <kbd className="font-mono">Ctrl+Alt+L</kbd>.
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
        {locked ? "Unlock" : "Lock"}
      </button>
    </div>
  );
}

function StatusGrid({ s }: { s: Status }) {
  const cursor = s.local_in_remote
    ? "driving peer"
    : s.peer_in_remote
      ? "driven by peer"
      : "local";
  return (
    <dl className="grid grid-cols-2 md:grid-cols-3 gap-x-8 gap-y-5">
      <Stat label="Cursor" value={cursor} />
      <Stat label="Peer addr" value={s.peer_addr ?? "—"} mono />
      <Stat label="Sent pkts" value={s.sent_pkts.toLocaleString()} />
      <Stat label="Recv pkts" value={s.recv_pkts.toLocaleString()} />
      <Stat label="Audio frames" value={s.audio_recv.toLocaleString()} />
      <Stat label="Injected" value={s.injected.toLocaleString()} />
    </dl>
  );
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
