import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type Status = {
  peer_connected: boolean;
  peer_addr: string | null;
  peer_name: string | null;
  sent_pkts: number;
  recv_pkts: number;
  injected: number;
  audio_recv: number;
  inject_errs: number;
  decrypt_errs: number;
};

type Settings = {
  mouse_sensitivity: number;
  invert_scroll_y: boolean;
  invert_scroll_x: boolean;
  auto_focus_on_take_control: boolean;
};

/**
 * Advanced tab — diagnostics + power-user info that doesn't
 * belong anywhere else: full traffic counters, error counts,
 * platform notes, and pointers to the on-disk config / log
 * locations the daemon writes to.
 */
export default function AdvancedPage() {
  const [s, setS] = useState<Status | null>(null);

  useEffect(() => {
    const tick = () => invoke<Status>("get_status").then(setS).catch(() => {});
    tick();
    const id = setInterval(tick, 1000);
    return () => clearInterval(id);
  }, []);

  return (
    <section className="grid gap-6">
      <InputPrefsCard />

      <div className="rounded-xl border border-ds-border bg-ds-surface p-5">
        <p className="text-base font-semibold text-ds-text mb-4">Traffic</p>
        {s ? (
          <dl className="grid grid-cols-2 md:grid-cols-3 gap-x-8 gap-y-3 text-sm">
            <Row k="Sent packets" v={s.sent_pkts.toLocaleString()} />
            <Row k="Received packets" v={s.recv_pkts.toLocaleString()} />
            <Row k="Audio frames recv" v={s.audio_recv.toLocaleString()} />
            <Row k="Injected events" v={s.injected.toLocaleString()} />
            <Row k="Inject errors" v={s.inject_errs.toLocaleString()} alert={s.inject_errs > 0} />
            <Row k="Decrypt errors" v={s.decrypt_errs.toLocaleString()} alert={s.decrypt_errs > 0} />
          </dl>
        ) : (
          <p className="text-sm text-ds-text-muted">connecting…</p>
        )}
      </div>

      <div className="rounded-xl border border-ds-border bg-ds-surface p-5">
        <p className="text-base font-semibold text-ds-text mb-4">Local files</p>
        <ul className="text-xs space-y-3 text-ds-text-muted">
          <FileEntry
            label="Daemon log"
            winPath="%APPDATA%\MineShare\logs\daemon.YYYY-MM-DD"
            linuxPath="~/.config/MineShare/logs/daemon.YYYY-MM-DD"
          />
          <FileEntry
            label="Layout config"
            winPath="%APPDATA%\MineShare\layout.json"
            linuxPath="~/.config/MineShare/layout.json"
          />
          <FileEntry
            label="Identity (Noise XX private key)"
            winPath="%APPDATA%\MineShare\identity.json"
            linuxPath="~/.config/MineShare/identity.json"
          />
        </ul>
      </div>

      <div className="rounded-xl border border-ds-border bg-ds-surface p-5">
        <p className="text-base font-semibold text-ds-text mb-4">Platform notes</p>
        <ul className="text-xs space-y-2 text-ds-text-muted list-disc pl-5 leading-relaxed">
          <li>
            Closing the window hides MineShare to the system tray; the
            daemon and bridge keep running. Use the tray icon's "Quit
            MineShare" entry to actually exit.
          </li>
          <li>
            Layout side auto-syncs across the encrypted control
            channel — drag the peer tile on either machine and the
            other side flips its mirror automatically.
          </li>
          <li>
            Linux clipboard sync uses arboard's Wayland data-control
            protocol when available, falling back to Xwayland; if you
            see "X11 server connection timed out" the launcher script
            didn't pick up XAUTHORITY.
          </li>
        </ul>
      </div>
    </section>
  );
}

/**
 * Stage 10 input-preference card.
 *
 * Mouse sensitivity is applied capture-side — the slider on
 * each machine controls how its outgoing mouse deltas are scaled
 * before being forwarded to the peer. Sub-pixel residue is retained
 * in the input crate so 0.5x doesn't drop alternating 1-pixel motions.
 */
