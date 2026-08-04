#!/usr/bin/env python3
"""Fail closed unless every committed fuzz regression is governed and intact."""

from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path


REQUIRED_FIELDS = {
    "finding_id",
    "target",
    "regression_path",
    "fixed_commit",
    "expected_current_outcome",
    "input_sha256",
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fail(message: str) -> None:
    raise ValueError(message)


def validate(root: Path) -> int:
    manifest_path = root / "fuzz-regressions.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 1:
        fail("fuzz-regressions.json must use schema_version 1")

    cases = manifest.get("cases")
    if not isinstance(cases, list) or not cases:
        fail("fuzz-regressions.json must govern at least one regression case")

    governed: set[str] = set()
    for index, case in enumerate(cases):
        if not isinstance(case, dict):
            fail(f"case {index} must be an object")
        missing = sorted(REQUIRED_FIELDS - case.keys())
        if missing:
            fail(f"case {index} is missing fields: {', '.join(missing)}")
        if any(not isinstance(case[field], str) or not case[field].strip() for field in REQUIRED_FIELDS):
            fail(f"case {index} has an empty or non-string required field")

        relative = case["regression_path"]
        if relative in governed:
            fail(f"duplicate governed regression path: {relative}")
        governed.add(relative)
        expected_fragment = f"/fuzz/regressions/{case['target']}/"
        if expected_fragment not in f"/{relative}":
            fail(f"{relative} is not in the {case['target']} regression directory")

        path = root / relative
        if not path.is_file():
            fail(f"governed regression input does not exist: {relative}")
        actual_hash = sha256(path)
        if actual_hash != case["input_sha256"]:
            fail(f"SHA-256 mismatch for {relative}: {actual_hash}")

        source_relative = case.get("source_artifact_path")
        source_hash = case.get("source_artifact_sha256")
        if bool(source_relative) != bool(source_hash):
            fail(f"case {index} must provide both source artifact path and hash")
        if source_relative:
            source_path = root / source_relative
            if not source_path.is_file():
                fail(f"source artifact does not exist: {source_relative}")
            actual_source_hash = sha256(source_path)
            if actual_source_hash != source_hash:
                fail(f"SHA-256 mismatch for {source_relative}: {actual_source_hash}")

    committed = {
        path.relative_to(root).as_posix()
        for path in root.glob("*/fuzz/regressions/**/*")
        if path.is_file() and path.name != ".gitkeep"
    }
    ungoverned = sorted(committed - governed)
    missing_inputs = sorted(governed - committed)
    if ungoverned:
        fail(f"ungoverned regression inputs: {', '.join(ungoverned)}")
    if missing_inputs:
        fail(f"manifest entries outside the committed corpus: {', '.join(missing_inputs)}")

    print(f"validated {len(cases)} governed fuzz regression case(s)")
    return len(cases)


if __name__ == "__main__":
    try:
        validate(Path(__file__).resolve().parents[1])
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"fuzz regression validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
