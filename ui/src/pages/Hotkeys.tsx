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
      <p className="text-sm text-neutral-500 mb-6 max-w-prose">
        Built-in hotkeys are hardcoded into the input layer for now —
        the editor that lets you rebind them lands with M5 Slice 5.
      </p>

      <div className="rounded-lg border border-neutral-200 dark:border-neutral-800 divide-y divide-neutral-200 dark:divide-neutral-800">
        <Row
          combo={["Ctrl", "Alt", "R"]}
          name="Toggle Remote"
          desc="Three-way: enter Remote / exit Remote on local / ask peer to release if peer holds Remote."
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
      <div>
        <p className="text-sm font-medium">{name}</p>
        <p className="text-xs text-neutral-500 max-w-md">{desc}</p>
      </div>
      <div className="flex items-center gap-1.5">
        {combo.map((k, i) => (
          <kbd
            key={i}
            className="inline-flex items-center justify-center min-w-[28px] h-7 px-2 rounded-md border border-neutral-300 dark:border-neutral-700 bg-neutral-50 dark:bg-neutral-900 text-[11px] font-mono"
          >
            {k}
          </kbd>
        ))}
      </div>
    </div>
  );
}