function InputPrefsCard() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    invoke<Settings>("get_settings")
      .then(setSettings)
      .catch((e) => setErr(String(e)));
  }, []);

  async function update(next: Settings) {
    setErr(null);
    setSettings(next);
    try {
      const applied = await invoke<Settings>("set_settings", { settings: next });
      setSettings(applied);
    } catch (e) {
      setErr(String(e));
    }
  }

  if (!settings) {
    return (
      <div className="rounded-xl border border-ds-border bg-ds-surface p-5">
        <p className="text-base font-semibold text-ds-text mb-3">Input preferences</p>
        <p className="text-xs text-ds-text-muted">
          {err ? `failed to load: ${err}` : "loading…"}
        </p>
      </div>
    );
  }

  return (
    <div className="rounded-xl border border-ds-border bg-ds-surface p-5">
      <p className="text-base font-semibold text-ds-text mb-1">Input preferences</p>
      <p className="text-xs text-ds-text-muted mb-5 max-w-prose leading-relaxed">
        Per-machine knobs for mouse + scroll forwarding to the peer.
        Persists between launches; takes effect immediately.
      </p>

      <div className="mb-5">
        <div className="flex items-baseline justify-between mb-1.5">
          <label className="text-sm font-medium text-ds-text">Mouse sensitivity</label>
          <span className="text-xs font-mono text-ds-text-muted">
            {settings.mouse_sensitivity.toFixed(2)}×
          </span>
        </div>
        <input
          type="range"
          min={0.25}
          max={3}
          step={0.05}
          value={settings.mouse_sensitivity}
          onChange={(e) =>
            update({ ...settings, mouse_sensitivity: parseFloat(e.target.value) })
          }
          className="w-full accent-emerald-500"
        />
        <div className="flex justify-between text-[10px] text-ds-text-muted font-mono mt-0.5">
          <span>0.25×</span>
          <span>1.00×</span>
          <span>3.00×</span>
        </div>
        <p className="text-[11px] text-ds-text-muted mt-2 max-w-prose leading-relaxed">
          Multiplier applied to outgoing mouse deltas. Dial down if
          driving a low-DPI peer from a high-DPI laptop feels too
          fast; dial up for the opposite.
        </p>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-2 mb-3">
        <Toggle
          label="Invert vertical scroll"
          hint="flip the sign of forwarded wheel events"
          on={settings.invert_scroll_y}
          onChange={(v) => update({ ...settings, invert_scroll_y: v })}
        />
        <Toggle
          label="Invert horizontal scroll"
          hint="trackpad two-finger horizontal swipe"
          on={settings.invert_scroll_x}
          onChange={(v) => update({ ...settings, invert_scroll_x: v })}
        />
      </div>

      <Toggle
        label="Auto-click to grab keyboard focus"
        hint="When the peer drives this machine, fire a single click in place to focus the window under the cursor. Useful on GNOME-Wayland (click-to-focus). Rate-limited to once every 30 s so rapid cursor crossings don't spam clicks — the previous unlimited version triggered phantom 'spacebar' behaviour by clicking on play/pause buttons, links, and similar focusable elements every time the cursor crossed. Leave OFF unless typed keys keep vanishing on the peer."
        on={settings.auto_focus_on_take_control}
        onChange={(v) => update({ ...settings, auto_focus_on_take_control: v })}
      />

      {err ? <p className="text-xs text-red-400 mt-3">{err}</p> : null}
    </div>
  );
}

function Toggle({
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
      className="flex items-center justify-between rounded-lg border border-ds-border bg-ds-hover px-3 py-3 hover:bg-ds-hover transition-colors text-left"
    >
      <div className="min-w-0 mr-3">
        <p className="text-sm font-medium text-ds-text">{label}</p>
        <p className="text-[11px] text-ds-text-muted mt-0.5 leading-relaxed">{hint}</p>
      </div>
      <span
        className={
          "relative inline-block h-5 w-9 rounded-full transition-colors shrink-0 " +
          (on ? "bg-emerald-500" : "bg-ds-hover")
        }
      >
        <span
          className={
            "absolute top-0.5 size-4 rounded-full bg-white shadow transition-transform " +
            (on ? "translate-x-[18px]" : "translate-x-0.5")
          }
        />
      </span>
    </button>
  );
}

function Row({ k, v, alert }: { k: string; v: string; alert?: boolean }) {
  return (
    <div>
      <dt className="text-[10px] uppercase tracking-widest text-ds-text-muted mb-1">{k}</dt>
      <dd className={"text-sm font-medium " + (alert ? "text-red-400" : "text-ds-text")}>
        {v}
      </dd>
    </div>
  );
}

function FileEntry({
  label,
  winPath,
  linuxPath,
}: {
  label: string;
  winPath: string;
  linuxPath: string;
}) {
  return (
    <li>
      <p className="font-medium text-ds-text mb-0.5">{label}</p>
      <p>
        <span className="text-ds-text-muted">win: </span>
        <code className="font-mono text-ds-text-muted">{winPath}</code>
      </p>
      <p>
        <span className="text-ds-text-muted">linux: </span>
        <code className="font-mono text-ds-text-muted">{linuxPath}</code>
      </p>
    </li>
  );
}
