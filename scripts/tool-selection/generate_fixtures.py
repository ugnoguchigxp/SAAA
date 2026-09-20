#!/usr/bin/env python3
"""Generates the reproducible tool-selection evaluation fixtures.

Curated entries (100) have distinct purpose/inputs/suitability; scale entries exist only to
measure 1,500/10,000-item behaviour and are marked `"curated": false`. Scenario families are the
split unit; paraphrases never cross a split.
"""

from __future__ import annotations

import argparse
import json
import random
from pathlib import Path

# name, operation, object, purpose, suitable, unsuitable
FAMILIES = [
    ("decision_history", "search", "decision_record", "Search past project decisions and their rationale.", ["project history", "why a decision was made"], ["live data"]),
    ("meeting_minutes", "search", "decision_record", "Search internal meeting minutes for decisions.", ["internal records", "meeting outcomes"], ["public web content"]),
    ("web_research", "search", "current_information", "Search the public web for current information.", ["latest news", "public facts"], ["private documents"]),
    ("competitor_news", "search", "current_information", "Track competitor announcements and news.", ["market monitoring"], ["internal planning"]),
    ("contract_lookup", "read", "document", "Read contract documents and clauses.", ["legal review"], ["code execution"]),
    ("spec_reader", "read", "document", "Read product specification documents.", ["requirements review"], ["sending email"]),
    ("table_query", "read", "table", "Read rows from a structured table.", ["aggregation", "filtering"], ["unstructured text"]),
    ("code_search", "search", "code", "Search a source code repository.", ["symbol lookup"], ["calendar events"]),
    ("code_edit", "update", "code", "Apply a code change.", ["refactoring"], ["reading only"]),
    ("file_search", "search", "file", "Search local files by content.", ["local documents"], ["remote APIs"]),
    ("file_extract", "extract", "file", "Extract text and metadata from files.", ["pdf parsing"], ["live calls"]),
    ("summarize_doc", "summarize", "document", "Summarize a long document.", ["digest generation"], ["precise quotes"]),
    ("compare_versions", "compare", "document", "Compare two document versions.", ["diffing"], ["sending"]),
    ("calendar_read", "read", "calendar_item", "Read calendar events.", ["scheduling"], ["file access"]),
    ("calendar_create", "create", "calendar_item", "Create a calendar event.", ["scheduling"], ["reading history"]),
    ("message_search", "search", "message", "Search chat messages.", ["conversation lookup"], ["web search"]),
    ("message_send", "send", "message", "Send a chat message.", ["notifications"], ["reading private files"]),
    ("email_send", "send", "message", "Send an email.", ["outreach"], ["bulk spam"]),
    ("transcript_read", "read", "message", "Read meeting transcripts.", ["minutes generation"], ["public web"]),
    ("metrics_query", "read", "table", "Query product metrics.", ["trend analysis"], ["sending messages"]),
    ("log_search", "search", "table", "Search application logs.", ["incident debugging"], ["business decisions"]),
    ("ticket_search", "search", "message", "Search support tickets.", ["customer issues"], ["code edits"]),
    ("ticket_update", "update", "message", "Update a support ticket.", ["triage"], ["reading calendars"]),
    ("crm_lookup", "read", "table", "Look up CRM records.", ["account context"], ["file deletion"]),
    ("invoice_extract", "extract", "document", "Extract invoice line items.", ["accounting"], ["web browsing"]),
    ("translate_text", "transform", "document", "Translate text between languages.", ["localization"], ["authoritative legal advice"]),
    ("image_describe", "extract", "file", "Describe an image.", ["accessibility"], ["precise measurement"]),
    ("audio_transcribe", "extract", "file", "Transcribe audio.", ["meeting notes"], ["summarizing tables"]),
    ("sql_query", "read", "table", "Run a read-only SQL query.", ["analytics"], ["schema changes"]),
    ("sql_execute", "execute", "table", "Execute a SQL statement with side effects.", ["migrations"], ["read-only lookups"]),
    ("deploy_status", "read", "code", "Check deployment status.", ["release checks"], ["editing code"]),
    ("deploy_trigger", "execute", "code", "Trigger a deployment.", ["release"], ["reading history"]),
    ("incident_page", "send", "message", "Page an on-call responder.", ["incidents"], ["routine questions"]),
    ("wiki_search", "search", "document", "Search an internal wiki.", ["internal knowledge"], ["live market data"]),
    ("wiki_update", "update", "document", "Update an internal wiki page.", ["documentation"], ["sending email"]),
    ("policy_lookup", "read", "document", "Look up company policy.", ["compliance"], ["code search"]),
    ("budget_query", "read", "table", "Read budget figures.", ["finance"], ["sending messages"]),
    ("purchase_create", "create", "document", "Create a purchase request.", ["procurement"], ["reading calendars"]),
    ("vendor_search", "search", "document", "Search approved vendors.", ["procurement"], ["code execution"]),
    ("risk_assess", "compare", "document", "Compare risks across options.", ["planning"], ["live data"]),
    ("roadmap_read", "read", "document", "Read the product roadmap.", ["planning"], ["sending messages"]),
    ("experiment_query", "read", "table", "Read A/B experiment results.", ["product analytics"], ["legal review"]),
    ("feedback_search", "search", "message", "Search customer feedback.", ["product research"], ["calendar edits"]),
    ("skill_lookup", "read", "table", "Look up employee skills.", ["staffing"], ["web browsing"]),
    ("survey_extract", "extract", "document", "Extract survey responses.", ["research"], ["sending messages"]),
    ("doc_convert", "transform", "file", "Convert documents between formats.", ["formatting"], ["answering questions"]),
    ("archive_search", "search", "document", "Search an archived document store.", ["historical records"], ["live data"]),
    ("schema_read", "read", "table", "Read a database schema.", ["data modeling"], ["running queries"]),
    ("notification_send", "send", "message", "Send a system notification.", ["alerts"], ["reading files"]),
    ("task_track", "read", "message", "Read tracked tasks and their status.", ["project tracking"], ["sending email"]),
]


