import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type DeviceInfo = { name: string; is_default: boolean };
type DevicesSnapshot = { outputs: DeviceInfo[]; inputs: DeviceInfo[] };

/**
 * Devices tab — read-only enumeration of cpal audio devices on
 * this machine. M5 Slice 4 surfaces *what* the bridge would
 * render peer audio into / capture mic from; switching the
 * active device away from the OS default lands in a follow-up
 * since cpal streams need to be torn down + rebuilt to retarget.
 */
export default function DevicesPage() {
  const [devs, setDevs] = useState<DevicesSnapshot | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    invoke<DevicesSnapshot>("list_audio_devices")
      .then(setDevs)
      .catch((e) => setErr(String(e)));
  }, []);

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
        title="Audio outputs"
        subtitle="Peer sysout (and peer mic on Win, when VB-CABLE is installed) renders into the system default."
        devices={devs.outputs}
      />
      <DeviceList
        title="Audio inputs"
        subtitle="Mic capture uses the system default input. PipeWire monitors aren't visible to cpal directly — the daemon spawns parec for those."
        devices={devs.inputs}
      />
    </section>
  );
}

function DeviceList({
  title,
  subtitle,
  devices,
}: {
  title: string;
  subtitle: string;
  devices: DeviceInfo[];
}) {
  return (
    <div className="rounded-lg border border-neutral-200 dark:border-neutral-800 p-5">
      <p className="text-base font-semibold mb-1">{title}</p>
      <p className="text-xs text-neutral-500 mb-4">{subtitle}</p>
      {devices.length === 0 ? (
        <p className="text-sm text-neutral-400">none reported by cpal</p>
      ) : (
        <ul className="space-y-1.5">
          {devices.map((d, i) => (
            <li
              key={i}
              className="flex items-center justify-between rounded-md border border-neutral-200 dark:border-neutral-800 px-3 py-2"
            >
              <span className="font-mono text-xs">{d.name}</span>
              {d.is_default ? (
                <span className="inline-flex items-center gap-1 text-[11px] text-emerald-600 px-2 py-0.5 rounded-full bg-emerald-50 dark:bg-emerald-950/40">
                  default
                </span>
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
