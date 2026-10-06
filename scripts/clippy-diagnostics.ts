/** Cargo JSON includes rendered source spans; keep them in failed verify output. */
export function renderedDiagnostics(output: string): string {
  return output
    .split("\n")
    .map((line) => {
      try {
        const item = JSON.parse(line) as {
          reason?: string;
          message?: { rendered?: string; message?: string };
        };
        return item.reason === "compiler-message"
          ? (item.message?.rendered ?? item.message?.message ?? line)
          : "";
      } catch {
        return line;
      }
    })
    .filter(Boolean)
    .join("\n");
}
