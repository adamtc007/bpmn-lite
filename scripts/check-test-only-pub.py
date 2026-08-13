#!/usr/bin/env python3
"""H6 (EOP-PLAN-CRATE-HYGIENE-001): reject new `#[cfg(test)] pub ...` items.

An item that is both `#[cfg(test)]`-gated and bare `pub` (not `pub(crate)`,
`pub(super)`, `pub(in ...)`) is a smell H2 spent effort removing (`test_lock`,
`utterance_engine::metrics`): it is never reachable from a normal downstream
build (cfg(test) is only set for the crate's own `--lib` unit-test compile),
so bare `pub` buys nothing a narrower visibility wouldn't, while reading as
though it were real API surface. New instances must be justified in-line and
listed in the exception file below; this check fails closed on anything else.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EXCEPTIONS_FILE = ROOT / "scripts/baselines/cfg-test-pub-exceptions.txt"
EXCLUDE_DIR_PARTS = {"target", ".git"}

CFG_TEST_RE = re.compile(r"#!?\[\s*cfg\([^)]*\btest\b[^)]*\)\s*\]")
ATTRIBUTE_RE = re.compile(r"^\s*#!?\[")
BARE_PUB_RE = re.compile(r"^\s*pub\s+(mod|fn|struct|enum|trait|const|static|type|use)\b")


def rust_files():
    for path in ROOT.rglob("*.rs"):
        if any(part in EXCLUDE_DIR_PARTS for part in path.parts):
            continue
        yield path


def find_cfg_test_pub_items():
    hits = []
    for path in rust_files():
        lines = path.read_text(errors="replace").splitlines()
        for index, line in enumerate(lines):
            if not CFG_TEST_RE.search(line):
                continue
            lookahead = index + 1
            while lookahead < len(lines) and ATTRIBUTE_RE.match(lines[lookahead]):
                lookahead += 1
            if lookahead < len(lines) and BARE_PUB_RE.match(lines[lookahead]):
                hits.append(f"{path.relative_to(ROOT)}:{lookahead + 1}")
    return sorted(hits)


def load_exceptions():
    if not EXCEPTIONS_FILE.exists():
        return set()
    return {
        line.split("#", 1)[0].strip()
        for line in EXCEPTIONS_FILE.read_text().splitlines()
        if line.split("#", 1)[0].strip()
    }


def main():
    hits = find_cfg_test_pub_items()
    exceptions = load_exceptions()
    unapproved = [hit for hit in hits if hit not in exceptions]
    stale = sorted(exceptions - set(hits))
    if unapproved:
        raise SystemExit(
            "new #[cfg(test)] pub item(s) without a reviewed exception — narrow "
            "to pub(crate)/pub(super), or add a justified line to "
            f"{EXCEPTIONS_FILE.relative_to(ROOT)}:\n" + "\n".join(unapproved)
        )
    if stale:
        raise SystemExit(
            f"{EXCEPTIONS_FILE.relative_to(ROOT)} lists location(s) that no "
            "longer exist or are no longer bare-pub under cfg(test) — remove "
            "the stale entry:\n" + "\n".join(stale)
        )
    print(f"ok: {len(hits)} #[cfg(test)] pub item(s), all reviewed")


if __name__ == "__main__":
    main()
