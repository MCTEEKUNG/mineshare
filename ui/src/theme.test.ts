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
