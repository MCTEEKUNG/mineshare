/**
 * Hotkeys tab — read-only listing for now; the live editor lands
 * in a follow-up that adds per-platform global-hotkey
 * registration via Tauri's tauri-plugin-global-shortcut. The
 * existing Ctrl+Alt+R toggle is hardcoded into the input modules
 * (windows.rs / linux.rs scancode 0x13).
 */
export default function HotkeysPage() {
  return (
    <section>
      <p className="text-sm text-ds-text-muted mb-6 max-w-prose leading-relaxed">
        Built-in hotkeys are hardcoded into the input layer for now —
        the editor that lets you rebind them lands with M5 Slice 5.
      </p>

      <div className="rounded-xl border border-ds-border bg-ds-surface overflow-hidden divide-y divide-ds-border">
        <Row
          combo={["Ctrl", "Alt", "R"]}
          name="Toggle Remote"
          desc="Three-way: enter Remote / exit Remote on local / ask peer to release if peer holds Remote."
        />
        <Row
          combo={["Ctrl", "Alt", "K"]}
          name="Cycle keyboard target"
          desc="Smart → Pinned to peer → Pinned to local → Auto → Smart. Smart (default) auto-routes keys to whichever machine's mouse is currently in use AND follows cursor crossings — handles the 'two mice, one keyboard' workflow without manual pinning. Auto is the strict cursor-only mode. Pinned-* override everything regardless of mouse activity."
        />
        <Row
          combo={["Ctrl", "Alt", "L"]}
          name="Game-mode lock"
          desc="Pins input to this PC. Edge crossing and auto-handover pause; Ctrl+Alt+R still works as a manual escape hatch. On Windows, the bridge also auto-engages this when a fullscreen app captures or hides the cursor."
        />
      </div>
    </section>
  );
}

function Row({
  combo,
  name,
  desc,
}: {
  combo: string[];
  name: string;
  desc: string;
}) {
  return (
    <div className="flex items-center justify-between gap-6 px-5 py-4">
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-ds-text">{name}</p>
        <p className="text-xs text-ds-text-muted max-w-md mt-0.5 leading-relaxed">{desc}</p>
      </div>
      <div className="flex items-center gap-1.5 shrink-0">
        {combo.map((k, i) => (
          <kbd
            key={i}
            className="inline-flex items-center justify-center min-w-[28px] h-7 px-2 rounded-lg border border-ds-border bg-ds-hover text-[11px] font-mono text-ds-text"
          >
            {k}
          </kbd>
        ))}
      </div>
    </div>
  );
}
