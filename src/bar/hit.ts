export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** The smallest rectangle covering all visible parts of the bar (rounded out to whole pixels).
 *  An empty list gives a zero-size rectangle: nothing on the bar is clickable. */
export function unionRect(rects: Rect[]): Rect {
  const visible = rects.filter((r) => r.width > 0 && r.height > 0);
  if (visible.length === 0) return { x: 0, y: 0, width: 0, height: 0 };
  const left = Math.floor(Math.min(...visible.map((r) => r.x)));
  const top = Math.floor(Math.min(...visible.map((r) => r.y)));
  const right = Math.ceil(Math.max(...visible.map((r) => r.x + r.width)));
  const bottom = Math.ceil(Math.max(...visible.map((r) => r.y + r.height)));
  return { x: left, y: top, width: right - left, height: bottom - top };
}

export const sameRect = (a: Rect | null, b: Rect | null) =>
  !!a && !!b && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;
