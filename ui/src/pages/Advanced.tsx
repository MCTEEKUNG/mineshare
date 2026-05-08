import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
};

/**
 * Advanced tab — diagnostics + power-user info that doesn't
 * belong anywhere else: full traffic counters, error counts,
 * platform notes, and pointers to the on-disk config / log
 * locations the daemon writes to.
 */
export default function AdvancedPage() {
  const [s, setS] = useState<Status | null>(null);

  useEffect(() => {
    const tick = () => invoke<Status>("get_status").then(setS).catch(() => {});
    tick();
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, []);

  return (
    <section className="grid gap-6">
      <div className="rounded-lg border border-neutral-200 dark:border-neutral-800 p-5">
        <p className="text-base font-semibold mb-3">Traffic</p>
        {s ? (
          <dl className="grid grid-cols-2 md:grid-cols-3 gap-x-8 gap-y-3 text-sm">
            <Row k="Sent packets" v={s.sent_pkts.toLocaleString()} />
            <Row k="Received packets" v={s.recv_pkts.toLocaleString()} />
            <Row k="Audio frames recv" v={s.audio_recv.toLocaleString()} />
            <Row k="Injected events" v={s.injected.toLocaleString()} />
            <Row
              k="Inject errors"
              v={s.inject_errs.toLocaleString()}
              alert={s.inject_errs > 0}
            />
            <Row
              k="Decrypt errors"
              v={s.decrypt_errs.toLocaleString()}
              alert={s.decrypt_errs > 0}
            />
          </dl>
        ) : (
          <p className="text-sm text-neutral-400">connecting…</p>
        )}
      </div>

      <div className="rounded-lg border border-neutral-200 dark:border-neutral-800 p-5">
        <p className="text-base font-semibold mb-3">Local files</p>
        <ul className="text-xs space-y-2 text-neutral-700 dark:text-neutral-300">
          <FileEntry
            label="Daemon log"
            winPath="%APPDATA%\MineShare\logs\daemon.YYYY-MM-DD"
            linuxPath="~/.config/MineShare/logs/daemon.YYYY-MM-DD"
          />
          <FileEntry
            label="Layout config"
            winPath="%APPDATA%\MineShare\layout.json"
            linuxPath="~/.config/MineShare/layout.json"
          />
          <FileEntry
            label="Identity (Noise XX private key)"
            winPath="%APPDATA%\MineShare\identity.json"
            linuxPath="~/.config/MineShare/identity.json"
          />
        </ul>
      </div>

      <div className="rounded-lg border border-neutral-200 dark:border-neutral-800 p-5">
        <p className="text-base font-semibold mb-3">Platform notes</p>
        <ul className="text-xs space-y-1.5 text-neutral-700 dark:text-neutral-300 list-disc pl-5">
          <li>
            Closing the window hides MineShare to the system tray; the
            daemon and bridge keep running. Use the tray icon's "Quit
            MineShare" entry to actually exit.
          </li>
          <li>
            Layout side auto-syncs across the encrypted control
            channel — drag the peer tile on either machine and the
            other side flips its mirror automatically.
          </li>
          <li>
            Linux clipboard sync uses arboard's Wayland data-control
            protocol when available, falling back to Xwayland; if you
            see "X11 server connection timed out" the launcher script
            didn't pick up XAUTHORITY.
          </li>
        </ul>
      </div>
    </section>
  );
}

function Row({ k, v, alert }: { k: string; v: string; alert?: boolean }) {
  return (
    <div>
      <dt className="text-[11px] uppercase tracking-wide text-neutral-500 mb-1">
        {k}
      </dt>
      <dd
        className={
          "text-sm font-medium " +
          (alert ? "text-red-600 dark:text-red-400" : "")
        }
      >
        {v}
      </dd>
    </div>
  );
}

function FileEntry({
  label,
  winPath,
  linuxPath,
}: {
  label: string;
  winPath: string;
  linuxPath: string;
}) {
  return (
    <li>
      <p className="font-medium text-neutral-900 dark:text-neutral-100">{label}</p>
      <p>
        <span className="text-neutral-500">win: </span>
        <code className="font-mono">{winPath}</code>
      </p>
      <p>
        <span className="text-neutral-500">linux: </span>
        <code className="font-mono">{linuxPath}</code>
      </p>
    </li>
  );
}
