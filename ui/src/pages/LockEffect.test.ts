import { describe, expect, it } from "vitest";
import { lockPresets, matchingPreset } from "./LockEffect";

describe("Game Lock presets", () => {
  it("recognizes saved presets and preserves custom edits", () => {
    for (const [name, effect] of Object.entries(lockPresets)) {
      expect(matchingPreset(JSON.parse(JSON.stringify(effect)))).toBe(name);
      expect(effect.duration_ms).toBeGreaterThanOrEqual(300);
      expect(effect.duration_ms).toBeLessThanOrEqual(3000);
      expect(effect.thickness).toBeLessThanOrEqual(16);
      expect(matchingPreset({ ...effect, duration_ms: 1750 })).toBe("Custom");
    }
  });
});
