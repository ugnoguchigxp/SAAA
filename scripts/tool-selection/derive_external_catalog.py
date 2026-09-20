#!/usr/bin/env python3
"""Derives an MCP-style terse-descriptor catalog from the curated evaluation catalog.

An external MCP server publishes a tool `name`, `description`, `inputSchema` and annotations;
it does not publish the ledger's `suitable` / `unsuitable` / `operations` / `objects` hints or
usage pages. This script strips those hints so the live E5/BGE lane can be measured on the text a
real external descriptor actually provides. Tool IDs and scenario gold sets are unchanged, so the
labels are not rewritten to match whatever the implementation returns.

Usage:
  python derive_external_catalog.py <catalog.jsonl> <scenarios.jsonl> <split.json> <output-dir>
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

KEEP = ("toolId", "backendKey", "title", "purpose", "inputSchema", "outputSchema", "backendBinding")
EMPTY = ("operations", "objects", "suitable", "unsuitable")


def main() -> int:
    catalog_path, scenarios_path, split_path, output_dir = sys.argv[1:5]
    output = Path(output_dir)
    output.mkdir(parents=True, exist_ok=True)

    with Path(catalog_path).open(encoding="utf-8") as handle:
        rows = [json.loads(line) for line in handle if line.strip()]
    derived = []
    for row in rows:
        entry = {key: row.get(key) for key in KEEP if row.get(key) is not None}
        for key in EMPTY:
            entry[key] = []
        entry["requiredInputs"] = row.get("requiredInputs", [])
        derived.append(entry)

    with (output / "catalog.jsonl").open("w", encoding="utf-8") as handle:
        for row in derived:
            handle.write(json.dumps(row, ensure_ascii=False) + "\n")
    # Reuse the fixed scenarios and split: the gold tool IDs are unchanged.
    (output / "scenarios.jsonl").write_text(Path(scenarios_path).read_text(encoding="utf-8"))
    (output / "split.json").write_text(Path(split_path).read_text(encoding="utf-8"))
    curated = sum(1 for row in rows if row.get("curated"))
    print(f"derived={len(derived)} curated={curated} output={output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
