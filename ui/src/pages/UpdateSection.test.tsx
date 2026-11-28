// @vitest-environment jsdom
import {afterEach, beforeEach, describe, expect, it, vi} from "vitest";
import {cleanup, fireEvent, render, screen, waitFor} from "@testing-library/react";
import UpdateSection from "./UpdateSection";
const {invoke} = vi.hoisted(() => ({invoke: vi.fn()}));
vi.mock("@tauri-apps/api/core", () => ({invoke}));
const dict: Record<string, string> = {update_title: "Updates", update_current: "Current", update_check: "Check", update_checking: "Checking",
  update_latest: "Latest", update_available: "Version {v} ready", update_install: "Install", update_installing: "Installing",
  update_manual: "Page", update_failed: "Failed: "};
vi.mock("../i18n", () => ({useT: () => ({t: (k: string) => dict[k] ?? k})}));
beforeEach(() => invoke.mockReset());
afterEach(cleanup);
const info = {current: "0.0.1", latest: "0.0.2", available: true, installable: true, page: "https://x"};
describe("in-app update", () => {
  it("checks on click and offers install only when a newer, installable version exists", async () => {
    invoke.mockResolvedValueOnce(info);
    render(<UpdateSection/>);
    expect(screen.queryByText("Install")).toBeNull();
    fireEvent.click(screen.getByText("Check"));
    await screen.findByText("Version 0.0.2 ready");
    invoke.mockResolvedValueOnce(undefined);
    fireEvent.click(screen.getByText("Install"));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("install_update"));
  });
  it("says so when already up to date and shows failures", async () => {
    invoke.mockResolvedValueOnce({...info, available: false});
    render(<UpdateSection/>);
    fireEvent.click(screen.getByText("Check"));
    await screen.findByText("Latest");
    invoke.mockRejectedValueOnce("offline");
    fireEvent.click(screen.getByText("Check"));
    await screen.findByRole("alert");
    expect(screen.getByRole("alert").textContent).toContain("offline");
  });
});
