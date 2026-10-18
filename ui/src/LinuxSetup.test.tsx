// @vitest-environment jsdom
import {afterEach, beforeEach, describe, expect, it, vi} from "vitest";
import {cleanup, fireEvent, render, screen, waitFor} from "@testing-library/react";
import LinuxSetup from "./LinuxSetup";
import {needsSetup, recommended, type SetupStatus} from "./linuxSetupModel";
const {invoke} = vi.hoisted(() => ({invoke: vi.fn()}));
vi.mock("@tauri-apps/api/core", () => ({invoke}));
vi.mock("./i18n", () => ({useT: () => ({locale: "en"})}));
const fresh: SetupStatus = {linux: true, session: "wayland", gnome_supported: true, wayland: true,
  input_active: false, input_configured: false, uinput_available: true, typeless: true, broker_configured: false, broker_active: false,
  extension_configured: false, extension_active: false, extensions_allowed: true, helpers_available: true, pending_login: false};
beforeEach(() => { invoke.mockReset(); sessionStorage.clear(); invoke.mockResolvedValue(fresh); });
afterEach(cleanup);
describe("Ubuntu first-run setup", () => {
  it("detects without making changes, then configures only after explicit click", async () => {
    render(<LinuxSetup/>);
    await screen.findByRole("dialog");
    expect(invoke.mock.calls.every(([name]) => name === "get_linux_setup_status")).toBe(true);
    const pending = {...fresh, input_configured: true, broker_configured: true, extension_configured: true, pending_login: true};
    invoke.mockResolvedValueOnce(pending);
    fireEvent.click(screen.getByRole("button", {name: "Set up selected"}));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("configure_linux_setup", {options: {input: true, broker: true, pointer: true}}));
    await screen.findByRole("status");
    expect(screen.getByRole("status").textContent).toContain("sign out");
    expect(screen.getAllByText(/Configured · sign out/).length).toBe(3);
    expect((screen.getByRole("button", {name: "Set up selected"}) as HTMLButtonElement).disabled).toBe(true);
  });
  it("does not install broker without Typeless or extensions on X11", () => {
    expect(recommended({...fresh, typeless: false, wayland: false})).toEqual({input: true, broker: false, pointer: false});
    expect(needsSetup({...fresh, linux: false})).toBe(false);
  });
  it("allows Later without changing anything and can reopen from Settings", async () => {
    render(<LinuxSetup/>);
    await screen.findByRole("dialog");
    fireEvent.click(screen.getByRole("button", {name: "Later / Close"}));
    expect(screen.queryByRole("dialog")).toBeNull();
    fireEvent(window, new Event("mineshare-linux-setup"));
    await screen.findByRole("dialog");
    expect(invoke.mock.calls.some(([name]) => name === "configure_linux_setup")).toBe(false);
  });
  it("shows cancellation errors and rechecks partially applied state", async () => {
    render(<LinuxSetup/>); await screen.findByRole("dialog");
    invoke.mockRejectedValueOnce(new Error("Authentication cancelled")).mockResolvedValueOnce(fresh);
    fireEvent.click(screen.getByRole("button", {name: "Set up selected"}));
    expect((await screen.findByRole("alert")).textContent).toContain("Authentication cancelled");
    await waitFor(() => expect((screen.getByRole("button", {name: "Set up selected"}) as HTMLButtonElement).disabled).toBe(false));
  });
  it("does not auto-open on Windows or when Linux is already active", async () => {
    invoke.mockResolvedValue({...fresh, input_active: true, broker_active: true, extension_active: true});
    render(<LinuxSetup/>); await waitFor(() => expect(invoke).toHaveBeenCalled());
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
