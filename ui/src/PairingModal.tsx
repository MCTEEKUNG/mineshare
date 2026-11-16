import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "./i18n";

type PairingPhase = { kind: "none" } | ({ attempt_id: number } & (
  | { kind: "awaitingpin"; peer_name: string; peer_addr: string }
  | { kind: "incomingrequest"; peer_name: string; peer_addr: string }
  | { kind: "waitingapproval"; peer_name: string; peer_addr: string }
  | { kind: "displayingpin"; pin: string; peer_name: string; peer_addr: string }
  | { kind: "verifying" }
  | { kind: "trusted"; peer_name: string }
  | { kind: "failed"; reason: string }));

/**
 * Targeted pairing consent and first-time PIN verification.
 * Selected trusted computers reconnect silently; explicit destination changes
 * still require the receiver's approval but skip the repeat PIN exchange.
 */
export default function PairingModal() {
  const [phase, setPhase] = useState<PairingPhase>({ kind: "none" });
  const [error, setError] = useState<string | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [responding, setResponding] = useState(false);
  const dialog = useRef<HTMLDivElement>(null);
  const cancelButton = useRef<HTMLButtonElement>(null);
  const { t } = useT();

  useEffect(() => {
    let active = true;
    const tick = () =>
      invoke<PairingPhase>("get_pairing_phase").then((value) => { if (active) setPhase(value); }).catch(() => {});
    tick();
    const id = setInterval(tick, 500);
    return () => { active = false; clearInterval(id); };
  }, []);

  const visible = phase.kind !== "none";
  const attemptId = phase.kind === "none" ? 0 : phase.attempt_id;
  useEffect(() => {
    if (!visible) return;
    const previous = document.activeElement as HTMLElement;
    return () => previous?.focus();
  }, [visible]);
  useEffect(() => {
    setResponding(false); setError(null);
    dialog.current?.querySelector<HTMLElement>("input, button")?.focus();
  }, [phase.kind, attemptId]);

  if (phase.kind === "none") return null;

  return (
    <div ref={dialog} role="dialog" aria-modal="true" aria-label={t("pair_title_request")} tabIndex={-1} className="fixed inset-0 z-50 bg-black/60 backdrop-blur-md flex items-center justify-center p-6" onKeyDown={event => {
      if (event.key === "Escape") { event.preventDefault(); cancelButton.current?.click(); }
      if (event.key === "Tab") {
        const controls = Array.from(dialog.current?.querySelectorAll<HTMLElement>('input:not(:disabled), button:not(:disabled)') ?? []);
        const first = controls[0], last = controls[controls.length - 1];
        if (!first) { event.preventDefault(); dialog.current?.focus(); }
        else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
      }
    }}>
      <div className="panel w-full max-w-md p-6 shadow-2xl shadow-black/40">
        {phase.kind === "incomingrequest" ? <>
          <h2 className="text-xl font-semibold">{t("pair_incoming_title")}</h2>
          <p className="text-lg font-medium mt-5 break-words">{phase.peer_name}</p><p className="text-xs text-ds-text-muted mt-1 break-all">{phase.peer_addr}</p>
          <p className="muted mt-4">{t("pair_incoming_hint")}</p>
          <button disabled={responding || cancelling} className="button-primary w-full mt-6" onClick={async () => {
            setResponding(true); setError(null);
            try { await invoke("respond_pairing_request", { accept: true, attemptId: phase.attempt_id }); }
            catch (e) { setError(String(e)); setResponding(false); }
          }}>{t("pair_accept")}</button>
        </> : null}
        {phase.kind === "waitingapproval" ? <CenteredMessage title={t("pair_waiting_title")} subtitle={`${phase.peer_name} · ${phase.peer_addr}`} /> : null}
        {phase.kind === "displayingpin" ? <DisplayPin {...phase} /> : null}
        {phase.kind === "awaitingpin" ? <EnterPin key={phase.attempt_id} {...phase} /> : null}
        {phase.kind === "verifying" ? (
          <CenteredMessage
            title={t("pair_verifying")}
            subtitle={t("pair_verifying_sub")}
          />
        ) : null}
        <button ref={cancelButton} disabled={cancelling || responding} className="button-secondary mt-4 w-full"
          onClick={async () => {
            setCancelling(true);
            setError(null);
            try {
              if (phase.kind === "incomingrequest") await invoke("respond_pairing_request", { accept: false, attemptId: phase.attempt_id });
              else await invoke("set_pairing_enabled", { enabled: false });
              setPhase({ kind: "none" });
            } catch (e) { setError(String(e)); }
            finally { setCancelling(false); }
          }}>
          {t(phase.kind === "incomingrequest" ? "pair_reject" : "pair_cancel")}
        </button>
        {error ? <p role="alert" className="mt-2 text-sm text-ds-danger">{error}</p> : null}
        {phase.kind === "trusted" ? (
          <CenteredMessage
            title={t("pair_success_title")}
            subtitle={`${phase.peer_name} ${t("pair_success_sub")}`}
            tone="success"
          />
        ) : null}
        {phase.kind === "failed" ? (
          <CenteredMessage
            title={t("pair_failed_title")}
            subtitle={phase.reason}
            tone="error"
          />
        ) : null}
      </div>
    </div>
  );
}

