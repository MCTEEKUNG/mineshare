import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "./i18n";

type PairingPhase =
  | { kind: "none" }
  | { kind: "awaitingpin"; peer_name: string; peer_addr: string }
  | { kind: "displayingpin"; pin: string; peer_name: string; peer_addr: string }
  | { kind: "verifying" }
  | { kind: "trusted"; peer_name: string }
  | { kind: "failed"; reason: string };

/**
 * Stage 7 PIN-pairing modal.
 *
 * Drawn as a full-screen overlay whenever the runtime's pairing
 * phase is anything but `none`. The acceptor sees a big PIN to
 * read aloud; the dialer sees an input box to type the PIN they
 * heard. After the round-trip we surface success / failure for
 * a couple of seconds before slipping back to the normal Status
 * tab. Once a peer is in the trust list (`trusted_peers.json`)
 * future sessions skip this modal and connect silently.
 */
export default function PairingModal() {
  const [phase, setPhase] = useState<PairingPhase>({ kind: "none" });
  const { t } = useT();

  useEffect(() => {
    const tick = () =>
      invoke<PairingPhase>("get_pairing_phase").then(setPhase).catch(() => {});
    tick();
    const id = setInterval(tick, 500);
    return () => clearInterval(id);
  }, []);

  if (phase.kind === "none") return null;

  return (
    <div className="fixed inset-0 z-50 bg-black/60 backdrop-blur-md flex items-center justify-center p-6">
      <div className="w-full max-w-md rounded-2xl border border-white/[0.08] bg-ds-surface p-6 shadow-2xl shadow-black/40">
        {phase.kind === "displayingpin" ? <DisplayPin {...phase} /> : null}
        {phase.kind === "awaitingpin" ? <EnterPin {...phase} /> : null}
        {phase.kind === "verifying" ? (
          <CenteredMessage
            title={t("pair_verifying")}
            subtitle={t("pair_verifying_sub")}
          />
        ) : null}
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

function DisplayPin({ pin, peer_addr }: { pin: string; peer_addr: string }) {
  const { t } = useT();
  return (
    <div>
      <p className="text-[10px] uppercase tracking-widest text-slate-400 mb-2">
        {t("pair_title_request")}
      </p>
      <p className="font-mono text-sm mb-6 text-slate-300">{peer_addr}</p>
      <p className="text-sm text-slate-400 mb-3">{t("pair_show_pin")}</p>
      <p className="font-mono text-5xl font-bold text-center tracking-[0.3em] py-6 my-2 rounded-xl bg-emerald-500/10 border border-emerald-500/25 text-emerald-300">
        {pin}
      </p>
      <p className="text-[11px] text-slate-400 mt-4 text-center">{t("pair_cancel_note")}</p>
    </div>
  );
}

function EnterPin({ peer_addr }: { peer_addr: string }) {
  const [value, setValue] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const { t } = useT();

  async function submit() {
    if (value.length !== 6 || submitting) return;
    setSubmitting(true);
    try {
      await invoke("submit_pin", { pin: value });
    } catch (e) {
      console.error("submit_pin failed", e);
      setSubmitting(false);
    }
  }

  return (
    <div>
      <p className="text-[10px] uppercase tracking-widest text-slate-400 mb-2">
        {t("pair_title_pair_with")}
      </p>
      <p className="font-mono text-sm mb-6 text-slate-300">{peer_addr}</p>
      <p className="text-sm text-slate-400 mb-3">{t("pair_enter_pin")}</p>
      <input
        type="text"
        inputMode="numeric"
        pattern="[0-9]{6}"
        maxLength={6}
        autoFocus
        value={value}
        onChange={(e) => setValue(e.target.value.replace(/\D/g, "").slice(0, 6))}
        onKeyDown={(e) => { if (e.key === "Enter") submit(); }}
        className="w-full font-mono text-5xl font-bold text-center tracking-[0.3em] py-6 rounded-xl bg-white/[0.04] border-2 border-white/[0.10] text-slate-100 focus:outline-none focus:border-emerald-500/60 transition-colors placeholder-slate-700"
        placeholder="──────"
      />
      <button
        onClick={submit}
        disabled={value.length !== 6 || submitting}
        className="mt-4 w-full rounded-xl bg-emerald-500 hover:bg-emerald-400 disabled:opacity-40 disabled:cursor-not-allowed text-white text-sm font-semibold py-2.5 transition-all duration-150 shadow-lg shadow-emerald-500/20"
      >
        {submitting ? t("pair_button_sending") : t("pair_button")}
      </button>
      <p className="text-[11px] text-slate-400 mt-3 text-center">{t("pair_after_note")}</p>
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
      ? "text-emerald-400"
      : tone === "error"
        ? "text-red-400"
        : "text-slate-300";
  return (
    <div className="text-center py-4">
      <p className={"text-2xl font-semibold mb-2 " + accent}>{title}</p>
      <p className="text-sm text-slate-400">{subtitle}</p>
    </div>
  );
}
