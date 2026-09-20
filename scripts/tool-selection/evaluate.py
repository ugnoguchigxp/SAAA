#!/usr/bin/env python3
"""Scores tool-selection evaluation results produced by the Rust pipeline.

The Rust example runs retrieval and ranking; this script only aggregates metrics so there is no
second ranking implementation. The `lane` field in the results header (mock/live) is copied into
the report so a mock run is never mistaken for the real-model result.

Usage:
  python evaluate.py --results results.json --split split.json --output metrics.json
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

RECALL_K = 30
HIT_K = 5


def load_family_split(split: dict) -> dict[str, str]:
    assignment: dict[str, str] = {}
    for name, families in (
        ("development", split.get("development", [])),
        ("validation", split.get("validation", [])),
        ("heldOut", split.get("heldOut", [])),
    ):
        for family in families:
            assignment[family] = name
    return assignment


def score(result: dict) -> dict:
    gold = [tool for tool in result.get("gold", []) if tool]
    predicted = result.get("predicted", [])
    if not gold:
        return {"eligible": False}
    top_k = predicted[:RECALL_K]
    hit = len(set(gold) & set(top_k))
    top5 = predicted[:HIT_K]
    return {
        "eligible": True,
        "recall": hit / len(gold),
        "hit_at_5": 1.0 if set(gold) & set(top5) else 0.0,
        "found": hit,
        "gold": len(gold),
    }


def aggregate(results: list[dict], split_assignment: dict[str, str]) -> dict:
    buckets = {"all": [], "development": [], "validation": [], "heldOut": []}
    no_match_total = 0
    no_match_accepted = 0
    no_match_degraded = 0
    degraded = 0
    for result in results:
        scored = score(result)
        if result.get("noMatch"):
            # A degraded run must not fake no-match confidence; it is reported separately and
            # excluded from the false-accept denominator.
            if result.get("degraded"):
                no_match_degraded += 1
            else:
                no_match_total += 1
                if result.get("status") != "no_match":
                    no_match_accepted += 1
        if result.get("degraded"):
            degraded += 1
        if not scored["eligible"]:
            continue
        buckets["all"].append(scored)
        split_name = split_assignment.get(result.get("family") or "", "development")
        buckets.setdefault(split_name, []).append(scored)

    report: dict = {
        "recallAt30": {},
        "hitAt5": {},
        "counts": {},
        "noMatch": {
            "total": no_match_total,
            "accepted": no_match_accepted,
            "falseAcceptRate": (no_match_accepted / no_match_total) if no_match_total else None,
            "degraded": no_match_degraded,
        },
        "degraded": degraded,
    }
    for name, scored in buckets.items():
        if not scored:
            continue
        report["recallAt30"][name] = sum(item["recall"] for item in scored) / len(scored)
        report["hitAt5"][name] = sum(item["hit_at_5"] for item in scored) / len(scored)
        report["counts"][name] = len(scored)
    return report


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--results", required=True)
    parser.add_argument("--split", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()

    results = json.loads(Path(args.results).read_text(encoding="utf-8"))
    split = json.loads(Path(args.split).read_text(encoding="utf-8"))
    assignment = load_family_split(split)
    report = aggregate(results.get("results", []), assignment)
    report["lane"] = results.get("lane")
    report["embeddingModelHash"] = results.get("embeddingModelHash")
    report["catalogSize"] = results.get("catalogSize")
    report["recallK"] = RECALL_K
    report["hitK"] = HIT_K
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
