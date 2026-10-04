export type Row = Record<string, unknown>;

export function parseRows(text: string): Row[] {
  return text
    .split("\n")
    .filter((line) => line.trim())
    .map((line) => {
      const raw: unknown = JSON.parse(line);
      if (!raw || typeof raw !== "object" || Array.isArray(raw))
        throw new Error("Expected observation objects");
      const row = raw as Row;
      const value = typeof row.attributes_json === "string" ? JSON.parse(row.attributes_json) : row;
      if (!value || typeof value !== "object" || Array.isArray(value))
        throw new Error("Expected observation objects");
      if (value.schemaVersion !== 1) throw new Error("Unsupported observation schema");
      return value;
    });
}
