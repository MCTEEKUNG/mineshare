import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "../i18n";

type VirtualMicBackend = "pipewire" | "vbcable" | "unavailable";
type EffectiveAudioState = "disabled" | "idle" | "starting" | "active" | "degraded" | "stopped";

type AudioStatus = {
  send_sysout: boolean;
  play_sysout: boolean;
  send_mic: boolean;
  play_mic: boolean;
  send_sysout_state: EffectiveAudioState;
  play_sysout_state: EffectiveAudioState;
  send_mic_state: EffectiveAudioState;
  play_mic_state: EffectiveAudioState;
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
  const [busy, setBusy] = useState(false);
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
    setBusy(true);
    try {
      await invoke("set_audio_toggle", { stream, direction, enabled });
      setStatus((s) =>
        s ? { ...s, [`${direction}_${stream}`]: enabled, [`${direction}_${stream}_state`]: enabled ? "starting" : "disabled" } as AudioStatus : s,
      );
    } catch (e) {
      setErr(String(e));
    }
    finally { setBusy(false); }
  }

  if (!status) {
    return (
      <p className="text-sm text-ds-text-muted">
        {err ? `failed: ${err}` : "…"}
      </p>
    );
  }

  return (
    <section>
      <fieldset disabled={busy} className="grid gap-4 mb-6">
        <StreamCard
          title={t("audio_sysout_title")}
          subtitle={t("audio_sysout_sub")}
          sendOn={status.send_sysout}
          playOn={status.play_sysout}
          sendState={status.send_sysout_state}
          playState={status.play_sysout_state}
          onToggle={(dir, on) => toggle("sysout", dir, on)}
        />
        <StreamCard
          title={t("audio_mic_title")}
          subtitle={t("audio_mic_sub")}
          sendOn={status.send_mic}
          playOn={status.play_mic}
          sendState={status.send_mic_state}
          playState={status.play_mic_state}
          onToggle={(dir, on) => toggle("mic", dir, on)}
        />
      </fieldset>

      <VirtualMicCard backend={status.virtual_mic} os={status.os} />

      {err ? <p role="alert" className="text-xs text-ds-danger mt-3">{err}</p> : null}
    </section>
  );
}

function StreamCard({
    title,
    subtitle,
    sendOn,
    playOn,
    sendState,
    playState,
    onToggle,
}: {
  title: string;
  subtitle: string;
  sendOn: boolean;
  playOn: boolean;
  sendState: EffectiveAudioState;
  playState: EffectiveAudioState;
  onToggle: (direction: Direction, enabled: boolean) => void;
}) {
  const { t } = useT();
  return (
    <section aria-label={title} className="panel p-5">
      <div className="mb-4">
        <p className="text-base font-semibold text-ds-text">{title}</p>
        <p className="text-xs text-ds-text-muted mt-0.5">{subtitle}</p>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        <ToggleRow
          label={t("audio_send")}
          accessibleLabel={`${title}: ${t("audio_send")}`}
          hint={t("audio_send_hint")}
          on={sendOn}
          state={sendState}
          onChange={(v) => onToggle("send", v)}
        />
        <ToggleRow
          label={t("audio_play")}
          accessibleLabel={`${title}: ${t("audio_play")}`}
          hint={t("audio_play_hint")}
          on={playOn}
          state={playState}
          onChange={(v) => onToggle("play", v)}
        />
      </div>
    </section>
  );
}

function ToggleRow({
  label,
  accessibleLabel,
  hint,
  on,
  state,
  onChange,
}: {
  label: string;
  accessibleLabel: string;
  hint: string;
  on: boolean;
  state: EffectiveAudioState;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      onClick={() => onChange(!on)}
      role="switch"
      aria-checked={on}
      aria-label={accessibleLabel}
      className="flex items-center justify-between rounded-lg border border-ds-border bg-ds-hover px-3 py-3 hover:bg-ds-hover transition-colors text-left"
    >
      <div>
        <div className="flex items-center gap-2">
          <p className="text-sm font-medium text-ds-text">{label}</p>
          <BackendBadge state={state} />
        </div>
        <p className="text-[11px] text-ds-text-muted mt-0.5">{hint}</p>
      </div>
      <Switch on={on} />
    </button>
  );
}

