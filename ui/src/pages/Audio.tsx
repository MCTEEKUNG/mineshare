import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
 *
 * Below the toggles, a status card summarises whether the local
 * machine has a virtual mic device the peer's mic frames can
 * actually be routed *into* — PipeWire null-sink on Linux,
 * VB-CABLE on Windows. If unavailable on Win, link straight to
 * the VB-CABLE installer.
 */
export default function AudioPage() {
  const [status, setStatus] = useState<AudioStatus | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    const tick = () =>
      invoke<AudioStatus>("get_audio_status")
        .then((s) => {
          setStatus(s);
          setErr(null);
        })
        .catch((e) => setErr(String(e)));
    tick();
    const id = setInterval(tick, 1500);
    return () => clearInterval(id);
  }, []);

  async function toggle(stream: Stream, direction: Direction, enabled: boolean) {
    setErr(null);
    try {
      await invoke("set_audio_toggle", { stream, direction, enabled });
      // Optimistically reflect locally; the next poll re-syncs.
      setStatus((s) =>
        s ? { ...s, [`${direction}_${stream}`]: enabled } as AudioStatus : s,
      );
    } catch (e) {
      setErr(String(e));
    }
  }

  if (!status) {
    return (
      <p className="text-sm text-neutral-400">
        {err ? `failed: ${err}` : "loading audio status…"}
      </p>
    );
  }

  return (
    <section>
      <p className="text-sm text-neutral-500 mb-6 max-w-prose">
        Two pipes per stream — outbound from this machine and
        inbound from the peer. Toggle either side off and the
        capture / playback keeps running for stats but the frames
        stop crossing the silent half. Takes effect on the next
        audio frame, no daemon restart.
      </p>

      <div className="grid gap-4 mb-8">
        <StreamCard
          title="System sound"
          subtitle="WASAPI loopback (Win) / PipeWire monitor (Linux) — captures whatever your default speakers are playing"
          sendOn={status.send_sysout}
          playOn={status.play_sysout}
          onToggle={(dir, on) => toggle("sysout", dir, on)}
        />
        <StreamCard
          title="Microphone"
          subtitle="cpal default input — your physical mic"
          sendOn={status.send_mic}
          playOn={status.play_mic}
          onToggle={(dir, on) => toggle("mic", dir, on)}
        />
      </div>

      <VirtualMicCard backend={status.virtual_mic} os={status.os} />

      {err ? <p className="text-xs text-red-600 mt-3">{err}</p> : null}
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
  return (
    <div className="rounded-lg border border-neutral-200 dark:border-neutral-800 p-5">
      <div className="mb-4">
        <p className="text-base font-semibold">{title}</p>
        <p className="text-xs text-neutral-500 mt-0.5">{subtitle}</p>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
        <ToggleRow
          label="Send to peer"
          hint="forward locally captured frames"
          on={sendOn}
          onChange={(v) => onToggle("send", v)}
        />
        <ToggleRow
          label="Receive from peer"
          hint="render peer's frames here"
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
      className="flex items-center justify-between rounded-md border border-neutral-200 dark:border-neutral-800 px-3 py-3 hover:bg-neutral-50 dark:hover:bg-neutral-900 transition-colors text-left"
    >
      <div>
        <p className="text-sm font-medium">{label}</p>
        <p className="text-[11px] text-neutral-500">{hint}</p>
      </div>
      <Switch on={on} />
    </button>
  );
}

function Switch({ on }: { on: boolean }) {
  return (
    <span
      className={
        "relative inline-block h-5 w-9 rounded-full transition-colors " +
        (on ? "bg-emerald-500" : "bg-neutral-300 dark:bg-neutral-700")
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
          PipeWire null-sink <code className="font-mono">mineshare_mic</code>{" "}
          loaded. Discord / Zoom / OBS see the matching monitor as{" "}
          <strong>"Monitor of MineShare-Mic"</strong> in their input picker.
        </p>
      </Card>
    );
  }
  if (backend === "vbcable") {
    return (
      <Card status="ok" title="Virtual microphone">
        <p>
          VB-CABLE detected. Peer mic frames render into{" "}
          <code className="font-mono">CABLE Input</code>; pick{" "}
          <code className="font-mono">CABLE Output</code> as your mic in any
          app.
        </p>
      </Card>
    );
  }

  // unavailable
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
            className="text-emerald-600 underline underline-offset-2"
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
        <code className="font-mono">pactl load-module module-null-sink</code>{" "}
        failed at startup. Make sure{" "}
        <code className="font-mono">pulseaudio-utils</code> is installed and
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
      ? "border-emerald-300 bg-emerald-50/60 dark:border-emerald-900 dark:bg-emerald-950/30"
      : "border-amber-300 bg-amber-50/60 dark:border-amber-900 dark:bg-amber-950/30";
  return (
    <div className={"rounded-lg border p-5 " + accent}>
      <p className="text-sm font-semibold mb-1">{title}</p>
      <div className="text-xs text-neutral-700 dark:text-neutral-300 leading-relaxed">
        {children}
      </div>
    </div>
  );
}
