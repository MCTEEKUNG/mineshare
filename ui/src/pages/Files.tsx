import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";

type Direction = "sending" | "receiving";
type Status =
  | "pending"
  | "active"
  | "verifying"
  | "done"
  | "cancelled"
  | "failed";

type Transfer = {
  id: number;
  direction: Direction;
  status: Status;
  name: string;
  size_bytes: number;
  bytes_so_far: number;
  final_path: string | null;
  error: string | null;
  seconds_elapsed: number;
};

/**
 * File-transfer tab.
 *
 * Drop a file anywhere on the MineShare window → it streams to the
 * paired peer over the encrypted control channel and lands at
 * `Downloads/MineShare/<name>`. Auto-accepted on the receive side
 * because the peer is already in the trust list.
 *
 * Tauri 2 emits drag-drop as **webview-scoped** events (not the
 * global event bus), so we hook them via
 * `getCurrentWebview().onDragDropEvent()` rather than the older
 * `listen('tauri://drag-drop')` pattern. That earlier pattern
 * silently never fires on Tauri 2 — caught us in v0.1, hence the
 * comment.
 */
export default function FilesPage() {
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  // Poll transfer state. 500 ms feels live for progress bars
  // without churning IPC during multi-GB transfers (each call
  // is just an atomic snapshot of a small map).
  useEffect(() => {
    const tick = () =>
      invoke<Transfer[]>("get_transfers").then(setTransfers).catch(() => {});
    tick();
    const id = setInterval(tick, 500);
    return () => clearInterval(id);
  }, []);

  // The actual native drag-drop handler lives in App.tsx so
  // dropping a file works on ANY tab, not just Files. We still
  // mirror the dragOver state here so the in-page drop zone
  // visualises the highlight when the user is hovering this
  // page specifically.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setDragOver(true);
        } else {
          setDragOver(false);
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

  async function cancel(id: number) {
    try {
      await invoke("cancel_transfer", { id });
    } catch {
      /* swallow — cancel is best-effort */
    }
  }

  async function openDownloads() {
    try {
      await invoke("open_downloads_dir");
    } catch (e) {
      setErr(String(e));
    }
  }

  const active = transfers.filter((t) =>
    ["pending", "active", "verifying"].includes(t.status),
  );
  const recent = transfers
    .filter((t) => !["pending", "active", "verifying"].includes(t.status))
    .slice(0, 20);

  return (
    <section>
      <p className="text-sm text-neutral-500 mb-5 max-w-prose">
        Drag any file onto this window to send it to the paired peer. Files
        arrive in <code className="font-mono text-[11px]">Downloads/MineShare/</code>{" "}
        on the other machine, integrity-checked with SHA-256 before being
        renamed into place.
      </p>

      {/*
        The drop zone is intentionally just a visual hint — the OS-level
        drag-drop is handled by Tauri's `onDragDropEvent` which fires on
        ANY drop within the webview, not just on this element. So users
        can drop a file anywhere and it works; this card just tells them
        where to aim.
      */}
      <div
        className={
          "rounded-xl border-2 border-dashed p-10 text-center transition-all duration-150 " +
          (dragOver
            ? "border-emerald-500 bg-emerald-50/80 dark:bg-emerald-950/50 scale-[1.01]"
            : "border-neutral-300 dark:border-neutral-700 bg-neutral-50/40 dark:bg-neutral-900/40")
        }
      >
        <p className="text-4xl mb-2 transition-transform" style={{ transform: dragOver ? "scale(1.15)" : undefined }}>
          {dragOver ? "📥" : "📤"}
        </p>
        <p className="text-sm font-medium">
          {dragOver ? "Drop to send" : "Drag a file anywhere on this window"}
        </p>
        <p className="text-xs text-neutral-500 mt-1">
          Auto-sends to the paired peer · multi-file drop OK · multi-GB OK
        </p>
      </div>

      <div className="flex justify-end mt-3">
        <button
          onClick={openDownloads}
          className="text-xs text-neutral-500 hover:text-neutral-900 dark:hover:text-neutral-100 transition-colors"
        >
          📁 Open Downloads/MineShare folder
        </button>
      </div>

      {err ? <p className="text-xs text-red-600 mt-3">{err}</p> : null}

      {active.length > 0 && (
        <>
          <h3 className="text-xs uppercase tracking-wide text-neutral-500 mt-8 mb-2">
            In progress
          </h3>
          <div className="space-y-2">
            {active.map((t) => (
              <TransferRow key={t.id} t={t} onCancel={() => cancel(t.id)} />
            ))}
          </div>
        </>
      )}

      {recent.length > 0 && (
        <>
          <h3 className="text-xs uppercase tracking-wide text-neutral-500 mt-8 mb-2">
            Recent
          </h3>
          <div className="space-y-2">
            {recent.map((t) => (
              <TransferRow key={t.id} t={t} />
            ))}
          </div>
        </>
      )}

      {active.length === 0 && recent.length === 0 ? (
        <p className="text-xs text-neutral-400 mt-8 text-center">
          No transfers yet.
        </p>
      ) : null}
    </section>
  );
}

function TransferRow({
  t,
  onCancel,
}: {
  t: Transfer;
  onCancel?: () => void;
}) {
  const pct =
    t.size_bytes === 0
      ? 0
      : Math.min(100, Math.floor((t.bytes_so_far / t.size_bytes) * 100));
  const dirIcon = t.direction === "sending" ? "↗" : "↘";
  const dirColor =
    t.direction === "sending"
      ? "text-emerald-600 dark:text-emerald-400"
      : "text-blue-600 dark:text-blue-400";
  const statusTone =
    t.status === "done"
      ? "text-emerald-600 dark:text-emerald-400"
      : t.status === "failed" || t.status === "cancelled"
        ? "text-red-600 dark:text-red-400"
        : "text-neutral-500";
  const inFlight = ["pending", "active", "verifying"].includes(t.status);
  const rate =
    t.seconds_elapsed > 0.1
      ? formatBytes(t.bytes_so_far / t.seconds_elapsed) + "/s"
      : "—";

  return (
    <div className="rounded-lg border border-neutral-200 dark:border-neutral-800 p-3">
      <div className="flex items-center justify-between gap-3 mb-1.5">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium truncate">
            <span className={"mr-1.5 " + dirColor}>{dirIcon}</span>
            {t.name}
          </p>
          <p className="text-[11px] text-neutral-500">
            {formatBytes(t.bytes_so_far)} / {formatBytes(t.size_bytes)}
            {inFlight && ` · ${rate}`}
            <span className={" ml-2 font-medium " + statusTone}>
              · {t.status}
            </span>
            {t.error ? <span className="text-red-600"> — {t.error}</span> : null}
          </p>
        </div>
        {inFlight && onCancel ? (
          <button
            onClick={onCancel}
            className="text-[11px] text-neutral-500 hover:text-red-600 px-2 py-1 rounded border border-neutral-200 dark:border-neutral-800 hover:border-red-300"
          >
            Cancel
          </button>
        ) : null}
      </div>
      {inFlight ? (
        <div className="h-1.5 rounded-full bg-neutral-100 dark:bg-neutral-800 overflow-hidden">
          <div
            className={
              "h-full transition-all " +
              (t.direction === "sending"
                ? "bg-emerald-500"
                : "bg-blue-500")
            }
            style={{ width: `${pct}%` }}
          />
        </div>
      ) : null}
    </div>
  );
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
