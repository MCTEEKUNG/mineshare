import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useT } from "../i18n";

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
 */
export default function FilesPage() {
  const { t } = useT();
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    const tick = () =>
      invoke<Transfer[]>("get_transfers").then(setTransfers).catch(() => {});
    tick();
    const id = setInterval(tick, 500);
    return () => clearInterval(id);
  }, []);

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
    } catch (e) { setErr(String(e)); }
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
      <div
        className={
          "panel border border-dashed p-12 text-center transition-colors " +
          (dragOver
            ? "border-ds-accent-border bg-ds-accent-soft"
            : "border-ds-border")
        }
      >
        <div className={
          "mx-auto mb-3 size-12 rounded-full flex items-center justify-center transition-all duration-150 " +
          (dragOver ? "bg-ds-accent-soft" : "bg-ds-hover")
        }>
          {dragOver
            ? <IconArrowDown className="size-6 text-ds-accent" />
            : <IconUpload className="size-6 text-ds-text-muted" />
          }
        </div>
        <p className={"text-sm font-medium " + (dragOver ? "text-ds-accent" : "text-ds-text")}>
          {t(dragOver ? "drop_title" : "files_subtitle")}
        </p>
        <p className="text-xs text-ds-text-muted mt-1">
          {t("drop_hint")}
        </p>
      </div>

      <div className="flex justify-end mt-3">
        <button
          onClick={openDownloads}
          className="text-xs text-ds-text-muted hover:text-ds-text transition-colors flex items-center gap-1.5"
        >
          <IconFolder className="size-3.5" />
          {t("files_open")}
        </button>
      </div>

      {err ? <p role="alert" className="text-xs text-ds-danger mt-3">{err}</p> : null}

      {active.length > 0 && (
        <>
          <h3 className="text-sm font-medium text-ds-text-muted mt-8 mb-3">
            {t("files_active")}
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
          <h3 className="text-sm font-medium text-ds-text-muted mt-8 mb-3">
            {t("files_recent")}
          </h3>
          <div className="space-y-2">
            {recent.map((t) => (
              <TransferRow key={t.id} t={t} />
            ))}
          </div>
        </>
      )}

      {active.length === 0 && recent.length === 0 ? (
        <p className="text-sm text-ds-text-muted mt-8 text-center">{t("files_empty")}</p>
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
  const { locale } = useT();
  const pct =
    t.size_bytes === 0
      ? 0
      : Math.min(100, Math.floor((t.bytes_so_far / t.size_bytes) * 100));
  const dirIcon = t.direction === "sending"
    ? <IconArrowUpRight className="size-3.5 text-ds-accent shrink-0" />
    : <IconArrowDownLeft className="size-3.5 text-ds-info shrink-0" />;

  const statusTone =
    t.status === "done"
      ? "text-ds-accent"
      : t.status === "failed" || t.status === "cancelled"
        ? "text-ds-danger"
        : "text-ds-text-muted";
  const inFlight = ["pending", "active", "verifying"].includes(t.status);
  const rate =
    t.seconds_elapsed > 0.1
      ? formatBytes(t.bytes_so_far / t.seconds_elapsed) + "/s"
      : "—";

  return (
    <div className="rounded-xl border border-ds-border bg-ds-surface p-3">
      <div className="flex items-center justify-between gap-3 mb-1.5">
        <div className="min-w-0 flex-1 flex items-start gap-2">
          <span className="mt-0.5">{dirIcon}</span>
          <div className="min-w-0">
            <p className="text-sm font-medium truncate text-ds-text">{t.name}</p>
            <p className="text-[11px] text-ds-text-muted">
              {formatBytes(t.bytes_so_far)} / {formatBytes(t.size_bytes)}
              {inFlight && ` · ${rate}`}
              <span className={" ml-2 font-medium " + statusTone}>· {t.status}</span>
              {t.error ? <span className="text-ds-danger"> — {t.error}</span> : null}
            </p>
          </div>
        </div>
        {inFlight && onCancel ? (
          <button
            onClick={onCancel}
            className="text-[11px] text-ds-text-muted hover:text-ds-danger px-2 py-1 rounded-lg border border-ds-border hover:border-ds-danger-border transition-colors shrink-0"
          >
            {locale === "th" ? "ยกเลิก" : "Cancel"}
          </button>
        ) : null}
      </div>
      {inFlight ? (
        <div className="h-1.5 rounded-full bg-ds-hover overflow-hidden">
          <div
            className={
              "h-full transition-all " +
              (t.direction === "sending" ? "bg-ds-accent" : "bg-ds-info")
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

function IconArrowDown({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <line x1="12" y1="5" x2="12" y2="19" /><polyline points="19 12 12 19 5 12" />
    </svg>
  );
}

function IconUpload({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <polyline points="16 16 12 12 8 16" />
      <line x1="12" y1="12" x2="12" y2="21" />
      <path d="M20.39 18.39A5 5 0 0 0 18 9h-1.26A8 8 0 1 0 3 16.3" />
    </svg>
  );
}

function IconFolder({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
    </svg>
  );
}

function IconArrowUpRight({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <line x1="7" y1="17" x2="17" y2="7" /><polyline points="7 7 17 7 17 17" />
    </svg>
  );
}

function IconArrowDownLeft({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <line x1="17" y1="7" x2="7" y2="17" /><polyline points="17 17 7 17 7 7" />
    </svg>
  );
}
