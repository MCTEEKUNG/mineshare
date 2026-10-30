export type PeerSide = "left" | "right" | "top" | "bottom";
export type Point = { x: number; y: number };
export const screenSides: PeerSide[] = ["left", "right", "top", "bottom"];

export function screenGeometry(width: number) {
  const tileWidth = Math.min(152, (width - 32) / 3);
  const tileHeight = tileWidth * 10 / 16;
  const gap = 4;
  const height = tileHeight * 3 + gap * 2 + 32;
  return { width, height, tileWidth, tileHeight, gap, local: { x: width / 2, y: height / 2 } };
}
export type ScreenGeometry = ReturnType<typeof screenGeometry>;

export function screenPosition(side: PeerSide, geometry: ScreenGeometry): Point {
  const { local, tileWidth, tileHeight, gap } = geometry;
  switch (side) {
    case "left": return { x: local.x - tileWidth - gap, y: local.y };
    case "right": return { x: local.x + tileWidth + gap, y: local.y };
    case "top": return { x: local.x, y: local.y - tileHeight - gap };
    case "bottom": return { x: local.x, y: local.y + tileHeight + gap };
  }
}

/** Snap near an adjacent screen slot; overlapping or distant drops cancel. */
export function screenDropSide(point: Point, geometry: ScreenGeometry): PeerSide | null {
  let closest: PeerSide | null = null;
  let distance = .75 ** 2;
  for (const side of screenSides) {
    const target = screenPosition(side, geometry);
    const candidate = ((point.x - target.x) / geometry.tileWidth) ** 2
      + ((point.y - target.y) / geometry.tileHeight) ** 2;
    if (candidate < distance) { closest = side; distance = candidate; }
  }
  return closest;
}
