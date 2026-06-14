import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type DeviceInfo = { name: string; is_default: boolean };
type DevicesSnapshot = {
  outputs: DeviceInfo[];
  inputs: DeviceInfo[];
  selected_output: string | null;
  selected_input: string | null;
};
type Direction = "output" | "input";

/**
 * Devices tab — pick the cpal output / input device the bridge
 * uses for the peer's sysout playback and the local mic capture.
 */
export default function DevicesPage() {
  const [devs, setDevs] = useState<DevicesSnapshot | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [pending, setPending] = useState<Direction | null>(null);

  async function refresh(force = false) {
    try {
      if (force) await invoke("refresh_audio_devices");
      const s = await invoke<DevicesSnapshot>("list_audio_devices");
      setDevs(s);
      setErr(null);
    } catch (e) {
      setErr(String(e));
    }
  }

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, 10_000);
    return () => clearInterval(id);
  }, []);

  async function pick(dir: Direction, name: string | null) {
    setPending(dir);
    setErr(null);
    try {
      const cmd = dir === "output" ? "set_audio_output_device" : "set_audio_input_device";
      await invoke(cmd, { name });
      setDevs((s) =>
        s ? { ...s, [dir === "output" ? "selected_output" : "selected_input"]: name } : s,
      );
    } catch (e) {
      setErr(String(e));
    } finally {
      setPending(null);
    }
  }

  if (!devs) {
    return (
      <p className="text-sm text-ds-text-muted">
        {err ? `failed: ${err}` : "loading devices…"}
      </p>
    );
  }

  return (
    <section className="grid gap-6">
      <DeviceList
        icon={<IconSpeaker className="size-5 text-ds-text-muted" />}
        title="Audio output"
        subtitle="Where peer sysout (and peer mic on Win, when VB-CABLE is installed) renders."
        devices={devs.outputs}
        selected={devs.selected_output}
        busy={pending === "output"}
        onPick={(n) => pick("output", n)}
        onRefresh={() => refresh(true)}
      />
      <DeviceList
        icon={<IconMic className="size-5 text-ds-text-muted" />}
        title="Audio input"
        subtitle="Where the bridge captures your mic. Pick a non-default device for headset / USB mic / OBS virtual cam, etc."
        devices={devs.inputs}
        selected={devs.selected_input}
        busy={pending === "input"}
        onPick={(n) => pick("input", n)}
        onRefresh={() => refresh(true)}
      />
      {err ? <p className="text-xs text-red-400">{err}</p> : null}
    </section>
  );
}

function DeviceList({
  icon,
  title,
  subtitle,
  devices,
  selected,
  busy,
  onPick,
  onRefresh,
}: {
  icon: React.ReactNode;
  title: string;
  subtitle: string;
  devices: DeviceInfo[];
  selected: string | null;
  busy: boolean;
  onPick: (name: string | null) => void;
  onRefresh: () => void;
}) {
  const followingDefault = selected === null;
  return (
    <div className="rounded-xl border border-ds-border bg-ds-surface overflow-hidden">
      <div className="flex items-center justify-between px-5 pt-4 pb-3 border-b border-ds-border">
        <div className="flex items-center gap-3 min-w-0">
          <span className="shrink-0">{icon}</span>
          <div className="min-w-0">
            <p className="text-base font-semibold text-ds-text leading-tight">{title}</p>
            <p className="text-xs text-ds-text-muted mt-0.5 max-w-md truncate">{subtitle}</p>
          </div>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          {busy ? <span className="text-[11px] text-ds-text-muted">switching…</span> : null}
          <button
            onClick={onRefresh}
            className="text-[11px] text-ds-text-muted hover:text-ds-text px-2 py-1 rounded-lg hover:bg-ds-hover transition-colors"
            title="Re-scan devices"
          >
            ↻ refresh
          </button>
        </div>
      </div>

      <ul className="divide-y divide-ds-border">
        <DeviceRow
          name="Follow system default"
          hint="Whatever the OS picks; the bridge re-targets if it changes."
          active={followingDefault}
          onClick={() => onPick(null)}
        />
        {devices.map((d, i) => (
          <DeviceRow
            key={i}
            name={d.name}
            hint={d.is_default ? "current OS default" : undefined}
            active={selected === d.name}
            onClick={() => onPick(d.name)}
          />
        ))}
        {devices.length === 0 ? (
          <li className="px-5 py-8 text-center text-sm text-ds-text-muted">
            none reported by cpal
          </li>
        ) : null}
      </ul>
    </div>
  );
}

function DeviceRow({
  name,
  hint,
  active,
  onClick,
}: {
  name: string;
  hint?: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <li>
      <button
        onClick={onClick}
        title={name}
        className={
          "w-full flex items-center justify-between gap-4 px-5 py-3 transition-colors text-left " +
          (active
            ? "bg-emerald-500/[0.08]"
            : "hover:bg-ds-hover")
        }
      >
        <div className="min-w-0 flex-1">
          <p className={
            "text-sm truncate " +
            (active ? "font-semibold text-emerald-300" : "font-medium text-ds-text")
          }>
            {name}
          </p>
          {hint ? <p className="text-[11px] text-ds-text-muted mt-0.5">{hint}</p> : null}
        </div>
        <span
          className={
            "shrink-0 inline-flex items-center justify-center size-5 rounded-full transition-colors " +
            (active
              ? "bg-emerald-500 text-white"
              : "border border-ds-border")
          }
          aria-hidden
        >
          {active ? <CheckIcon /> : null}
        </span>
      </button>
    </li>
  );
}

function CheckIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" className="size-3">
      <polyline points="20 6 9 17 4 12" />
    </svg>
  );
}

function IconSpeaker({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" />
      <path d="M19.07 4.93a10 10 0 0 1 0 14.14M15.54 8.46a5 5 0 0 1 0 7.07" />
    </svg>
  );
}

function IconMic({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className={className}>
      <path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z" />
      <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
      <line x1="12" y1="19" x2="12" y2="23" />
      <line x1="8" y1="23" x2="16" y2="23" />
    </svg>
  );
}
