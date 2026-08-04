#!/usr/bin/env python3
"""Dependency-free validation for a generated semantic corpus v3."""

import argparse
import json
from pathlib import Path

from split_validation import validate_record_split


def load_and_validate(path):
    records = []
    with Path(path).open() as source:
        for line_number, line in enumerate(source, 1):
            record = json.loads(line)
            closure = record.get("semantic_v3")
            if not closure or closure.get("corpus_schema_id") != "bpmn.semantic-corpus.v3":
                raise ValueError(f"line {line_number}: missing semantic corpus v3 closure")
            if not closure.get("board_inclusion_truth"):
                raise ValueError(f"line {line_number}: gold candidate is off-board")
            if record["label"] not in closure.get("full_served_list", []):
                raise ValueError(f"line {line_number}: full board omitted gold candidate")
            if len(closure.get("candidate_pairs", [])) != len(closure["full_served_list"]):
                raise ValueError(f"line {line_number}: pair/list cardinality mismatch")
            records.append(record)
    return records


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("corpus", type=Path)
    parser.add_argument("--split-manifest", type=Path)
    args = parser.parse_args()
    records = load_and_validate(args.corpus)
    if args.split_manifest:
        manifest = json.loads(args.split_manifest.read_text())
        split = {
            record["example_id"]: manifest["family_split"][record["family_id"]]
            for record in records
        }
        validate_record_split(records, split)
    print(f"validated {len(records)} semantic v3 records")


if __name__ == "__main__":
    main()
