import { useT } from "../i18n";

export function ShortcutRows({ compact = false }: { compact?: boolean }) {
  const { t } = useT();
  const rows = [
    { keys: ["Ctrl", "Alt", "R"], name: "shortcut_remote", hint: "shortcut_remote_hint" },
    { keys: ["Ctrl", "Shift", "L"], name: "shortcut_lock", hint: "shortcut_lock_hint" },
    ...(!compact ? [{ keys: ["Ctrl", "Alt", "K"], name: "shortcut_keyboard", hint: "shortcut_keyboard_hint" }, { keys: ["Ctrl", "Alt", "G"], name: "gd_title", hint: "shortcut_game_hint" }] : []),
  ];
  return <div>{rows.map(row => <div key={row.name} className="shortcut-row">
    <div className="min-w-0"><p className="text-sm font-medium">{t(row.name)}</p><p className="text-xs text-ds-text-muted mt-1 leading-relaxed">{t(row.hint)}</p></div>
    <span className="key-combo" aria-label={row.keys.join(" + ")}>{row.keys.map(key => <kbd key={key}>{key}</kbd>)}</span>
  </div>)}</div>;
}

export default function HotkeysPage() { return <ShortcutRows />; }
