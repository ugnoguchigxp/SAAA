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
  },
): LogicalRect {
  const visible =
    !options.hidden &&
    rect.width > 0 &&
    rect.height > 0 &&
    rect.x < options.viewportWidth &&
    rect.y < options.viewportHeight &&
    rect.x + rect.width > 0 &&
    rect.y + rect.height > 0;
  return {
    x: rect.x,
    y: rect.y,
    width: rect.width,
    height: rect.height,
    visible,
  };
}

export function nextGeometryGeneration(current: number): number {
  return current + 1;
}

export function isStaleGeometry(applied: number, latest: number): boolean {
  return applied !== latest;
}
