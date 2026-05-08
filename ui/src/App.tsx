import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import LayoutPage from "./pages/Layout";

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

        {tab === "status" && status ? <StatusGrid s={status} /> : null}
        {tab === "layout" ? <LayoutPage /> : null}
        {tab === "devices" || tab === "audio" || tab === "hotkeys" || tab === "advanced" ? (
          <Placeholder name={tab} />
        ) : null}
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

function Placeholder({ name }: { name: string }) {
  return (
    <div className="rounded-lg border border-dashed border-neutral-300 dark:border-neutral-700 p-12 text-center text-neutral-500">
      <p className="font-medium text-neutral-700 dark:text-neutral-300 mb-1">
        {name} — coming up
      </p>
      <p className="text-sm">M5 Slices 3 + 4 fill these tabs in.</p>
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
