import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useT } from "../i18n";

type DeviceInfo = { name: string; is_default: boolean };
type DevicesSnapshot = {
  outputs: DeviceInfo[]; inputs: DeviceInfo[];
  selected_output: string | null; selected_input: string | null; selected_sysout_capture: string | null;
};
type Direction = "output" | "input" | "sysout_capture";

export default function DevicesPage() {
  const { t } = useT();
  const [devices, setDevices] = useState<DevicesSnapshot | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let active = true;
    const tick = () => invoke<DevicesSnapshot>("list_audio_devices").then(value => { if (active) setDevices(value); }).catch(e => { if (active) setError(String(e)); });
    tick();
    const timer = setInterval(tick, 5000);
    return () => { active = false; clearInterval(timer); };
  }, []);
  async function pick(direction: Direction, name: string | null) {
    setBusy(true); setError("");
    const command = direction === "output" ? "set_audio_output_device" : direction === "input" ? "set_audio_input_device" : "set_sysout_capture_device";
    try {
      await invoke(command, { name });
      setDevices(current => current && ({ ...current, [`selected_${direction}`]: name }));
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  return <section className="panel p-5">
    <h2 className="text-base font-semibold">{t("devices_title")}</h2>
    <p className="muted mt-2 mb-5">{t("devices_hint")}</p>
    {devices ? <fieldset disabled={busy} className="space-y-5">{([
      { direction: "sysout_capture", label: "devices_capture", list: devices.outputs },
      { direction: "output", label: "devices_output", list: devices.outputs },
      { direction: "input", label: "devices_input", list: devices.inputs },
    ] as const).map(({ direction, label, list }) => {
      const selected = devices[`selected_${direction}`];
      return <label className="block" key={direction}>
        <span className="block mb-2 text-sm">{t(label)}</span>
        <select className="w-full min-w-0 rounded-xl bg-ds-elevated border border-ds-border px-3 py-2.5 text-sm" value={selected === null ? "default" : `device:${selected}`} onChange={e => pick(direction, e.target.value === "default" ? null : e.target.value.slice(7))}>
          <option value="default">{t("devices_default")}</option>
          {selected !== null && !list.some(device => device.name === selected) && <option value={`device:${selected}`}>{selected}</option>}
          {list.map(device => <option key={device.name} value={`device:${device.name}`}>{device.name}{device.is_default ? " ✓" : ""}</option>)}
        </select>
        {list.length === 0 && <span className="block mt-2 text-xs text-ds-text-muted">{t("devices_empty")}</span>}
      </label>;
    })}</fieldset> : !error && <p className="muted">{t("devices_loading")}</p>}
    {error && <p role="alert" className="text-sm text-ds-danger mt-3">{error}</p>}
  </section>;
}
