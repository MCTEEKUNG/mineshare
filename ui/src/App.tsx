import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import LayoutPage from "./pages/Layout";

type Status = {
  device_id: string;
  display_name: string;
  os: string;
};

export default function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<Status>("get_status")
      .then(setStatus)
      .catch((e) => setError(String(e)));
  }, []);

  return (
    <div className="min-h-screen flex">
      <aside className="w-56 shrink-0 border-r border-neutral-200 dark:border-neutral-800 px-3 py-6">
        <h1 className="text-lg font-semibold mb-6 px-3">MineShare</h1>
        <nav className="flex flex-col gap-1 text-sm">
          <NavItem active>Layout</NavItem>
          <NavItem>Devices</NavItem>
          <NavItem>Audio</NavItem>
          <NavItem>Hotkeys</NavItem>
          <NavItem>Advanced</NavItem>
        </nav>
      </aside>
      <main className="flex-1 px-10 py-8">
        <header className="mb-6 flex items-baseline justify-between">
          <h2 className="text-2xl font-semibold">Layout</h2>
          {status ? (
            <p className="text-xs text-neutral-500">
              {status.display_name} · {status.os} ·{" "}
              <span className="font-mono">{status.device_id.slice(0, 8)}</span>
            </p>
          ) : error ? (
            <p className="text-xs text-red-600">daemon offline: {error}</p>
          ) : (
            <p className="text-xs text-neutral-400">connecting…</p>
          )}
        </header>
        <LayoutPage />
      </main>
    </div>
  );
}

function NavItem({
  children,
  active,
}: {
  children: React.ReactNode;
  active?: boolean;
}) {
  return (
    <button
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
