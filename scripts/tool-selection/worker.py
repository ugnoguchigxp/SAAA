#!/usr/bin/env python3
"""Fixed local ML worker for tool selection (JSONL protocol version 1).

One process is started once by the Rust client and reused. Models are loaded only from the local
paths recorded in the manifest; no model is downloaded and no remote code is executed here.
stdout carries protocol lines only; diagnostics go to stderr.

Protocol (one JSON object per line):

  {"version":1,"id":"...","op":"embed","kind":"query","texts":["..."]}
  {"version":1,"id":"...","ok":true,"vectors":[[...]],"dimension":384,"modelHash":"..."}
  {"version":1,"id":"...","op":"rerank","query":"...","documents":[{"id":"r1","text":"..."}]}
  {"version":1,"id":"...","ok":true,"scores":[{"id":"r1","value":1.2}],"modelHash":"..."}
"""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any

QUERY_PREFIX = "query: "
PASSAGE_PREFIX = "passage: "
MAX_LENGTH = 512
BATCH_SIZE = 8


def load_manifest(path: str) -> dict[str, Any]:
    with open(path, "r", encoding="utf-8") as handle:
        manifest = json.load(handle)
    if manifest.get("formatVersion") != 1:
        raise ValueError("unsupported manifest formatVersion")
    for key in ("embedding", "reranker"):
        section = manifest.get(key)
        if not isinstance(section, dict) or not section.get("localPath"):
            raise ValueError(f"manifest is missing {key}.localPath")
        if not section.get("hash"):
            raise ValueError(f"manifest is missing {key}.hash")
    if not isinstance(manifest["embedding"].get("dimension"), int):
        raise ValueError("manifest is missing embedding.dimension")
    return manifest


class Worker:
    def __init__(self, manifest: dict[str, Any]) -> None:
        self.manifest = manifest
        self.embedding_hash = manifest["embedding"]["hash"]
        self.reranker_hash = manifest["reranker"]["hash"]
        self.dimension = int(manifest["embedding"]["dimension"])
        self._embedder = None
        self._reranker = None

    def embedder(self):
        if self._embedder is None:
            from sentence_transformers import SentenceTransformer

            self._embedder = SentenceTransformer(
                self.manifest["embedding"]["localPath"], local_files_only=True
            )
        return self._embedder

    def reranker(self):
        if self._reranker is None:
            import torch
            from transformers import AutoModelForSequenceClassification, AutoTokenizer

            path = self.manifest["reranker"]["localPath"]
            tokenizer = AutoTokenizer.from_pretrained(path, local_files_only=True)
            model = AutoModelForSequenceClassification.from_pretrained(
                path, local_files_only=True
            )
            model.eval()
            torch.manual_seed(0)
            self._reranker = (tokenizer, model)
        return self._reranker

    def embed(self, request: dict[str, Any]) -> dict[str, Any]:
        texts = request.get("texts")
        if not isinstance(texts, list) or any(not isinstance(t, str) for t in texts):
            return self.error(request, "invalid texts")
        prefix = QUERY_PREFIX if request.get("kind") == "query" else PASSAGE_PREFIX
        encoded = [f"{prefix}{text}" for text in texts]
        vectors = self.embedder().encode(
            encoded, normalize_embeddings=True, convert_to_numpy=True
        )
        values = [[float(x) for x in vector] for vector in vectors]
        for vector in values:
            if len(vector) != self.dimension:
                return self.error(request, "embedding dimension mismatch")
        return {
            "version": 1,
            "id": request["id"],
            "ok": True,
            "vectors": values,
            "dimension": self.dimension,
            "modelHash": self.embedding_hash,
        }

    def rerank(self, request: dict[str, Any]) -> dict[str, Any]:
        import torch

        documents = request.get("documents")
        query = request.get("query")
        if not isinstance(documents, list) or not isinstance(query, str):
            return self.error(request, "invalid rerank request")
        tokenizer, model = self.reranker()
        scores: list[dict[str, Any]] = []
        with torch.no_grad():
            for start in range(0, len(documents), BATCH_SIZE):
                batch = documents[start : start + BATCH_SIZE]
                pairs = [[query, str(doc.get("text", ""))] for doc in batch]
                inputs = tokenizer(
                    pairs,
                    padding=True,
                    truncation=True,
                    max_length=MAX_LENGTH,
                    return_tensors="pt",
                )
                logits = model(**inputs).logits.view(-1).float()
                for doc, value in zip(batch, logits.tolist()):
                    scores.append({"id": doc.get("id"), "value": float(value)})
        return {
            "version": 1,
            "id": request["id"],
            "ok": True,
            "scores": scores,
            "modelHash": self.reranker_hash,
        }

    @staticmethod
    def error(request: dict[str, Any], message: str) -> dict[str, Any]:
        return {
            "version": 1,
            "id": request.get("id", ""),
            "ok": False,
            "error": message,
        }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", required=True)
    args = parser.parse_args()
    manifest = load_manifest(args.manifest)
    worker = Worker(manifest)
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            print(json.dumps({"version": 1, "ok": False, "error": "invalid json"}), flush=True)
            continue
        operation = request.get("op")
        if operation == "embed":
            response = worker.embed(request)
        elif operation == "rerank":
            response = worker.rerank(request)
        else:
            response = Worker.error(request, "unknown op")
        print(json.dumps(response, ensure_ascii=False), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
