# UI — Refined Evolution (Modernize + User-Friendly)

_Date: 2026-06-14 · Area: `ui/` (Tauri + React)_

## Problem

The current UI is a competent dark-sidebar dashboard (emerald accent, Tailwind,
custom SVG icons, i18n), but it has friction points that make it feel like a
developer tool rather than a polished product:

1. **Flat navigation.** Seven sibling tabs (`Status, Layout, Devices, Audio,
   Files, Hotkeys, Advanced`) with no grouping — related settings (Devices +
   Audio) are split, and rarely-touched config (Hotkeys, Advanced) sits at the
   same level as daily-use views.
2. **No real "home."** The default `Status` tab is a stack of cards; the single
   most important fact — *am I connected, to whom, how fast* — isn't visually
   dominant.
3. **Dark-only, hardcoded.** Colors are baked in everywhere (`bg-ds-bg`,
   `text-slate-100`, `white/[0.06]` overlays). No theme choice.
4. **`App.tsx` is ~1000 lines** mixing the shell, the Home/Status view, toasts,
   pills, latency histogram, and all icon components in one file.

## Goal

Keep everything that works about the dark dashboard. Make it feel modern and
friendly through: consolidated navigation, a connection-first Home, a proper
theming system (light / dark / system), and a polish + componentization pass.
This is **Direction A — refined evolution**, explicitly the lowest-risk path:
no framework change, no visual identity reinvention.

## Scope decisions (confirmed with user)

- **Navigation consolidates to 5 items:** `Home · Layout · Audio & Devices ·
  Files · Settings`.
  - `Audio & Devices` = today's `Audio` + `Devices` merged (both are audio
    routing / device selection).
  - `Settings` hosts `Hotkeys` and `Advanced` as sub-sections.
- **Theming:** add **light + dark + system-follow**. Treated as a real refactor
  (its own phase), not polish.
- **Home** replaces `Status` as the landing view with a connection hero.

## Design

### Navigation

`App.tsx`'s `Tab` union and `<nav>` change from 7 entries to 5:

```
type Tab = "home" | "layout" | "audio_devices" | "files" | "settings";
```

- Sidebar stays (220px, logo, language toggle, version footer).
- `Audio & Devices` renders a single page that composes the existing
  `AudioPage` and `DevicesPage` bodies under section headers (no logic rewrite —
  compose, don't merge their internals).
- `Settings` renders a page with two sections (`Hotkeys`, `Advanced`), again
  composing the existing page bodies. The **mouse-rate slider** (see the mouse
  spec) lives here under a new "Performance" section.
- Active-tab indicator, badges (Files transfer count) preserved.

### Home (connection-first)

A new `pages/Home.tsx` (extracted from the current inline `status` branch in
`App.tsx`), reordered for hierarchy:

1. **Connection hero** (top, full width): the two machines + the cursor-bridge
   state, peer name, and the **latency headline** (e.g. `4 ms · paired with
   Ubuntu`) promoted from the small histogram caption to a large readout. When
   no peer: a clear "Waiting for a peer on the LAN…" state.
2. **Mode pills** (Mouse/cursor + Keyboard) — promoted directly under the hero,
   since "where does my input go" is the #1 confusion vector (preserve existing
   `ModePill` / `KeyboardPill` logic verbatim — only their placement changes).
3. **Game-lock card** + **anti-cheat banner** (unchanged behavior).
4. **Stats grid** + **latency histogram** move below the fold.

No data-flow changes: the same `get_status` / `get_latency` polls feed Home.

### Theming (light / dark / system)

The blocker is that color is hardcoded. Approach:

- Introduce **semantic CSS variables** (e.g. `--bg`, `--surface`, `--border`,
  `--text`, `--text-muted`, `--accent`) defined for `[data-theme="dark"]` and
  `[data-theme="light"]` on `:root`, plus a `system` mode that reads
  `prefers-color-scheme` and sets `data-theme` accordingly.
- Map them through Tailwind: replace `bg-ds-*` / `text-slate-*` /
  `white/[0.0x]` usages with token-backed utilities (extend
  `tailwind.config.ts` `colors` to reference the CSS vars). The emerald accent
  becomes `--accent` so it stays consistent and themeable.
- Persist the choice (Tauri store / existing config plumbing) and apply on
  startup before first paint to avoid a flash.
- A theme switcher (Light / Dark / System) lives in `Settings`.

This touches **every component** that currently hardcodes a color. It is the
largest single chunk of this spec and is sequenced last so the structural wins
ship first.

### Polish + componentization

- Split `App.tsx`: move icon components to `ui/src/icons.tsx`; move
  Home view + its sub-components (`ModePill`, `KeyboardPill`, `StatusGrid`,
  `LatencyCard`, `Histogram`) into `pages/Home.tsx`; keep `App.tsx` as the
  shell (sidebar, routing, global drag-drop, toasts, badges).
- Establish a consistent spacing scale and card style; ensure visible
  focus-rings on all interactive elements (accessibility + keyboard nav).
- i18n: add keys for new nav labels and any new strings; keep EN + existing
  locale in sync.

## Phasing

1. **Nav consolidation + Home hero** — restructure tabs, extract `Home.tsx`,
   compose `Audio & Devices` and `Settings`. No theming yet. Ships the biggest
   UX win at low risk.
2. **Theming refactor** — CSS variables + token sweep + switcher + persistence.
3. **Polish + componentization** — `App.tsx` split, spacing/focus pass, i18n
   cleanup.

## Non-goals

- No new framework, router library, or component kit.
- No visual-identity reinvention (we picked refined evolution, not Bold).
- No backend/daemon changes (the mouse-rate slider's plumbing is owned by the
  mouse spec; this spec only places the control in `Settings`).

## Risks

- **Theming sweep is broad** — every hardcoded color must be migrated; missing
  one shows as an unreadable element in the off-theme. Mitigate by grepping for
  `bg-ds-`, `text-slate-`, `white/[`, `black/[` and converting systematically,
  and by reviewing both themes per page.
- **Composing pages vs. merging** — `Audio & Devices` and `Settings` must reuse
  existing page bodies to avoid regressing working logic; resist rewriting.

## Verification

- Manual: each of the 5 nav targets renders; Home hero shows correct
  connected/disconnected/driving/driven states; theme switch flips light/dark
  and persists across restart; system mode follows OS preference.
- `tsc -b` clean; existing build (`ui` Tauri build) succeeds.
