export type LogicalRect = {
  x: number;
  y: number;
  width: number;
  height: number;
  visible: boolean;
};

export function logicalRectFromDom(
  rect: { x: number; y: number; width: number; height: number },
  options: {
    viewportWidth: number;
    viewportHeight: number;
    hidden?: boolean;
    clip?: { x: number; y: number; width: number; height: number };
  },
): LogicalRect {
  let x = rect.x;
  let y = rect.y;
  let width = rect.width;
  let height = rect.height;
  if (options.clip) {
    const right = Math.min(x + width, options.clip.x + options.clip.width);
    const bottom = Math.min(y + height, options.clip.y + options.clip.height);
    x = Math.max(x, options.clip.x);
    y = Math.max(y, options.clip.y);
    width = right - x;
    height = bottom - y;
  }
  const visible =
    !options.hidden &&
    width > 0 &&
    height > 0 &&
    x < options.viewportWidth &&
    y < options.viewportHeight &&
    x + width > 0 &&
    y + height > 0;
  return { x, y, width, height, visible };
}

export function nextGeometryGeneration(current: number): number {
  return current + 1;
}

export function isStaleGeometry(applied: number, latest: number): boolean {
  return applied !== latest;
}
