/**
 * Theme system — light / dark / system with persistence.
 *
 * `resolveTheme` is the one piece of pure logic worth a unit test
 * (see theme.test.ts): it maps a user choice + the OS preference
 * to the concrete theme actually applied. Everything else is the
 * React/DOM plumbing around it.
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
} from "react";
import { useT } from "./i18n";

export type ThemeChoice = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

export function resolveTheme(choice: ThemeChoice, prefersDark: boolean): ResolvedTheme {
  if (choice === "system") return prefersDark ? "dark" : "light";
  return choice;
}

const STORAGE_KEY = "mineshare.theme";
const ThemeCtx = createContext<{ choice: ThemeChoice; setChoice: (c: ThemeChoice) => void } | null>(null);

export function parseThemeChoice(value: string | null): ThemeChoice {
  return value === "light" || value === "dark" ? value : "system";
}

function readChoice(): ThemeChoice {
  try { return parseThemeChoice(localStorage.getItem(STORAGE_KEY)); }
  catch { return "system"; }
}

export function applyTheme(choice: ThemeChoice) {
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  document.documentElement.dataset.theme = resolveTheme(choice, prefersDark);
}

/** Pre-paint the persisted theme before React renders to avoid a flash. */
export function initThemeFromStorage() {
  applyTheme(readChoice());
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [choice, setChoiceState] = useState<ThemeChoice>(readChoice);
  const setChoice = useCallback((c: ThemeChoice) => {
    setChoiceState(c);
    try { localStorage.setItem(STORAGE_KEY, c); } catch { /* Keep this session usable. */ }
    applyTheme(c);
  }, []);
  useEffect(() => {
    applyTheme(choice);
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => { if (choice === "system") applyTheme("system"); };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, [choice]);
  return <ThemeCtx.Provider value={{ choice, setChoice }}>{children}</ThemeCtx.Provider>;
}

export function useTheme() {
  const ctx = useContext(ThemeCtx);
  if (!ctx) throw new Error("useTheme outside ThemeProvider");
  return ctx;
}

export function ThemeSwitcher() {
  const { t } = useT();
  const { choice, setChoice } = useTheme();
  const opts: ThemeChoice[] = ["system", "light", "dark"];
  return (
    <div className="inline-flex rounded-lg border border-ds-border overflow-hidden">
      {opts.map((o) => (
        <button key={o} onClick={() => setChoice(o)} aria-pressed={choice === o}
          className={"px-3 py-1.5 text-xs capitalize " + (choice === o ? "bg-ds-hover text-ds-text" : "text-ds-text-muted hover:text-ds-text")}>
          {t(`theme_${o}`)}
        </button>
      ))}
    </div>
  );
}