def input_schema(operation: str) -> dict:
    properties = {"query": {"type": "string"}}
    if operation in ("create", "update", "send", "execute"):
        properties["payload"] = {"type": "string"}
    return {
        "type": "object",
        "properties": properties,
        "required": ["query"],
        "additionalProperties": False,
    }


def catalog_entry(tool_id: str, family: tuple, source: str, curated: bool) -> dict:
    name, operation, obj, purpose, suitable, unsuitable = family
    suffix = "public" if source == "web" else "internal"
    return {
        "toolId": tool_id,
        "backendKey": tool_id,
        "title": f"{name} ({suffix})",
        "purpose": f"{purpose} Source: {suffix}.",
        "operations": [operation],
        "objects": [obj],
        "suitable": suitable,
        "unsuitable": unsuitable,
        "requiredInputs": ["query"],
        "inputSchema": input_schema(operation),
        "effect": "write" if operation in ("create", "update", "send", "delete", "execute") else "read",
        "usage": [
            {
                "section": "usage",
                "page": 0,
                "text": f"Use {tool_id} for {obj} {operation} tasks. It is suitable for {', '.join(suitable)}.",
            }
        ],
        "backendBinding": {"capabilityId": tool_id, "revisionId": f"{tool_id}-rev1"},
        "curated": curated,
    }


# Unrelated capability vocabulary for scale-only entries. These must NOT duplicate a curated
# family's purpose, otherwise the gold tool becomes semantically indistinguishable and the
# retrieval metric measures label ambiguity rather than retrieval quality.
NOISE_VERBS = [
    "publish", "schedule", "reconcile", "encrypt", "render", "compress", "provision",
    "rotate", "replicate", "audit", "anonymize", "throttle", "shard", "index", "replay",
    "retry", "quarantine", "purge", "watermark", "notarize", "tunnel", "cache", "sample",
    "calibrate", "synthesize", "classify", "cluster", "forecast", "lint", "fuzz",
]
NOISE_OBJECTS = [
    "telemetry stream", "billing export", "container image", "feature flag", "session token",
    "route table", "key bundle", "snapshot archive", "queue backlog", "schema diff",
    "rate limit", "webhook payload", "artifact manifest", "media asset", "edge cache",
    "job scheduler", "disk volume", "network policy", "build cache", "metric rollup",
]


def scale_entry(index: int, rng: random.Random) -> dict:
    verb = rng.choice(NOISE_VERBS)
    obj = rng.choice(NOISE_OBJECTS)
    tool_id = f"scale_{index:04d}_{verb}_{obj.replace(' ', '_')}"
    return {
        "toolId": tool_id,
        "backendKey": tool_id,
        "title": f"{verb} {obj}",
        "purpose": f"Infrastructure utility to {verb} a {obj} for platform operations.",
        "operations": [rng.choice(["read", "update", "create", "execute"])],
        "objects": [rng.choice(["table", "code", "file", "message"])],
        "suitable": ["platform operations"],
        "unsuitable": ["business knowledge work"],
        "requiredInputs": ["query"],
        "inputSchema": input_schema("read"),
        "effect": "read",
        "usage": [{"section": "usage", "page": 0, "text": f"Runs the {verb} {obj} operation."}],
        "backendBinding": {"capabilityId": tool_id, "revisionId": f"{tool_id}-rev1"},
        "curated": False,
    }


