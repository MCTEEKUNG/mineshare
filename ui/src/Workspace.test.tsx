// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import App from "./App";
import { ConnectionProvider } from "./ConnectionControls";
import { LanguageProvider } from "./i18n";
import LayoutPage from "./pages/Layout";
import DevicesPage from "./pages/Devices";
import type { Status } from "./types";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ onDragDropEvent: async () => () => {} }) }));
vi.mock("./theme", () => ({ ThemeSwitcher: () => <span>Theme</span> }));

const status: Status = {
  peer_connected: true, peer_addr: "192.168.1.108", peer_name: "Tee",
  local_in_remote: false, peer_in_remote: false, input_locked: false, game_drive: "off", anticheat_warning: null,
  keyboard_target: "smart", sent_pkts: 1, recv_pkts: 1, injected: 1, audio_recv: 1, inject_errs: 0, decrypt_errs: 0,
  keys_forwarded: 1, keys_injected: 1, local_version: "0.0.16 · same", peer_version: "0.0.16 · same",
};
beforeEach(() => {
  invoke.mockReset(); localStorage.clear();
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it("identifies the selected offline installation and does not replace it with a nearby namesake", async () => {
  const selected = { device_id: "fda74f50-9760-4636-859a-ff5f7240086c", display_name: "Tee", os: null };
  let requested = false;
  invoke.mockImplementation(async (command: string) => {
    if (command === "get_status") return { ...status, peer_connected: false, peer_name: "Stale connection" };
    if (command === "get_connection_control") return { paused: false, pairing_enabled: requested, selected_peer: selected };
    if (command === "get_latency") return { p50_ms: 1.2 };
    if (command === "get_transfers") return [];
    if (command === "get_pairing_phase") return { kind: "none" };
    if (command === "get_linux_setup_status") return { linux: false };
    if (command === "get_layout") return { peer_side: "right" };
    if (command === "list_nearby_peers") return [{ device_id: "6289ef5a-windows", display_name: "Tee", os: "windows", addresses: ["192.168.1.111"], trusted: true, connected: false, request_supported: true }];
    if (command === "send_pairing_request") requested = true;
    return null;
  });
  render(<LanguageProvider><ConnectionProvider><App /></ConnectionProvider></LanguageProvider>);
  await screen.findByText("Waiting for: Tee · fda74f50");
  expect(document.querySelector("header [role='status']")?.textContent).toBe("Disconnected");
  expect(await screen.findByRole("button", { name: "Arrange screen: Tee" })).toBeTruthy();
  expect(screen.queryByText("Stale connection")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Choose a computer" }));
  const picker = await screen.findByRole("dialog");
  await within(picker).findByText("windows · 192.168.1.111");
  expect(within(picker).getByText("Selected computer: Tee · fda74f50")).toBeTruthy();
  expect(within(picker).queryByText("Selected")).toBeNull();
  expect(invoke.mock.calls.some(([command]) => command === "send_pairing_request")).toBe(false);
  fireEvent.click(within(picker).getByRole("button", { name: /^Tee\s*windows/ }));
  expect(invoke.mock.calls.some(([command]) => command === "send_pairing_request")).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "Send request" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("send_pairing_request", { deviceId: "6289ef5a-windows" }));
  // The saved destination is not relabelled until the other computer accepts.
  await screen.findByText("Selected computer: Tee · fda74f50");
});

it("keeps pause state visible across navigation and mounts advanced settings only when opened", async () => {
  let paused = false;
  invoke.mockImplementation(async (command: string, args?: { paused: boolean }) => {
    if (command === "get_status") return { ...status, peer_connected: !paused };
    if (command === "set_connections_paused") paused = args!.paused;
    if (command === "get_connection_control") return { paused, pairing_enabled: false };
    if (command === "get_latency") return { p50_ms: 1.2, last_ms: 2 };
    if (command === "get_transfers") return [];
    if (command === "get_pairing_phase") return { kind: "none" };
    if (command === "get_linux_setup_status") return { linux: false };
    if (command === "get_layout") return { peer_side: "right" };
    if (command === "get_startup_status") return { enabled: true, managed: false };
    if (command === "get_settings") return { mouse_sensitivity: 1, touchpad_scroll_speed: 1, mouse_rate_hz: 250, invert_scroll_y: false, invert_scroll_x: false, auto_focus_on_take_control: false };
    return null;
  });
  render(<LanguageProvider><ConnectionProvider><App /></ConnectionProvider></LanguageProvider>);
  const pause = await screen.findByRole("button", { name: "Disconnect & pause" });
  await waitFor(() => expect((pause as HTMLButtonElement).disabled).toBe(false));
  expect(document.querySelector("header [role='status']")?.textContent).toBe("Connected");
  expect(screen.getByRole("region", { name: "Latency" }).querySelector("p")?.textContent).toBe("1.2ms");
  fireEvent.click(pause);
  await screen.findByRole("button", { name: "Resume connections" });
  expect(document.querySelector("header [role='status']")?.textContent).toBe("Disconnected");
  expect(screen.getByRole("region", { name: "Latency" }).querySelector("p")?.textContent).toBe("—");
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  await screen.findByRole("heading", { name: "Settings" });
  expect(screen.getAllByText("Connections paused").length).toBeGreaterThan(0);
  expect(invoke.mock.calls.some(([command]) => command === "get_settings")).toBe(false);
  fireEvent.click(screen.getByText("Mouse & keyboard", { selector: "summary" }));
  await screen.findByLabelText("Mouse update rate");
});

it("retains the current screen side when saving fails", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "get_layout") return { peer_side: "right" };
    if (command === "set_layout") throw new Error("Cannot save layout");
  });
  render(<LanguageProvider><LayoutPage /></LanguageProvider>);
  const peer = await screen.findByRole("button", { name: "Arrange screen: Paired computer" });
  fireEvent.keyDown(peer, { key: "ArrowLeft" });
  expect((await screen.findByRole("alert")).textContent).toContain("Cannot save layout");
  expect(peer.getAttribute("data-peer-side")).toBe("right");
  expect(invoke).toHaveBeenCalledWith("set_layout", { cfg: { peer_side: "left" } });
});

