# Workspace UI refresh

The supplied Typeless screenshot is the visual reference: inset charcoal sidebar, large left-aligned heading, soft gray panels, subtle dotted workspace, with MineShare's original antique gold identity restored after user feedback. Its marketing cards and update notification are not MineShare features.

Palette: original warm dark surfaces (`#15130f`, `#191611`, `#211d17`, `#29231b`) and gold accent `#d4af37`; ivory surfaces with darker gold `#8a651f` in light mode. Primary buttons and paired screens use theme tokens; navigation and feature icons use gold. The original gold logo is displayed without grayscale/brightness filters. The refreshed panel layout, display dragging, and controls remain unchanged.

## Everyday controls

| Area | Result |
| --- | --- |
| Home | Dedicated display arrangement panel, separate shortcuts/control panel underneath, clipboard/audio/file entry points |
| Connection | Disconnect/pause, resume, choose a LAN computer and send a targeted pairing request; the receiver accepts or declines, and paused state is shared across navigation |
| Screen position | Drag screen 2 against any of the four edges of screen 1 on Home or Settings, with drop previews and automatic saving; the existing runtime mirrors the peer edge |
| Audio devices | Compact selectors with system-default option; automatic device enumeration replaces repeated refresh buttons |
| Manual input/game controls | Available on demand in Settings, including recovery from a pinned keyboard target |
| Telemetry/build identifiers | Detailed diagnostics are collapsed; a build mismatch warning remains visible in the sidebar |

Optional settings mount when expanded, so closed panels do not keep polling. Clipboard sync remains automatic. Reconnection targets the selected trusted computer; discovery alone never pairs new computers. Audio sharing controls remain explicit. See [connection-controls.md](connection-controls.md) for the request, approval and recovery behavior.

The display editor uses numbered monitor rectangles in the style of Windows 11. The local computer is the anchor. A valid drop snaps the paired display against an edge; dropping over the local display, dropping outside the targets, pointer cancellation, or Escape keeps the existing layout. Arrow keys provide an alternative to dragging. Failed saves restore the prior position, and status polling cannot overwrite a pending edit with an older response.

## Verification

- Windows: `npm run build`, `npm test` — 30 tests passed.
- Linux: the prepared `mineshare-linux-builder:0.0.16` Docker image ran `npm ci`, tests, and frontend production build from a read-only source mount; 30 tests passed. The temporary container is removed afterwards.
- Both environments produced identical frontend asset names.
- Browser checks use a Tauri IPC fixture, not live hardware: pause/resume, targeted pairing selection/confirmation/approval/cancellation, audio device selection, microphone toggle, screen position, keyboard recovery, navigation, Thai/English, dark/light, build mismatch, long peer names, and the 880×560 minimum window.
- Display editor browser checks additionally cover mouse dragging in all four directions, highlighted targets, invalid drops, Escape cancellation, keyboard positioning, failed-save rollback, navigation persistence, and monitor bounds at 880×560. The upper panel contains no keyboard commands.
- Screenshots are under `artifacts/ux-rework/`; `home-screen-layout.png` shows the production frontend with separate screen and shortcut panels. The newer request-pairing flow is under `artifacts/request-pairing/`.

This refresh and the request-pairing flow were installed on Windows and Tee on 2026-10-21 from source `75c6aa848d2c`. The real session reports matching `control-v3` builds; deployment evidence is in `artifacts/request-pairing/release/verification.md`. The user subsequently confirmed successful reconnection after physically interrupting the network on 2026-10-21.

The subsequent Ubuntu setup correction is deployed on both computers as source `1747886a3dcf`. See [ubuntu-setup-upgrade.md](ubuntu-setup-upgrade.md) for the repro, fix and native UI verification.

The subsequent palette and latency information hierarchy were installed on both
computers as source `b2839fa75580`, `control-v3`, on 2026-10-21. The root
[Design.md](../Design.md) is now the canonical design guideline. The header
shows connection status only; latency is the prominent metric, with no stale
value while paused/disconnected. IP is available under diagnostics. Native UI,
package identity, connection, input readiness and preserved configuration are
recorded in `artifacts/latency-design/release/verification.md`.
