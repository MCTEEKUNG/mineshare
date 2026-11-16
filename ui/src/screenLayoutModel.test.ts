import { expect, it } from "vitest";
import { screenDropSide, screenGeometry, screenPosition, screenSides } from "./screenLayoutModel";

it.each([320, 400, 600])("keeps all four attached positions visible at canvas width %i", width => {
  const geometry = screenGeometry(width);
  for (const side of screenSides) {
    const point = screenPosition(side, geometry);
    expect(point.x - geometry.tileWidth / 2).toBeGreaterThanOrEqual(0);
    expect(point.x + geometry.tileWidth / 2).toBeLessThanOrEqual(width);
    expect(point.y - geometry.tileHeight / 2).toBeGreaterThanOrEqual(0);
    expect(point.y + geometry.tileHeight / 2).toBeLessThanOrEqual(geometry.height);
    expect(screenDropSide(point, geometry)).toBe(side);
  }
});

it("accepts a drop near an edge and rejects drops on top of this computer or outside the canvas", () => {
  const geometry = screenGeometry(480);
  const right = screenPosition("right", geometry);
  expect(screenDropSide({ x: right.x - 20, y: right.y + 10 }, geometry)).toBe("right");
  expect(screenDropSide(geometry.local, geometry)).toBeNull();
  expect(screenDropSide({ x: 900, y: geometry.local.y }, geometry)).toBeNull();
});
