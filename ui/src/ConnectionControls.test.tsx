// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import ConnectionControls, { ConnectionProvider } from "./ConnectionControls";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./i18n", () => ({ useT: () => ({ t: (key: string) => key }) }));
beforeEach(() => invoke.mockReset());
afterEach(cleanup);

it("pauses connections and offers resume while new pairing remains disabled", async () => {
  let paused = false;
  invoke.mockImplementation(async (command: string, args?: { paused: boolean }) => {
    if (command === "set_connections_paused") paused = args!.paused;
    return { paused, pairing_enabled: false };
  });
  render(<ConnectionProvider><ConnectionControls /></ConnectionProvider>);
  const pause = await screen.findByRole("button", { name: "connections_pause" });
  await waitFor(() => expect((pause as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(pause);
  const resume = await screen.findByRole("button", { name: "connections_resume" });
  expect(invoke).toHaveBeenCalledWith("set_connections_paused", { paused: true });
  expect((screen.getByRole("button", { name: "pair_start" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(resume);
  await screen.findByRole("button", { name: "connections_pause" });
  expect(invoke).toHaveBeenCalledWith("set_connections_paused", { paused: false });
});

it("discovery never sends requests until a specific computer is chosen and confirmed", async () => {
  let enabled = false;
  invoke.mockImplementation(async (command: string, args?: { enabled?: boolean; deviceId?: string }) => {
    if (command === "list_nearby_peers") return ["A", "B"].map(name => ({ device_id: name, display_name: name, os: "Linux", addresses: ["192.168.1.3"], trusted: false, connected: false, request_supported: true }));
    if (command === "send_pairing_request") enabled = true;
    if (command === "set_pairing_enabled") enabled = args!.enabled!;
    return { paused: false, pairing_enabled: enabled };
  });
  render(<ConnectionProvider><ConnectionControls /></ConnectionProvider>);
  const start = await screen.findByRole("button", { name: "pair_start" });
  await waitFor(() => expect((start as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(start);
  const device = await screen.findByRole("button", { name: /^B/ });
  expect(invoke.mock.calls.some(([command]) => command === "send_pairing_request")).toBe(false);
  expect(invoke).not.toHaveBeenCalledWith("set_pairing_enabled", { enabled: true });
  fireEvent.click(device);
  expect(invoke.mock.calls.some(([command]) => command === "send_pairing_request")).toBe(false);
  fireEvent.click(screen.getByRole("button", { name: "nearby_send" }));
  const stop = await screen.findByRole("button", { name: "pair_stop" });
  expect(invoke).toHaveBeenCalledWith("send_pairing_request", { deviceId: "B" });
  expect(invoke.mock.calls.filter(([command]) => command === "send_pairing_request")).toHaveLength(1);
  fireEvent.click(stop);
  await screen.findByRole("button", { name: "pair_start" });
  expect(invoke).toHaveBeenCalledWith("set_pairing_enabled", { enabled: false });
});

it("clears a transient status error after the service becomes available", async () => {
  invoke.mockRejectedValueOnce(new Error("temporarily unavailable"))
    .mockResolvedValue({ paused: false, pairing_enabled: false });
  render(<ConnectionProvider><ConnectionControls /></ConnectionProvider>);
  expect((await screen.findByRole("alert")).textContent).toContain("temporarily unavailable");
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull(), { timeout: 2000 });
  expect((screen.getByRole("button", { name: "connections_pause" }) as HTMLButtonElement).disabled).toBe(false);
});
