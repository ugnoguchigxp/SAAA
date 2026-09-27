#!/usr/bin/env python3
"""Regenerate the conservative TTS preset intersection from SudachiDict and JMdict.

Usage: python3 scripts/update-tts-presets.py --small small_lex.zip --core core_lex.zip --jmdict JMdict_e.gz
The source archives are obtained from the upstream URLs documented in
src-tauri/data/tts-presets.SOURCES.md. The output is deterministic for those inputs.
"""
import argparse
import collections
import csv
import gzip
import io
import re
import xml.etree.ElementTree as ET
import zipfile
from pathlib import Path

KANJI = re.compile(r"[一-龯々]")
KATAKANA = re.compile(r"[ァ-ヴー]+\Z")


def katakana(text: str) -> str:
    return "".join(chr(ord(char) + 96) if "ぁ" <= char <= "ゖ" else char for char in text)


def valid_written(text: str) -> bool:
    return 2 <= len(text) <= 100 and KANJI.search(text) is not None and not any(char.isspace() or ord(char) < 32 for char in text)


def sudachi(paths: list[Path]) -> dict[str, set[str]]:
    readings = collections.defaultdict(set)
    for path in paths:
        member = path.stem + ".csv"
        with zipfile.ZipFile(path).open(member) as stream:
            for row in csv.reader(io.TextIOWrapper(stream, encoding="utf-8")):
                if len(row) != 19 or row[6] == "固有名詞":
                    continue
                written, reading = row[4], row[11]
                if valid_written(written) and KATAKANA.fullmatch(reading) and written != reading:
                    readings[written].add(reading)
    return readings


def jmdict(path: Path) -> dict[str, set[str]]:
    readings = collections.defaultdict(set)
    with gzip.open(path, "rb") as stream:
        for _, entry in ET.iterparse(stream, events=("end",)):
            if entry.tag != "entry":
                continue
            written_forms = [item.text for item in entry.findall("./k_ele/keb") if item.text]
            for reading_element in entry.findall("./r_ele"):
                if reading_element.find("re_nokanji") is not None:
                    continue
                reading = katakana(reading_element.findtext("reb") or "")
                if not KATAKANA.fullmatch(reading):
                    continue
                restricted = [item.text for item in reading_element.findall("re_restr") if item.text]
                for written in restricted or written_forms:
                    if valid_written(written):
                        readings[written].add(reading)
            entry.clear()
    return readings


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--small", type=Path, required=True)
    parser.add_argument("--core", type=Path, required=True)
    parser.add_argument("--jmdict", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("src-tauri/data/tts-presets.tsv"))
    args = parser.parse_args()
    first = sudachi([args.small, args.core])
    second = jmdict(args.jmdict)
    records = []
    for written, sudachi_readings in first.items():
        jm_readings = second.get(written)
        if len(sudachi_readings) == len(jm_readings or ()) == 1:
            reading = next(iter(sudachi_readings))
            if reading in jm_readings:
                records.append((written, reading))
    records.sort()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("w", encoding="utf-8", newline="") as stream:
        writer = csv.writer(stream, delimiter="\t", lineterminator="\n")
        writer.writerows(records)
    print(f"{len(records)} presets written to {args.output}")


if __name__ == "__main__":
    main()