it("keeps device overrides explicit and can return to the system default", async () => {
  invoke.mockResolvedValue({ outputs: [{ name: "Headphones", is_default: true }], inputs: [{ name: "Mic", is_default: true }], selected_output: "Headphones", selected_input: null, selected_sysout_capture: null });
  render(<LanguageProvider><DevicesPage /></LanguageProvider>);
  const output = await screen.findByRole("combobox", { name: "Listen through" });
  fireEvent.change(output, { target: { value: "default" } });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_audio_output_device", { name: null }));
  expect(invoke.mock.calls.some(([command]) => command === "refresh_audio_devices")).toBe(false);
});


it("marks only locked monitors with the user's color and clears a stale peer lock offline", async () => {
  invoke.mockImplementation(async (command: string) => command === "get_layout" ? { peer_side: "right" } : null);
  const effect = { rainbow: false, color: [255, 30, 100] as [number, number, number], animation: "pulse" as const, duration_ms: 900, thickness: 8 };
  const { rerender, container } = render(<LanguageProvider><LayoutPage connected status={{ ...status, input_locked: true, remote_input_locked: false, lock_effect: effect }} /></LanguageProvider>);
  await screen.findByRole("button", { name: "Arrange screen: Paired computer" });
  const local = () => container.querySelector<HTMLElement>(".display-local")!;
  const peer = () => container.querySelector<HTMLElement>(".display-peer")!;
  expect(local().getAttribute("data-locked")).toBe("true");
  expect(local().style.getPropertyValue("--lock-color")).toBe("rgb(255, 30, 100)");
  expect(peer().getAttribute("data-locked")).toBe("false");
  rerender(<LanguageProvider><LayoutPage connected status={{ ...status, input_locked: false, remote_input_locked: true, lock_effect: effect }} /></LanguageProvider>);
  expect(local().getAttribute("data-locked")).toBe("false");
  expect(peer().getAttribute("data-locked")).toBe("true");
  const peerVisual = { locked: true, effect: { ...effect, color: [40, 210, 190] as [number, number, number] } };
  rerender(<LanguageProvider><LayoutPage connected status={{ ...status, remote_input_locked: false, peer_lock_visual: peerVisual, lock_effect: effect }} /></LanguageProvider>);
  expect(peer().getAttribute("data-locked")).toBe("true");
  expect(peer().style.getPropertyValue("--lock-color")).toBe("rgb(40, 210, 190)");
  rerender(<LanguageProvider><LayoutPage connected={false} status={{ ...status, remote_input_locked: true, lock_effect: effect }} /></LanguageProvider>);
  expect(peer().getAttribute("data-locked")).toBe("false");
});