function DisplayPin({ pin, peer_addr, peer_name }: { pin: string; peer_addr: string; peer_name: string }) {
  const { t } = useT();
  return (
    <div>
      <p className="text-[10px] uppercase tracking-widest text-ds-text-muted mb-2">
        {t("pair_title_request")}
      </p>
      <p className="text-sm mb-6 text-ds-text break-words">{peer_name}<span className="block text-xs text-ds-text-muted mt-1">{peer_addr}</span></p>
      <p className="text-sm text-ds-text-muted mb-3">{t("pair_show_pin")}</p>
      <p className="font-mono text-5xl font-bold text-center tracking-[0.3em] py-6 my-2 rounded-xl bg-ds-accent-soft border border-ds-accent-border text-ds-accent">
        {pin}
      </p>
      <p className="text-[11px] text-ds-text-muted mt-4 text-center">{t("pair_cancel_note")}</p>
    </div>
  );
}

function EnterPin({ peer_addr, peer_name, attempt_id }: { peer_addr: string; peer_name: string; attempt_id: number }) {
  const [value, setValue] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { t } = useT();

  async function submit() {
    if (value.length !== 6 || submitting) return;
    setSubmitting(true);
    try {
      await invoke("submit_pin", { pin: value, attemptId: attempt_id });
    } catch (e) {
      setError(String(e));
      setSubmitting(false);
    }
  }

  return (
    <div>
      <p className="text-[10px] uppercase tracking-widest text-ds-text-muted mb-2">
        {t("pair_title_pair_with")}
      </p>
      <p className="text-sm mb-6 text-ds-text break-words">{peer_name}<span className="block text-xs text-ds-text-muted mt-1">{peer_addr}</span></p>
      <p className="text-sm text-ds-text-muted mb-3">{t("pair_enter_pin")}</p>
      <input
        type="text"
        inputMode="numeric"
        pattern="[0-9]{6}"
        maxLength={6}
        autoFocus
        value={value}
        onChange={(e) => setValue(e.target.value.replace(/\D/g, "").slice(0, 6))}
        onKeyDown={(e) => { if (e.key === "Enter") submit(); }}
        className="w-full font-mono text-5xl font-bold text-center tracking-[0.3em] py-6 rounded-xl bg-ds-hover border-2 border-ds-border text-ds-text focus:outline-none focus:border-ds-accent transition-colors placeholder:text-ds-text-muted"
        placeholder="──────"
      />
      <button
        onClick={submit}
        disabled={value.length !== 6 || submitting}
        className="mt-4 w-full rounded-xl bg-ds-accent hover:bg-ds-accent-strong disabled:opacity-40 disabled:cursor-not-allowed text-ds-on-accent text-sm font-semibold py-2.5 transition-all duration-150 shadow-lg"
      >
        {submitting ? t("pair_button_sending") : t("pair_button")}
      </button>
      <p className="text-[11px] text-ds-text-muted mt-3 text-center">{t("pair_after_note")}</p>
      {error ? <p role="alert" className="text-sm text-ds-danger mt-2">{error}</p> : null}
    </div>
  );
}

function CenteredMessage({
  title,
  subtitle,
  tone,
}: {
  title: string;
  subtitle: string;
  tone?: "success" | "error";
}) {
  const accent =
    tone === "success"
      ? "text-ds-success"
      : tone === "error"
        ? "text-ds-danger"
        : "text-ds-text";
  return (
    <div className="text-center py-4">
      <p className={"text-2xl font-semibold mb-2 " + accent}>{title}</p>
      <p className="text-sm text-ds-text-muted">{subtitle}</p>
    </div>
  );
}
