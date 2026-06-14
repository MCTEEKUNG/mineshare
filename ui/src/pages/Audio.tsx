import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "../i18n";

type VirtualMicBackend = "pipewire" | "vbcable" | "unavailable";

type AudioStatus = {
  send_sysout: boolean;
  play_sysout: boolean;
  send_mic: boolean;
  play_mic: boolean;
  virtual_mic: VirtualMicBackend;
  os: string;
};

type Stream = "sysout" | "mic";
type Direction = "send" | "play";

/**
 * Audio settings tab.
 *
 * Two streams (system sound + microphone), each with two toggle
 * directions ("send to peer" / "render from peer"). All four
 * round-trip through `set_audio_toggle`, which flips a single
 * AtomicBool the runtime's pump tasks check on every frame —
 * the change takes effect on the next 20 ms frame, no daemon
 * restart needed.
 */
export default function AudioPage() {
  const [status, setStatus] = useState<AudioStatus | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const { t } = useT();

  useEffect(() => {
    const tick = () =>
      invoke<AudioStatus>("get_audio_status")
        .then((s) => { setStatus(s); setErr(null); })
        .catch((e) => setErr(String(e)));
    tick();
    const id = setInterval(tick, 1500);
    return () => clearInterval(id);
  }, []);

  async function toggle(stream: Stream, direction: Direction, enabled: boolean) {
    setErr(null);
    try {
      await invoke("set_audio_toggle", { stream, direction, enabled });
      setStatus((s) =>
        s ? { ...s, [`${direction}_${stream}`]: enabled } as AudioStatus : s,
      );
    } catch (e) {
      setErr(String(e));
    }
  }

  if (!status) {
    return (
      <p className="text-sm text-slate-400">
        {err ? `failed: ${err}` : "…"}
      </p>
    );
  }

  return (
    <section>
      <p className="text-sm text-slate-400 mb-6 max-w-prose leading-relaxed">
        {t("audio_intro")}
      </p>

      <div className="grid gap-4 mb-8">
        <StreamCard
          title={t("audio_sysout_title")}
          subtitle={t("audio_sysout_sub")}
          sendOn={status.send_sysout}
          playOn={status.play_sysout}
          onToggle={(dir, on) => toggle("sysout", dir, on)}
        />
        <StreamCard
          title={t("audio_mic_title")}
          subtitle={t("audio_mic_sub")}
          sendOn={status.send_mic}
          playOn={status.play_mic}
          onToggle={(dir, on) => toggle("mic", dir, on)}
        />
      </div>

      <VirtualMicCard backend={status.virtual_mic} os={status.os} />

      {err ? <p className="text-xs text-red-400 mt-3">{err}</p> : null}
    </section>
  );
}

function StreamCard({
  title,
  subtitle,
  sendOn,
  playOn,
  onToggle,
}: {
  title: string;
  subtitle: string;
  sendOn: boolean;
  playOn: boolean;
  onToggle: (direction: Direction, enabled: boolean) => void;
}) {
  const { t } = useT();
  return (
    <div className="rounded-xl border border-white/[0.08] bg-ds-surface p-5">
      <div className="mb-4">
        <p className="text-base font-semibold text-slate-100">{title}</p>
        <p className="text-xs text-slate-400 mt-0.5">{subtitle}</p>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        <ToggleRow
          label={t("audio_send")}
          hint={t("audio_send_hint")}
          on={sendOn}
          onChange={(v) => onToggle("send", v)}
        />
        <ToggleRow
          label={t("audio_play")}
          hint={t("audio_play_hint")}
          on={playOn}
          onChange={(v) => onToggle("play", v)}
        />
      </div>
    </div>
  );
}

function ToggleRow({
  label,
  hint,
  on,
  onChange,
}: {
  label: string;
  hint: string;
  on: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      onClick={() => onChange(!on)}
      className="flex items-center justify-between rounded-lg border border-white/[0.06] bg-white/[0.03] px-3 py-3 hover:bg-white/[0.06] transition-colors text-left"
    >
      <div>
        <p className="text-sm font-medium text-slate-200">{label}</p>
        <p className="text-[11px] text-slate-400 mt-0.5">{hint}</p>
      </div>
      <Switch on={on} />
    </button>
  );
}

function Switch({ on }: { on: boolean }) {
  return (
    <span
      className={
        "relative inline-block h-5 w-9 rounded-full transition-colors shrink-0 " +
        (on ? "bg-emerald-500" : "bg-white/[0.12]")
      }
    >
      <span
        className={
          "absolute top-0.5 size-4 rounded-full bg-white shadow transition-transform " +
          (on ? "translate-x-[18px]" : "translate-x-0.5")
        }
      />
    </span>
  );
}

function VirtualMicCard({
  backend,
  os,
}: {
  backend: VirtualMicBackend;
  os: string;
}) {
  if (backend === "pipewire") {
    return (
      <Card status="ok" title="Virtual microphone">
        <p>
          PipeWire null-sink <code className="font-mono text-emerald-400">mineshare_mic</code>{" "}
          loaded. Discord / Zoom / OBS see the matching monitor as{" "}
          <strong className="text-slate-200">"Monitor of MineShare-Mic"</strong> in their input picker.
        </p>
      </Card>
    );
  }
  if (backend === "vbcable") {
    return (
      <Card status="ok" title="Virtual microphone">
        <p>
          VB-CABLE detected. Peer mic frames render into{" "}
          <code className="font-mono text-emerald-400">CABLE Input</code>; pick{" "}
          <code className="font-mono text-emerald-400">CABLE Output</code> as your mic in any app.
        </p>
      </Card>
    );
  }

  if (os === "windows") {
    return (
      <Card status="warn" title="Virtual microphone — VB-CABLE not detected">
        <p>
          The bridge keeps working, but apps on this machine can't pick up the
          peer's mic until VB-CABLE is installed.{" "}
          <a
            href="https://vb-audio.com/Cable/"
            target="_blank"
            rel="noopener noreferrer"
            className="text-emerald-400 underline underline-offset-2 hover:text-emerald-300 transition-colors"
          >
            Install from vb-audio.com/Cable
          </a>{" "}
          and restart MineShare.
        </p>
      </Card>
    );
  }
  return (
    <Card status="warn" title="Virtual microphone — unavailable">
      <p>
        <code className="font-mono text-slate-300">pactl load-module module-null-sink</code>{" "}
        failed at startup. Make sure{" "}
        <code className="font-mono text-slate-300">pulseaudio-utils</code> is installed and
        that you're running a PipeWire session, then restart MineShare.
      </p>
    </Card>
  );
}

function Card({
  status,
  title,
  children,
}: {
  status: "ok" | "warn";
  title: string;
  children: React.ReactNode;
}) {
  const accent =
    status === "ok"
      ? "border-emerald-500/25 bg-emerald-500/[0.07]"
      : "border-amber-500/25 bg-amber-500/[0.07]";
  const titleColor = status === "ok" ? "text-emerald-300" : "text-amber-300";
  return (
    <div className={"rounded-xl border p-5 " + accent}>
      <p className={"text-sm font-semibold mb-1.5 " + titleColor}>{title}</p>
      <div className="text-xs text-slate-400 leading-relaxed">
        {children}
      </div>
    </div>
  );
}
