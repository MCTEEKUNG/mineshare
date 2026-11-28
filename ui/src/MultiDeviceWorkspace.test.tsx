// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { ConnectionProvider } from "./ConnectionControls";
import ConnectedDevices from "./components/ConnectedDevices";
import WorkspaceLayout from "./components/WorkspaceLayout";
import { LanguageProvider } from "./i18n";
import type { PeerSession } from "./connectionState";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
let peers: PeerSession[];

beforeEach(() => {
  invoke.mockReset(); localStorage.clear();
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  peers = [
    { device_id: "laptop", display_name: "Laptop", os: "windows", input_active: true, send_audio: false, receive_audio: false, display_width: 1920, display_height: 1080, peer_side: "right" },
    { device_id: "phone", display_name: "Galaxy S24 Ultra", os: "android", input_active: false, send_audio: false, receive_audio: true, display_width: 1440, display_height: 3120, peer_side: "left" },
  ];
  invoke.mockImplementation(async (command: string, args: Record<string, unknown>) => {
    if (command === "get_connection_control") return { paused: false, pairing_enabled: false, connected_peer_ids: peers.map(peer => peer.device_id) };
    if (command === "list_connected_peers") return peers;
    if (command === "get_layout") return { peer_side: "right" };
    if (command === "select_input_peer") peers = peers.map(peer => ({ ...peer, input_active: peer.device_id === args.deviceId }));
    if (command === "set_peer_audio_route") peers = peers.map(peer => peer.device_id === args.deviceId ? { ...peer, send_audio: args.sendSystem as boolean, receive_audio: args.receiveSystem as boolean } : peer);
    if (command === "set_peer_layout") peers = peers.map(peer => peer.device_id === args.deviceId ? { ...peer, peer_side: args.side as PeerSession["peer_side"] } : peer);
    return null;
  });
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });

it("moves input to Android while keeping each device's sound route", async () => {
  render(<LanguageProvider><ConnectionProvider><ConnectedDevices /></ConnectionProvider></LanguageProvider>);
  const phone = await screen.findByRole("article", { name: "Galaxy S24 Ultra" });
  const laptop = screen.getByRole("article", { name: "Laptop" });
  fireEvent.click(within(phone).getByRole("button", { name: "Use mouse & keyboard" }));
  await waitFor(() => expect(within(phone).getByRole("button", { name: "Mouse & keyboard destination" })).toBeTruthy());
  expect(within(phone).getByRole<HTMLInputElement>("checkbox", { name: "Listen to this device" }).checked).toBe(true);
  expect(within(laptop).getByRole<HTMLInputElement>("checkbox", { name: "Listen to this device" }).checked).toBe(false);
  fireEvent.click(within(laptop).getByRole("checkbox", { name: "Send this computer’s sound" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_peer_audio_route", { deviceId: "laptop", sendSystem: true, receiveSystem: false }));
  expect(peers.find(peer => peer.device_id === "phone")?.receive_audio).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Keep mouse & keyboard here" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("select_input_peer", { deviceId: null }));
  expect(peers).toHaveLength(2);
});

it("renders all connected displays and saves only the chosen phone's position", async () => {
  render(<LanguageProvider><ConnectionProvider><WorkspaceLayout /></ConnectionProvider></LanguageProvider>);
  const phone = await screen.findByRole("button", { name: "Arrange screen: Galaxy S24 Ultra" });
  const laptop = screen.getByRole("button", { name: "Arrange screen: Laptop" });
  expect(Number.parseFloat(phone.style.aspectRatio)).toBeCloseTo(1440 / 3120);
  fireEvent.keyDown(phone, { key: "ArrowDown" });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_peer_layout", { deviceId: "phone", side: "bottom" }));
  await waitFor(() => expect(phone.getAttribute("data-peer-side")).toBe("bottom"));
  expect(laptop.getAttribute("data-peer-side")).toBe("right");
});

it("explains the selected phone's actual crossing edge and updates after moving it", async () => {
  peers = peers.map(peer => ({ ...peer, input_active: peer.device_id === "phone" }));
  render(<LanguageProvider><ConnectionProvider><WorkspaceLayout /></ConnectionProvider></LanguageProvider>);
  const phone = await screen.findByRole("button", { name: "Arrange screen: Galaxy S24 Ultra" });
  expect(screen.getByText("Move your mouse across the left edge of this computer’s screen to control Galaxy S24 Ultra. Your keyboard follows the mouse.")).toBeTruthy();
  fireEvent.keyDown(phone, { key: "ArrowDown" });
  await screen.findByText("Move your mouse across the bottom edge of this computer’s screen to control Galaxy S24 Ultra. Your keyboard follows the mouse.");
});

it("keeps one device per edge by trading places with the device already there", async () => {
  peers = peers.map(peer => ({ ...peer, peer_side: "left" as const }));
  peers[0] = { ...peers[0], peer_side: "bottom" };
  render(<LanguageProvider><ConnectionProvider><WorkspaceLayout /></ConnectionProvider></LanguageProvider>);
  const phone = await screen.findByRole("button", { name: "Arrange screen: Galaxy S24 Ultra" });
  fireEvent.keyDown(phone, { key: "ArrowDown" });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_peer_layout", { deviceId: "phone", side: "bottom" }));
  expect(invoke).toHaveBeenCalledWith("set_peer_layout", { deviceId: "laptop", side: "left" });
  await waitFor(() => expect(screen.getByRole("button", { name: "Arrange screen: Laptop" }).getAttribute("data-peer-side")).toBe("left"));
});
