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
 *
 * Stage 8.4 made these runtime-switchable; this revision (post
 * Stage 10 polish request) cleans up the visual hierarchy:
 *
 *   - One row per physical device, no mono font (cpal names are
 *     readable English, no need for Courier).
 *   - Speaker / microphone emoji on the section header so the
 *     two lists are unambiguously different at a glance.
 *   - A single "active" check icon replaces the badge stack —
 *     OS default is a small inline "(default)" suffix in the
 *     name itself, not a competing badge.
 *   - A "Follow system default" row sits at the top of each list
 *     and makes it obvious how to revert when the user has
 *     overridden the choice.
 *   - Manual "Refresh" button so newly hot-plugged devices can
 *     be picked up without waiting for the next poll tick.
 */
export default function DevicesPage() {
  const [devs, setDevs] = useState<DevicesSnapshot | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [pending, setPending] = useState<Direction | null>(null);

  async function refresh(force = false) {
    try {
      // The Rust side caches device enumeration for 5 s — pinging
      // it every couple seconds was contributing to "Not Responding"
      // pauses on slower Win laptops because each cpal enumeration
      // takes 100–500 ms of COM time. We now poll lazily (10 s) and
      // let the user trigger an explicit invalidation via the
      // refresh button when they hot-plug a device.
      if (force) {
        await invoke("refresh_audio_devices");
      }
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
      const cmd =
        dir === "output" ? "set_audio_output_device" : "set_audio_input_device";
      await invoke(cmd, { name });
      // Optimistic update — confirmed by the next poll.
      setDevs((s) =>
        s
          ? {
              ...s,
              [dir === "output" ? "selected_output" : "selected_input"]: name,
            }
          : s,
      );
    } catch (e) {
      setErr(String(e));
    } finally {
      setPending(null);
    }
  }

  if (!devs) {
    return (
      <p className="text-sm text-neutral-400">
        {err ? `failed: ${err}` : "loading devices…"}
      </p>
    );
  }

  return (
    <section className="grid gap-6">
      <DeviceList
        icon="🔊"
        title="Audio output"
        subtitle="Where peer sysout (and peer mic on Win, when VB-CABLE is installed) renders."
        devices={devs.outputs}
        selected={devs.selected_output}
        busy={pending === "output"}
        onPick={(n) => pick("output", n)}
        onRefresh={() => refresh(true)}
      />
      <DeviceList
        icon="🎙️"
        title="Audio input"
        subtitle="Where the bridge captures your mic. Pick a non-default device for headset / USB mic / OBS virtual cam, etc."
        devices={devs.inputs}
        selected={devs.selected_input}
        busy={pending === "input"}
        onPick={(n) => pick("input", n)}
        onRefresh={() => refresh(true)}
      />
      {err ? <p className="text-xs text-red-600">{err}</p> : null}
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
  icon: string;
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
    <div className="rounded-lg border border-neutral-200 dark:border-neutral-800 overflow-hidden">
      {/* Header: icon + title + status pill on the right --------- */}
      <div className="flex items-center justify-between px-5 pt-4 pb-3 border-b border-neutral-200 dark:border-neutral-800">
        <div className="flex items-center gap-3 min-w-0">
          <span className="text-2xl leading-none">{icon}</span>
          <div className="min-w-0">
            <p className="text-base font-semibold leading-tight">{title}</p>
            <p className="text-xs text-neutral-500 mt-0.5 max-w-md truncate">
              {subtitle}
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          {busy ? (
            <span className="text-[11px] text-neutral-400">switching…</span>
          ) : null}
          <button
            onClick={onRefresh}
            className="text-[11px] text-neutral-500 hover:text-neutral-900 dark:hover:text-neutral-100 px-2 py-1 rounded-md hover:bg-neutral-50 dark:hover:bg-neutral-900 transition-colors"
            title="Re-scan devices"
          >
            ↻ refresh
          </button>
        </div>
      </div>

      {/* Body: list of devices ----------------------------------- */}
      <ul className="divide-y divide-neutral-100 dark:divide-neutral-900">
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
          <li className="px-5 py-8 text-center text-sm text-neutral-400">
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
            ? "bg-emerald-50/70 dark:bg-emerald-950/30"
            : "hover:bg-neutral-50 dark:hover:bg-neutral-900/60")
        }
      >
        <div className="min-w-0 flex-1">
          <p
            className={
              "text-sm truncate " +
              (active
                ? "font-semibold text-emerald-700 dark:text-emerald-300"
                : "font-medium text-neutral-800 dark:text-neutral-200")
            }
          >
            {name}
          </p>
          {hint ? (
            <p className="text-[11px] text-neutral-500 mt-0.5">{hint}</p>
          ) : null}
        </div>
        <span
          className={
            "shrink-0 inline-flex items-center justify-center size-5 rounded-full transition-colors " +
            (active
              ? "bg-emerald-500 text-white"
              : "border border-neutral-300 dark:border-neutral-700")
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
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="3"
      strokeLinecap="round"
      strokeLinejoin="round"
      className="size-3"
    >
      <polyline points="20 6 9 17 4 12" />
    </svg>
  );
}