def build(seed: int, curated: int, scale: int) -> tuple[list, list, list, dict]:
    rng = random.Random(seed)
    catalog = []
    for family in FAMILIES:
        catalog.append(catalog_entry(f"{family[0]}_web", family, "web", True))
        catalog.append(catalog_entry(f"{family[0]}_internal", family, "internal", True))
    catalog = catalog[:curated]
    for index in range(scale):
        catalog.append(scale_entry(index + 1, rng))

    curated_tools = [entry["toolId"] for entry in catalog if entry["curated"]]
    scenarios = []
    families = sorted({entry["toolId"].rsplit("_", 1)[0] for entry in catalog if entry["curated"]})
    for index, family in enumerate(families):
        tools = [tool for tool in curated_tools if tool.startswith(family + "_")]
        if not tools:
            continue
        purpose = next(
            entry["purpose"] for entry in catalog if entry["toolId"] == tools[0]
        )
        intents = [
            purpose,
            f"I need help to {purpose[0].lower()}{purpose[1:]}",
            f"次の作業を手伝って: {purpose}",
        ]
        for variant in range(3):
            scenarios.append(
                {
                    "id": f"scenario_{index:03d}_{variant}",
                    "family": family,
                    "intent": intents[variant],
                    "operation": "search",
                    "objectType": "decision_record",
                    "project": "A" if variant % 2 == 0 else None,
                    "gold": tools if variant == 0 else [tools[0]],
                }
            )

    for index in range(20):
        scenarios.append(
            {
                "id": f"nomatch_{index:03d}",
                "family": f"nomatch_{index:03d}",
                "intent": f"unrelated request {index}",
                "operation": "unknown",
                "objectType": "unknown",
                "project": None,
                "gold": [],
                "noMatch": True,
            }
        )

    corrections = []
    for index, family in enumerate(families):
        tools = [tool for tool in curated_tools if tool.startswith(family + "_")]
        if len(tools) < 2:
            continue
        for variant, (scope, duration) in enumerate(
            (("project", "persistent"), ("conversation", "once"))
        ):
            corrections.append(
                {
                    "id": f"correction_{index:03d}_{variant}",
                    "family": family,
                    "message": f"この案件では{family}は{tools[0]}ではなく{tools[1]}を使って",
                    "decision": f"scenario_{index:03d}_0",
                    "rejected": tools[0],
                    "preferred": tools[1],
                    "scope": scope,
                    "duration": duration,
                    "condition": {
                        "operation": "search",
                        "objectType": "decision_record",
                        "phase": None,
                        "inputKind": None,
                    },
                    "expect": {
                        "appliesWhen": {"objectType": "decision_record", "project": "A"},
                        "notAppliesWhen": [
                            {"objectType": "current_information", "project": "A"},
                            {"objectType": "decision_record", "project": "B"},
                        ],
                    },
                }
            )

    shuffled = families[:]
    rng.shuffle(shuffled)
    development = shuffled[: int(len(shuffled) * 0.6)]
    validation = shuffled[int(len(shuffled) * 0.6) : int(len(shuffled) * 0.8)]
    held_out = shuffled[int(len(shuffled) * 0.8) :]
    # no_match scenarios are their own families; they are placed in validation so the
    # no-match threshold is selected on validation, never on held-out.
    validation = validation + [f"nomatch_{index:03d}" for index in range(20)]
    split = {
        "seed": seed,
        "unit": "scenario-family",
        "development": development,
        "validation": validation,
        "heldOut": held_out,
    }
    return catalog, scenarios, corrections, split


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--output",
        default="src-tauri/tests/fixtures/tool-selection",
    )
    parser.add_argument("--curated", type=int, default=100)
    parser.add_argument("--scale", type=int, default=1400)
    parser.add_argument("--seed", type=int, default=20260920)
    args = parser.parse_args()

    catalog, scenarios, corrections, split = build(args.seed, args.curated, args.scale)
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    for name, rows in (
        ("catalog.jsonl", catalog),
        ("scenarios.jsonl", scenarios),
        ("corrections.jsonl", corrections),
    ):
        with (output / name).open("w", encoding="utf-8") as handle:
            for row in rows:
                handle.write(json.dumps(row, ensure_ascii=False) + "\n")
    (output / "split.json").write_text(json.dumps(split, indent=2) + "\n", encoding="utf-8")
    print(
        f"catalog={len(catalog)} scenarios={len(scenarios)} corrections={len(corrections)} "
        f"families={len(split['development']) + len(split['validation']) + len(split['heldOut'])}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
