import { expect, it } from "vitest";
import { comparePeerBuild } from "./buildCompatibility";

const desktop = "0.0.32 · abc123 · 2026-11-27 · src:123 · control-v5";

it("keeps exact desktop source comparison while accepting the matching Android release", () => {
  expect(comparePeerBuild(desktop, desktop, "windows")).toBe("same_build");
  expect(comparePeerBuild(desktop, desktop.replace("src:123", "src:456"), "linux")).toBe("different_build");
  expect(comparePeerBuild(desktop, "0.0.32 · Android · control-v5", "android")).toBe("compatible_android");
});

it("reports Android release or protocol mismatches and missing protocol metadata", () => {
  expect(comparePeerBuild(desktop, "0.0.33 · Android · control-v5", "android")).toBe("different_build");
  expect(comparePeerBuild(desktop, "0.0.32 · Android · control-v4", "android")).toBe("different_build");
  expect(comparePeerBuild(desktop, "0.0.32 · Android", "android")).toBe("different_build");
  expect(comparePeerBuild("0.0.32", "0.0.32 · Android", "android")).toBe("different_build");
});
