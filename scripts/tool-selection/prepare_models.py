#!/usr/bin/env python3
"""Downloads and locks the tool-selection models, then writes a manifest.

This is a separate command from inference: the app only ever loads local files from the paths in
the manifest. Model IDs are fixed by the implementation guide; the no-match threshold is chosen
on the validation split and stored here.

Usage:
  python prepare_models.py --models-dir <dir> --manifest <manifest.json> [--skip-download]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import sys
from pathlib import Path

EMBEDDING_MODEL = "intfloat/multilingual-e5-small"
RERANKER_MODEL = "BAAI/bge-reranker-v2-m3"
LOCK_PACKAGES = (
    "sentence-transformers",
    "transformers",
    "torch",
    "huggingface-hub",
)


def tree_hash(directory: Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(p for p in directory.rglob("*") if p.is_file()):
        digest.update(path.relative_to(directory).as_posix().encode("utf-8"))
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
    return digest.hexdigest()


def snapshot(model_id: str, target: Path) -> Path:
    from huggingface_hub import snapshot_download

    return Path(
        snapshot_download(
            repo_id=model_id,
            local_dir=str(target),
            allow_patterns=[
                "*.json",
                "*.txt",
                "*.model",
                "*.safetensors",
                "*.bin",
                "*.spm",
            ],
        )
    )


def package_versions() -> dict[str, str]:
    from importlib.metadata import PackageNotFoundError, version

    versions: dict[str, str] = {}
    for name in LOCK_PACKAGES:
        try:
            versions[name] = version(name)
        except PackageNotFoundError:
            versions[name] = "missing"
    return versions


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--models-dir", required=True)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--no-match-threshold", type=float, default=0.0)
    parser.add_argument("--skip-download", action="store_true")
    args = parser.parse_args()

    models_dir = Path(args.models_dir)
    models_dir.mkdir(parents=True, exist_ok=True)
    embedding_dir = models_dir / "multilingual-e5-small"
    reranker_dir = models_dir / "bge-reranker-v2-m3"

    if not args.skip_download:
        snapshot(EMBEDDING_MODEL, embedding_dir)
        snapshot(RERANKER_MODEL, reranker_dir)
    else:
        for directory in (embedding_dir, reranker_dir):
            if not directory.exists():
                print(f"missing model directory: {directory}", file=sys.stderr)
                return 2

    from sentence_transformers import SentenceTransformer

    encoder = SentenceTransformer(str(embedding_dir), local_files_only=True)
    dimension = int(encoder.get_sentence_embedding_dimension())

    manifest = {
        "formatVersion": 1,
        "embedding": {
            "model": EMBEDDING_MODEL,
            "localPath": str(embedding_dir.resolve()),
            "hash": tree_hash(embedding_dir),
            "dimension": dimension,
            "queryPrefix": "query: ",
            "passagePrefix": "passage: ",
        },
        "reranker": {
            "model": RERANKER_MODEL,
            "localPath": str(reranker_dir.resolve()),
            "hash": tree_hash(reranker_dir),
            "maxLength": 512,
            "batchSize": 8,
        },
        "noMatchThreshold": args.no_match_threshold,
        "packages": package_versions(),
        "platform": {
            "python": platform.python_version(),
            "system": platform.platform(),
        },
    }
    manifest_path = Path(args.manifest)
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    print(f"wrote {manifest_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