function BackendBadge({ state }: { state: EffectiveAudioState }) {
  const { locale } = useT();
  const labels = { active: "ใช้งานอยู่", starting: "กำลังเริ่ม", degraded: "มีปัญหา", stopped: "หยุดแล้ว", disabled: "ปิดอยู่", idle: "รอเชื่อมต่อ" };
  const tone =
    state === "active"
      ? "text-ds-success"
      : state === "starting"
        ? "text-ds-warning"
        : state === "degraded" || state === "stopped"
          ? "text-ds-danger"
          : "text-ds-text-muted";
  return <span className={`text-[11px] ${tone}`}>{locale === "th" ? labels[state] : state}</span>;
}

function Switch({ on }: { on: boolean }) {
  return (
    <span
      className={
        "relative inline-block h-5 w-9 rounded-full transition-colors shrink-0 " +
        (on ? "bg-[#4b6de8]" : "bg-[#777471]")
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
  const { locale } = useT();
  const th = locale === "th";
  if (backend === "pipewire") {
    return (
      <Card status="ok" title={th ? "ไมโครโฟนเสมือนพร้อมใช้งาน" : "Virtual microphone ready"}>
        <p>
          {th ? "เลือก " : "Choose "}<strong className="text-ds-text">Monitor of MineShare-Mic</strong>{th ? " เป็นไมโครโฟนในแอปที่จะใช้งาน" : " as the microphone in the app you want to use."}
        </p>
      </Card>
    );
  }
  if (backend === "vbcable") {
    return (
      <Card status="ok" title={th ? "ไมโครโฟนเสมือนพร้อมใช้งาน" : "Virtual microphone ready"}>
        <p>
          {th ? "เลือก " : "Choose "}<strong className="text-ds-text">CABLE Output</strong>{th ? " เป็นไมโครโฟนในแอปที่จะใช้งาน เพื่อใช้เสียงจากอีกเครื่อง" : " as the microphone in your app to use audio from the other computer."}
        </p>
      </Card>
    );
  }

  if (os === "windows") {
    return (
      <Card status="warn" title={th ? "ยังใช้ไมโครโฟนจากอีกเครื่องไม่ได้" : "Peer microphone needs setup"}>
        <p>
          {th ? "ติดตั้ง VB-CABLE แล้วเปิด MineShare ใหม่ เพื่อใช้ไมโครโฟนจากอีกเครื่องในแอปของคุณ " : "Install VB-CABLE and restart MineShare to use the other computer's microphone in your apps. "}
          <a
            href="https://vb-audio.com/Cable/"
            target="_blank"
            rel="noopener noreferrer"
            className="text-ds-accent underline underline-offset-2 hover:text-ds-accent-strong transition-colors"
          >
            {th ? "ดาวน์โหลด VB-CABLE" : "Download VB-CABLE"}
          </a>{" "}
        </p>
      </Card>
    );
  }
  return (
    <Card status="warn" title={th ? "ไมโครโฟนเสมือนไม่พร้อมใช้งาน" : "Virtual microphone unavailable"}>
      <p>
        {th ? "ตรวจว่าเครื่องนี้ใช้ PipeWire และติดตั้ง pulseaudio-utils แล้ว จากนั้นเปิด MineShare ใหม่" : "Check that PipeWire is running and pulseaudio-utils is installed, then restart MineShare."}
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
  const titleColor = status === "ok" ? "text-ds-text" : "text-ds-warning";
  return (
    <div className="panel p-5">
      <p className={"text-sm font-semibold mb-1.5 " + titleColor}>{title}</p>
      <div className="text-xs text-ds-text-muted leading-relaxed">
        {children}
      </div>
    </div>
  );
}
