#!/usr/bin/env python3
"""H6 (EOP-PLAN-CRATE-HYGIENE-001): R3 test-topology gate.

Every Cargo integration-test target (a direct child of some crate's tests/
directory) must carry an R3 classification in
docs/generated/test-topology-manifest.txt, and xtask/tests/ must be the sole
home for multi-crate application tests (plan §3 decision 1): a `multi`
classification anywhere outside xtask/tests/, or a non-`multi` target inside
it, fails closed rather than silently drifting.
"""

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "docs/generated/test-topology-manifest.txt"
CLASSES = {"intra", "inter", "multi"}
EXCLUDE_DIR_PARTS = {"target", ".git", "fuzz"}


def discover_targets():
    targets = []
    for tests_dir in ROOT.glob("*/tests"):
        if any(part in EXCLUDE_DIR_PARTS for part in tests_dir.parts):
            continue
        for rust_file in tests_dir.glob("*.rs"):
            targets.append(rust_file.relative_to(ROOT).as_posix())
    return sorted(targets)


def load_manifest():
    manifest = {}
    for line in MANIFEST.read_text().splitlines():
        line = line.split("#", 1)[0].strip()
        if not line:
            continue
        path, _, cls = line.rpartition(" ")
        if not path or cls not in CLASSES:
            raise SystemExit(f"{MANIFEST}: malformed line: {line!r}")
        manifest[path] = cls
    return manifest


def main():
    targets = discover_targets()
    manifest = load_manifest()

    unclassified = sorted(set(targets) - set(manifest))
    if unclassified:
        raise SystemExit(
            "integration-test target(s) with no R3 classification in "
            f"{MANIFEST.relative_to(ROOT)}:\n" + "\n".join(unclassified)
        )

    stale = sorted(set(manifest) - set(targets))
    if stale:
        raise SystemExit(
            f"{MANIFEST.relative_to(ROOT)} lists target(s) that no longer "
            "exist — remove the stale entry:\n" + "\n".join(stale)
        )

    misplaced_multi = sorted(
        path for path, cls in manifest.items()
        if cls == "multi" and not path.startswith("xtask/tests/")
    )
    if misplaced_multi:
        raise SystemExit(
            "multi-crate application test(s) outside xtask/tests/ — move to "
            f"xtask/tests/, the sole approved home:\n" + "\n".join(misplaced_multi)
        )

    non_multi_in_xtask = sorted(
        path for path, cls in manifest.items()
        if cls != "multi" and path.startswith("xtask/tests/")
    )
    if non_multi_in_xtask:
        raise SystemExit(
            "xtask/tests/ target(s) not classified multi-crate application — "
            "xtask/tests/ is reserved for that class only:\n"
            + "\n".join(non_multi_in_xtask)
        )

    print(f"ok: {len(targets)} integration-test target(s) classified and placed correctly")


if __name__ == "__main__":
    main()
