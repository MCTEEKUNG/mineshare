// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import PairingModal from "./PairingModal";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./i18n", () => ({ useT: () => ({ t: (key: string) => key }) }));
beforeEach(() => invoke.mockReset());
afterEach(cleanup);

it.each(["awaitingpin", "displayingpin", "verifying", "waitingapproval"])("can stop %s without waiting for timeout", async (kind) => {
  invoke.mockResolvedValue({ kind, attempt_id: 42, peer_name: "Nearby PC", peer_addr: "192.168.1.3", pin: "123456" });
  render(<PairingModal />);
  const cancel = await screen.findByRole("button", { name: "pair_cancel" });
  fireEvent.click(cancel);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("set_pairing_enabled", { enabled: false }));
});

it.each([true, false])("requires an explicit answer to an incoming request (accept=%s)", async accept => {
  invoke.mockResolvedValue({ kind: "incomingrequest", attempt_id: 81, peer_name: "Chosen PC", peer_addr: "192.168.1.8" });
  render(<PairingModal />);
  const answer = await screen.findByRole("button", { name: accept ? "pair_accept" : "pair_reject" });
  expect(invoke.mock.calls.some(([command]) => command === "respond_pairing_request")).toBe(false);
  fireEvent.click(answer);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("respond_pairing_request", { accept, attemptId: 81 }));
  expect(screen.queryByRole("textbox")).toBeNull();
});
