# UI Refined Evolution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Modernize the MineShare Tauri/React UI by consolidating navigation to 5 items, introducing a connection-first Home, adding a light/dark/system theming system, and splitting the ~1000-line `App.tsx` — without changing the daemon or the visual identity.

**Architecture:** Three phases. (1) Restructure navigation and extract a `Home` page from the inline Status branch; compose `Audio & Devices` and `Settings` from existing page bodies. (2) Replace hardcoded colors with CSS-variable-backed semantic tokens and add a theme switcher with persistence. (3) Split `App.tsx`, add a focus/spacing polish pass, sync i18n. Spec: `docs/superpowers/specs/2026-06-14-ui-refined-evolution-design.md`.

**Tech Stack:** React 18, TypeScript, Vite, Tailwind CSS (`darkMode: "class"`, `ds.*` color tokens), Tauri 2 (`@tauri-apps/api`), homegrown i18n (`ui/src/i18n.tsx`). No test runner exists yet; this plan adds `vitest` only for the one piece of pure logic (theme resolution). All other verification is `tsc -b`, `vite build`, and explicit manual checks.

---

## File Structure

- `ui/src/App.tsx` — **shrinks** to the shell: sidebar, 5-tab routing, global drag-drop, toasts, transfer badge. Home view + icons move out.
- `ui/src/icons.tsx` — **new**: all SVG icon components moved from `App.tsx`.
- `ui/src/pages/Home.tsx` — **new**: connection hero + mode pills + game-lock + stats + latency (moved from `App.tsx`'s `status` branch and its helper components).
- `ui/src/pages/AudioDevices.tsx` — **new**: composes existing `AudioPage` + `DevicesPage` bodies under section headers.
- `ui/src/pages/Settings.tsx` — **new**: composes existing `HotkeysPage` + `AdvancedPage` under section headers; hosts the future "Performance" section (mouse-rate slider lands here via the mouse plan).
- `ui/src/theme.tsx` — **new**: theme context, `resolveTheme()` pure helper, persistence, `<ThemeProvider>` + `useTheme()` + a `ThemeSwitcher` component.
- `ui/src/theme.test.ts` — **new**: unit tests for `resolveTheme()`.
- `ui/src/index.css` — **modify**: define semantic CSS variables for `[data-theme="dark"]` / `[data-theme="light"]`.
- `ui/tailwind.config.ts` — **modify**: point `ds.*` (and new semantic) colors at the CSS variables.
- `ui/src/i18n.tsx` — **modify**: new nav keys (`nav_home`, `nav_audio_devices`, `nav_settings`), theme labels; keep `en` + `th` in sync.
- `ui/vitest.config.ts`, `ui/package.json` — **modify/new**: add `vitest` dev dependency + `test` script.

---

## Phase 1 — Navigation + Home

### Task 1: Extract icons into their own module

**Files:**
- Create: `ui/src/icons.tsx`
- Modify: `ui/src/App.tsx` (remove the `IconX` function definitions at the bottom, add an import)

- [ ] **Step 1: Create `ui/src/icons.tsx`** containing every icon component currently defined at the bottom of `App.tsx` (`IconActivity, IconGrid, IconHeadphones, IconVolume, IconFile, IconKeyboard, IconSliders, IconArrowDown, IconArrowUp, IconArrowUpRight, IconArrowDownLeft, IconX, IconAlertTriangle`). Move them verbatim, each prefixed with `export`. Add at the top:

```tsx
/* SVG icon components — moved out of App.tsx to keep the shell focused. */
```

- [ ] **Step 2: In `App.tsx`, delete** the moved icon function definitions and add near the other imports:

```tsx
import {
  IconActivity, IconGrid, IconHeadphones, IconVolume, IconFile,
  IconKeyboard, IconSliders, IconArrowDown, IconArrowUp, IconArrowUpRight,
  IconArrowDownLeft, IconX, IconAlertTriangle,
} from "./icons";
```

- [ ] **Step 3: Verify typecheck + build**

Run: `cd ui && npx tsc -b`
Expected: no errors (icons resolve from the new module).

- [ ] **Step 4: Commit**

```bash
git add ui/src/icons.tsx ui/src/App.tsx
git commit -m "refactor(ui): extract SVG icons into icons.tsx"
```

### Task 2: Extract the Home page from the Status branch

**Files:**
- Create: `ui/src/pages/Home.tsx`
- Modify: `ui/src/App.tsx`

- [ ] **Step 1: Create `ui/src/pages/Home.tsx`.** Move these components out of `App.tsx` into it, verbatim, each `export`ed or used internally: `VersionFooter` stays in App; move `AntiCheatBanner`, `GameLockCard`, `StatusGrid`, `ModePill`, `KeyboardPill`, `LatencyCard`, `LatencyStat`, `Histogram`, `labelFor`, `Stat`. Define and export a `HomePage` component that takes the data the branch used:

```tsx
import { invoke } from "@tauri-apps/api/core";
import type { Status, Latency } from "../types";

export default function HomePage({ status, latency }: { status: Status; latency: Latency | null }) {
  return (
    <>
      {status.anticheat_warning ? <AntiCheatBanner game={status.anticheat_warning} /> : null}
      <ConnectionHero status={status} latency={latency} />
      <ModePill s={status} />
      <GameLockCard s={status} onChange={(v) => invoke("set_input_lock", { locked: v })} />
      <StatusGrid s={status} />
      {status.peer_connected ? <LatencyCard latency={latency} /> : null}
    </>
  );
}
```

- [ ] **Step 2: Create `ui/src/types.ts`** and move the `Status`, `Latency`, `Transfer`, `ToastEntry` type definitions out of `App.tsx` into it (each `export`ed). Import them where needed in both `App.tsx` and `Home.tsx`.

- [ ] **Step 3: Add the `ConnectionHero` component** to `Home.tsx` — a full-width card that leads with peer + latency:

```tsx
function ConnectionHero({ status, latency }: { status: Status; latency: Latency | null }) {
  const connected = status.peer_connected;
  const headlineMs = latency?.p50_ms ?? latency?.last_ms ?? null;
  return (
    <div className="rounded-2xl border border-white/[0.08] bg-ds-surface p-6 mb-6">
      <div className="flex items-center justify-between gap-6">
        <div className="flex items-center gap-4">
          <MachineBadge label="This PC" active />
          <span className="text-emerald-400 font-mono text-lg">━━●━━▶</span>
          <MachineBadge label={status.peer_name ?? "Peer"} active={connected} />
        </div>
        <div className="text-right">
          <p className="text-[10px] uppercase tracking-widest text-slate-500">Latency</p>
          <p className="text-2xl font-semibold text-white">
            {connected && headlineMs != null ? `${headlineMs < 10 ? headlineMs.toFixed(1) : headlineMs.toFixed(0)} ms` : "—"}
          </p>
          <p className="text-xs text-slate-400">{connected ? `paired with ${status.peer_addr ?? "peer"}` : "waiting for a peer on the LAN…"}</p>
        </div>
      </div>
    </div>
  );
}

function MachineBadge({ label, active }: { label: string; active: boolean }) {
  return (
    <div className={"rounded-xl border px-4 py-3 text-sm font-medium " + (active ? "border-emerald-500/30 bg-emerald-500/10 text-emerald-200" : "border-white/[0.08] bg-white/[0.03] text-slate-400")}>
      {label}
    </div>
  );
}
```

- [ ] **Step 4: In `App.tsx`, replace** the inline `tab === "status"` JSX block with `{tab === "home" && status ? <HomePage status={status} latency={latency} /> : null}` and import `HomePage`. (The `Tab` type still has `"status"` at this point — Task 3 renames it. To keep this step compiling, temporarily render Home under the existing `"status"` value, i.e. `tab === "status"`.)

- [ ] **Step 5: Verify**

Run: `cd ui && npx tsc -b`
Expected: no errors. `App.tsx` line count drops substantially.

- [ ] **Step 6: Commit**

```bash
git add ui/src/pages/Home.tsx ui/src/types.ts ui/src/App.tsx
git commit -m "refactor(ui): extract Home page + connection hero from App.tsx"
```

### Task 3: Consolidate navigation to 5 items

**Files:**
- Modify: `ui/src/App.tsx`, `ui/src/i18n.tsx`
- Create: `ui/src/pages/AudioDevices.tsx`, `ui/src/pages/Settings.tsx`

- [ ] **Step 1: Add i18n keys.** In `ui/src/i18n.tsx`, add to both `en` and `th` dictionaries (translate `th` to match existing tone):

```tsx
// en
nav_home: "Home",
nav_audio_devices: "Audio & Devices",
nav_settings: "Settings",
section_audio: "Audio",
section_devices: "Devices",
section_hotkeys: "Hotkeys",
section_advanced: "Advanced",
section_performance: "Performance",
```

- [ ] **Step 2: Create `ui/src/pages/AudioDevices.tsx`** composing the existing page bodies:

```tsx
import AudioPage from "./Audio";
import DevicesPage from "./Devices";
import { useT } from "../i18n";

export default function AudioDevicesPage() {
  const { t } = useT();
  return (
    <div className="flex flex-col gap-10">
      <section>
        <h3 className="text-sm font-semibold text-slate-300 mb-3">{t("section_audio")}</h3>
        <AudioPage />
      </section>
      <section>
        <h3 className="text-sm font-semibold text-slate-300 mb-3">{t("section_devices")}</h3>
        <DevicesPage />
      </section>
    </div>
  );
}
```

- [ ] **Step 3: Create `ui/src/pages/Settings.tsx`** the same way with `HotkeysPage` + `AdvancedPage` under `section_hotkeys` / `section_advanced`. Add a placeholder `Performance` section comment where the mouse-rate slider will mount:

```tsx
{/* Performance section (mouse-rate slider) added by the mouse-rate plan. */}
```

- [ ] **Step 4: Update the `Tab` union and nav** in `App.tsx`:

```tsx
type Tab = "home" | "layout" | "audio_devices" | "files" | "settings";
```

Set `useState<Tab>("home")`. Replace the seven `<NavItem>`s with five: Home (`IconActivity`), Layout (`IconGrid`), Audio & Devices (`IconVolume`), Files (`IconFile`, keep badge), Settings (`IconSliders`). Update the main-content switch:

```tsx
{tab === "home" && status ? <HomePage status={status} latency={latency} /> : null}
{tab === "layout" ? <LayoutPage /> : null}
{tab === "audio_devices" ? <AudioDevicesPage /> : null}
{tab === "files" ? <FilesPage /> : null}
{tab === "settings" ? <SettingsPage /> : null}
```

Update the drag-drop handler's `setTab("files")` (unchanged) and the latency poll guard `if (tab !== "status")` → `if (tab !== "home")`. Update `tabTitle` to `t(\`nav_${tab}\`)` (now resolves `nav_home`, `nav_audio_devices`, `nav_settings`).

- [ ] **Step 5: Verify typecheck + build + manual**

Run: `cd ui && npx tsc -b && npx vite build`
Expected: builds clean.
Manual (`npm run tauri dev` or existing run flow): all 5 tabs render; Home is default and shows the hero; Audio & Devices shows both sections; Settings shows Hotkeys + Advanced; Files badge still appears during a transfer.

- [ ] **Step 6: Commit**

```bash
git add ui/src/App.tsx ui/src/i18n.tsx ui/src/pages/AudioDevices.tsx ui/src/pages/Settings.tsx
git commit -m "feat(ui): consolidate navigation to 5 items (Home/Layout/Audio & Devices/Files/Settings)"
```

---

## Phase 2 — Theming (light / dark / system)

### Task 4: Add vitest and the theme-resolution unit (TDD)

**Files:**
- Modify: `ui/package.json`
- Create: `ui/vitest.config.ts`, `ui/src/theme.test.ts`, `ui/src/theme.tsx`

- [ ] **Step 1: Add vitest.** In `ui/package.json` devDependencies add `"vitest": "^2.1.0"` and a script `"test": "vitest run"`. Run `cd ui && npm install`.

- [ ] **Step 2: Create `ui/vitest.config.ts`**:

```ts
import { defineConfig } from "vitest/config";
export default defineConfig({ test: { environment: "node" } });
```

- [ ] **Step 3: Write the failing test** `ui/src/theme.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { resolveTheme } from "./theme";

describe("resolveTheme", () => {
  it("returns the explicit choice for light/dark", () => {
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
  });
  it("follows the OS for 'system'", () => {
    expect(resolveTheme("system", true)).toBe("dark");   // prefersDark = true
    expect(resolveTheme("system", false)).toBe("light");
  });
});
```

- [ ] **Step 4: Run it, expect FAIL**

Run: `cd ui && npx vitest run theme.test.ts`
Expected: FAIL — `resolveTheme` is not defined / `theme.tsx` missing.

- [ ] **Step 5: Create `ui/src/theme.tsx` with the minimal pure helper** (provider/components added in Task 5):

```tsx
export type ThemeChoice = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

export function resolveTheme(choice: ThemeChoice, prefersDark: boolean): ResolvedTheme {
  if (choice === "system") return prefersDark ? "dark" : "light";
  return choice;
}
```

- [ ] **Step 6: Run it, expect PASS**

Run: `cd ui && npx vitest run theme.test.ts`
Expected: PASS (4 assertions).

- [ ] **Step 7: Commit**

```bash
git add ui/package.json ui/package-lock.json ui/vitest.config.ts ui/src/theme.test.ts ui/src/theme.tsx
git commit -m "test(ui): add vitest + resolveTheme pure helper"
```

### Task 5: Theme provider, persistence, and switcher

**Files:**
- Modify: `ui/src/theme.tsx`, `ui/src/main.tsx`, `ui/src/i18n.tsx`

- [ ] **Step 1: Extend `theme.tsx`** with a context, provider, hook, and switcher:

```tsx
import { createContext, useContext, useEffect, useState, useCallback } from "react";

const STORAGE_KEY = "mineshare.theme";
const ThemeCtx = createContext<{ choice: ThemeChoice; setChoice: (c: ThemeChoice) => void } | null>(null);

function readChoice(): ThemeChoice {
  const v = localStorage.getItem(STORAGE_KEY);
  return v === "light" || v === "dark" || v === "system" ? v : "system";
}

export function applyTheme(choice: ThemeChoice) {
  const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  document.documentElement.dataset.theme = resolveTheme(choice, prefersDark);
}

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [choice, setChoiceState] = useState<ThemeChoice>(readChoice);
  const setChoice = useCallback((c: ThemeChoice) => {
    setChoiceState(c);
    localStorage.setItem(STORAGE_KEY, c);
    applyTheme(c);
  }, []);
  useEffect(() => {
    applyTheme(choice);
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => { if (readChoice() === "system") applyTheme("system"); };
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
  const { choice, setChoice } = useTheme();
  const opts: ThemeChoice[] = ["light", "dark", "system"];
  return (
    <div className="inline-flex rounded-lg border border-white/[0.08] overflow-hidden">
      {opts.map((o) => (
        <button key={o} onClick={() => setChoice(o)}
          className={"px-3 py-1.5 text-xs capitalize " + (choice === o ? "bg-white/[0.10] text-white" : "text-slate-400 hover:text-slate-200")}>
          {o}
        </button>
      ))}
    </div>
  );
}
```

- [ ] **Step 2: Wrap the app and pre-paint the theme** in `ui/src/main.tsx`. Before `ReactDOM.createRoot(...).render(...)`, call `applyTheme(readChoice())` is internal — instead export an `initThemeBeforeRender()` or simply call `applyTheme` via a tiny inline read. Add an exported helper in `theme.tsx`:

```tsx
export function initThemeFromStorage() { applyTheme(readChoice()); }
```

Then in `main.tsx`: import `{ ThemeProvider, initThemeFromStorage }`, call `initThemeFromStorage()` at module top (before render) to avoid a flash, and wrap `<App />` in `<ThemeProvider>`.

- [ ] **Step 3: Mount the switcher in Settings.** In `ui/src/pages/Settings.tsx` add a top section:

```tsx
import { ThemeSwitcher } from "../theme";
// ...inside the returned JSX, first section:
<section>
  <h3 className="text-sm font-semibold text-slate-300 mb-3">{t("section_appearance")}</h3>
  <ThemeSwitcher />
</section>
```

Add i18n key `section_appearance: "Appearance"` (en) + th equivalent.

- [ ] **Step 4: Verify**

Run: `cd ui && npx tsc -b && npx vitest run`
Expected: typecheck clean, tests pass.
Manual: switching Light/Dark/System in Settings flips `document.documentElement.dataset.theme`; reload preserves choice; System follows OS appearance change. (Colors don't visually change yet — Task 6 wires tokens.)

- [ ] **Step 5: Commit**

```bash
git add ui/src/theme.tsx ui/src/main.tsx ui/src/pages/Settings.tsx ui/src/i18n.tsx
git commit -m "feat(ui): theme provider, persistence, and Light/Dark/System switcher"
```

### Task 6: Migrate colors to CSS-variable tokens

**Files:**
- Modify: `ui/src/index.css`, `ui/tailwind.config.ts`, then component files.

- [ ] **Step 1: Define semantic CSS variables** in `ui/src/index.css`:

```css
:root, [data-theme="dark"] {
  --ds-bg: #0F172A;
  --ds-surface: #1B2336;
  --ds-elevated: #1E293B;
  --ds-sidebar: #0D1525;
  --ds-accent: #22C55E;
  --ds-text: #F1F5F9;        /* was text-slate-100 */
  --ds-text-muted: #94A3B8;  /* slate-400 */
  --ds-border: rgba(255,255,255,0.08);
  --ds-hover: rgba(255,255,255,0.06);
}
[data-theme="light"] {
  --ds-bg: #F4F5F7;
  --ds-surface: #FFFFFF;
  --ds-elevated: #FFFFFF;
  --ds-sidebar: #EAECEF;
  --ds-accent: #16A34A;
  --ds-text: #1F2937;
  --ds-text-muted: #6B7280;
  --ds-border: rgba(0,0,0,0.10);
  --ds-hover: rgba(0,0,0,0.05);
}
```

- [ ] **Step 2: Point Tailwind tokens at the variables** in `ui/tailwind.config.ts`:

```ts
colors: {
  ds: {
    bg: "var(--ds-bg)",
    surface: "var(--ds-surface)",
    elevated: "var(--ds-elevated)",
    sidebar: "var(--ds-sidebar)",
    accent: "var(--ds-accent)",
    text: "var(--ds-text)",
    "text-muted": "var(--ds-text-muted)",
    border: "var(--ds-border)",
    hover: "var(--ds-hover)",
  },
},
```

- [ ] **Step 3: Sweep hardcoded neutrals.** Across `ui/src/App.tsx`, `ui/src/pages/*.tsx`, `ui/src/PairingModal.tsx`, `ui/src/icons.tsx` (no colors there), replace, in this order, only the structural neutrals (NOT the semantic status colors emerald/amber/red/blue, which stay): `text-slate-100` → `text-ds-text`; `text-slate-400`/`text-slate-500` → `text-ds-text-muted`; `border-white/[0.06]` and `border-white/[0.08]` → `border-ds-border`; `bg-white/[0.04]`/`hover:bg-white/[0.08]` overlays → `bg-ds-hover`/`hover:bg-ds-hover`. The top-level `bg-ds-bg`/`bg-ds-surface`/`bg-ds-sidebar` already resolve via tokens — no change needed.

Use a grep to find every occurrence first:

Run: `grep -rn "text-slate-100\|text-slate-400\|text-slate-500\|white/\[0.0" ui/src`

Convert each hit. Leave `text-slate-200`/`text-slate-300` as a judgment call — map both to `text-ds-text` for light-mode legibility.

- [ ] **Step 4: Verify both themes**

Run: `cd ui && npx tsc -b && npx vite build`
Manual: toggle Light and Dark; walk all 5 tabs + the PairingModal; confirm no unreadable (e.g. white-on-white) elements. Fix any missed neutral by converting it to the matching `ds-*` token.

- [ ] **Step 5: Commit**

```bash
git add ui/src/index.css ui/tailwind.config.ts ui/src/App.tsx ui/src/pages ui/src/PairingModal.tsx
git commit -m "feat(ui): CSS-variable semantic tokens enabling light + dark themes"
```

---

## Phase 3 — Polish + componentization

### Task 7: Spacing, focus states, and final cleanup

**Files:**
- Modify: `ui/src/App.tsx`, `ui/src/pages/*.tsx`, `ui/src/index.css`

- [ ] **Step 1: Add visible focus rings.** In `ui/src/index.css` add:

```css
:where(button, [role="button"], a, input, select):focus-visible {
  outline: 2px solid var(--ds-accent);
  outline-offset: 2px;
}
```

- [ ] **Step 2: Confirm `App.tsx` is now the shell only** — sidebar, routing, drag-drop, toasts, badge. If any Home-specific helper remains, move it to `Home.tsx`. Target: `App.tsx` well under 400 lines.

- [ ] **Step 3: i18n sync check.** Ensure every key used in `en` exists in `th` and vice-versa. Add a dev-only guard at the bottom of `i18n.tsx`:

```tsx
if (import.meta.env.DEV) {
  const a = Object.keys(en), b = Object.keys(th);
  const missing = [...a.filter(k => !b.includes(k)), ...b.filter(k => !a.includes(k))];
  if (missing.length) console.warn("i18n key mismatch:", missing);
}
```

- [ ] **Step 4: Verify**

Run: `cd ui && npx tsc -b && npx vite build && npx vitest run`
Manual: tab through the UI with the keyboard — focus rings visible; no console i18n warning.

- [ ] **Step 5: Commit**

```bash
git add ui/src
git commit -m "polish(ui): focus rings, App.tsx slimmed to shell, i18n parity guard"
```

---

## Self-Review

**Spec coverage:**
- Nav → 5 items: Task 3. ✓
- Connection-first Home: Task 2 (hero) + reordered pills. ✓
- Light/dark/system theming: Tasks 4–6. ✓
- App.tsx split / componentization: Tasks 1, 2, 7. ✓
- Polish (spacing/focus), i18n: Task 7. ✓
- Mouse-rate slider placement in Settings → Performance: section stub in Task 3 Step 3 + `section_performance` key (Task 3 Step 1); the control itself is owned by the mouse-rate plan. ✓

**Placeholder scan:** The only intentional stub is the Performance-section comment in `Settings.tsx`, which is explicitly owned by the mouse plan — not a gap in this plan.

**Type consistency:** `ThemeChoice`/`ResolvedTheme`/`resolveTheme` consistent across Tasks 4–5. `Tab` union updated once (Task 3) after Home renders temporarily under `"status"` (Task 2 Step 4) — sequencing noted so each task compiles. `HomePage` props `{ status, latency }` consistent between Task 2 Step 1 and the App switch in Task 3 Step 4.

**Notes for the implementer:** No test runner existed before Task 4; do not expect pre-existing UI tests. Verification for visual tasks is `tsc -b` + `vite build` + the listed manual checks.
