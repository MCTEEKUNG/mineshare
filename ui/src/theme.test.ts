import { afterEach, describe, it, expect, vi } from "vitest";
import { initThemeFromStorage, parseThemeChoice, resolveTheme } from "./theme";

afterEach(() => vi.unstubAllGlobals());

describe("resolveTheme", () => {
  it("defaults new and invalid preferences to System without replacing explicit choices", () => {
    for (const value of [null, "", "invalid", "system"]) expect(parseThemeChoice(value)).toBe("system");
    expect(parseThemeChoice("light")).toBe("light");
    expect(parseThemeChoice("dark")).toBe("dark");
  });
  it("returns the explicit choice for light/dark", () => {
    expect(resolveTheme("light", true)).toBe("light");
    expect(resolveTheme("dark", false)).toBe("dark");
  });
  it("follows the OS for 'system'", () => {
    expect(resolveTheme("system", true)).toBe("dark");   // prefersDark = true
    expect(resolveTheme("system", false)).toBe("light");
  });
  it("uses the OS theme on first launch and preserves an explicit override", () => {
    const dataset: Record<string, string> = {};
    vi.stubGlobal("document", { documentElement: { dataset } });
    vi.stubGlobal("window", { matchMedia: () => ({ matches: true }) });
    vi.stubGlobal("localStorage", { getItem: () => null });
    initThemeFromStorage();
    expect(dataset.theme).toBe("dark");
    vi.stubGlobal("window", { matchMedia: () => ({ matches: false }) });
    initThemeFromStorage();
    expect(dataset.theme).toBe("light");
    vi.stubGlobal("localStorage", { getItem: () => "dark" });
    initThemeFromStorage();
    expect(dataset.theme).toBe("dark");
  });
  it("does not prevent app startup when preference storage is unavailable", () => {
    const dataset: Record<string, string> = {};
    vi.stubGlobal("document", { documentElement: { dataset } });
    vi.stubGlobal("window", { matchMedia: () => ({ matches: true }) });
    vi.stubGlobal("localStorage", { getItem: () => { throw new Error("storage disabled"); } });
    expect(() => initThemeFromStorage()).not.toThrow();
    expect(dataset.theme).toBe("dark");
  });
});
