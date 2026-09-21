export type DiffLine = { kind: "same" | "added" | "removed"; text: string };

export function lineDiff(before: string, after: string): DiffLine[] {
  const left = before.replace(/\r\n?/g, "\n").split("\n");
  const right = after.replace(/\r\n?/g, "\n").split("\n");
  if (left.length * right.length > 1_000_000) {
    return [
      ...left.map((text) => ({ kind: "removed" as const, text })),
      ...right.map((text) => ({ kind: "added" as const, text })),
    ];
  }
  const rows = left.length + 1;
  const columns = right.length + 1;
  const lcs = new Uint32Array(rows * columns);
  for (let i = left.length - 1; i >= 0; i -= 1) {
    for (let j = right.length - 1; j >= 0; j -= 1) {
      lcs[i * columns + j] =
        left[i] === right[j]
          ? lcs[(i + 1) * columns + j + 1] + 1
          : Math.max(lcs[(i + 1) * columns + j], lcs[i * columns + j + 1]);
    }
  }
  const output: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < left.length || j < right.length) {
    if (i < left.length && j < right.length && left[i] === right[j]) {
      output.push({ kind: "same", text: left[i] });
      i += 1;
      j += 1;
    } else if (
      j < right.length &&
      (i === left.length || lcs[i * columns + j + 1] >= lcs[(i + 1) * columns + j])
    ) {
      output.push({ kind: "added", text: right[j++] });
    } else {
      output.push({ kind: "removed", text: left[i++] });
    }
  }
  return output;
}
