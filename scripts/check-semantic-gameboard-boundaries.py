#!/usr/bin/env python3
"""Standing Phase 0 public-API, visibility and dependency-direction gate."""

import hashlib
import json
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASELINE = ROOT / "scripts/baselines/semantic-gameboard-public-api-v1.json"
FIXTURES = ROOT / "scripts/fixtures/gameboard_api"
GENERATED_BASELINES = ROOT / "docs/generated/public-api-baselines"


def run(command, *, cwd=ROOT, check=True):
    return subprocess.run(
        command,
        cwd=cwd,
        check=check,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def check_public_api(baseline):
    actual = []
    for surface in baseline["surfaces"]:
        command = ["cargo", "public-api", "-p", surface["package"], "-sss"]
        if surface["features"]:
            command.extend(["-F", surface["features"]])
        output = run(command).stdout.encode()
        observed = {
            "package": surface["package"],
            "features": surface["features"],
            "items": len(output.splitlines()),
            "sha256": hashlib.sha256(output).hexdigest(),
        }
        actual.append(observed)
        if observed != {key: surface[key] for key in observed}:
            raise SystemExit(
                "public API drift; name the consumer, owning facade, stability contract "
                f"and reason before updating the baseline:\nexpected={surface}\nactual={observed}"
            )
    return actual


def check_modules_and_globs(baseline):
    for relative, approved in baseline["approved_pub_modules"].items():
        source = (ROOT / relative).read_text()
        observed = sorted(re.findall(r"^pub mod ([A-Za-z0-9_]+);", source, re.MULTILINE))
        if observed != sorted(approved):
            raise SystemExit(f"unapproved pub mod drift in {relative}: {observed}")
    for source_path in [ROOT / "utterance-engine/src", ROOT / "designer-graph/src"]:
        for rust in source_path.rglob("*.rs"):
            if re.search(r"pub\s+use\s+[^;]*::\s*\*\s*;", rust.read_text()):
                raise SystemExit(f"glob public re-export refused: {rust.relative_to(ROOT)}")


def check_workspace_public_api_baselines():
    """H6 (EOP-PLAN-CRATE-HYGIENE-001): extends the narrow surface check above
    workspace-wide. Every library package's committed full-text baseline
    (docs/generated/public-api-baselines/<crate>.txt, produced by the exact
    `cargo public-api -p <crate> -sss` invocation recorded in that file's own
    `# command:` header) must match the crate's actual current surface. This
    is the same gate mechanism as check_public_api above, not a second one —
    it reads a different, wider baseline set."""
    checked = []
    for baseline_file in sorted(GENERATED_BASELINES.glob("*.txt")):
        if baseline_file.name == "BASELINE_REVISION.txt":
            continue
        lines = baseline_file.read_text().splitlines()
        header = {}
        body_start = len(lines)
        for index, line in enumerate(lines):
            if line.startswith("# "):
                key, _, value = line[2:].partition(":")
                header[key.strip()] = value.strip()
            elif line == "":
                body_start = index + 1
                break
        package = header.get("package")
        if not package:
            raise SystemExit(f"{baseline_file}: missing '# package:' header")
        expected = "\n".join(lines[body_start:]).strip("\n")
        actual = run(["cargo", "public-api", "-p", package, "-sss"]).stdout.strip("\n")
        if actual != expected:
            raise SystemExit(
                f"public API drift in {package} (baseline: "
                f"{baseline_file.relative_to(ROOT)}) — update the baseline only "
                "after naming the consumer, owning facade, stability contract and "
                "reason in a tranche receipt:\n"
                f"--- expected ({baseline_file.name}) ---\n{expected}\n"
                f"--- actual (cargo public-api -p {package} -sss) ---\n{actual}"
            )
        checked.append(package)
    return checked


def check_baseline_coverage(checked_packages):
    """A library package with no committed baseline is unprotected by the
    ratchet above — fail closed rather than silently skip it."""
    metadata = json.loads(
        run(["cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"]).stdout
    )
    lib_packages = {
        package["name"]
        for package in metadata["packages"]
        if any("lib" in target["kind"] for target in package["targets"])
    }
    missing = lib_packages - set(checked_packages)
    if missing:
        raise SystemExit(
            "library package(s) with no committed public-API baseline in "
            f"docs/generated/public-api-baselines/: {sorted(missing)}"
        )


def check_dependencies():
    metadata = json.loads(
        run(["cargo", "metadata", "--locked", "--format-version", "1", "--no-deps"]).stdout
    )
    packages = {package["name"]: package for package in metadata["packages"]}

    def normal_dependencies(package):
        return {
            dependency["name"]
            for dependency in packages[package]["dependencies"]
            if dependency.get("kind") in (None, "normal")
        }

    forbidden = {"bpmn-lite-server-designer", "xtask", "utterance-engine-fuzz"}
    for capability in ("utterance-engine", "designer-graph"):
        wrong = normal_dependencies(capability) & forbidden
        if wrong:
            raise SystemExit(f"{capability} depends outward on {sorted(wrong)}")
    xtask_wrong = normal_dependencies("xtask") & {
        "utterance-engine",
        "designer-graph",
        "bpmn-lite-server-designer",
    }
    if xtask_wrong:
        raise SystemExit(f"xtask became a privileged semantic consumer: {sorted(xtask_wrong)}")
    server_dependencies = normal_dependencies("bpmn-lite-server-designer")
    if not {"utterance-engine", "designer-graph"} <= server_dependencies:
        raise SystemExit("application composition-root dependency closure is incomplete")


def compile_fixture(source_name, should_pass):
    with tempfile.TemporaryDirectory(prefix="gameboard-api-fixture-") as directory:
        root = Path(directory)
        (root / "src").mkdir()
        shutil.copyfile(FIXTURES / source_name, root / "src/main.rs")
        (root / "Cargo.toml").write_text(
            "[package]\nname = \"gameboard-api-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n"
            "[workspace]\n\n[dependencies]\n"
            f"utterance-engine = {{ path = {json.dumps(str(ROOT / 'utterance-engine'))} }}\n"
        )
        result = run(["cargo", "check", "--quiet"], cwd=root, check=False)
        if should_pass and result.returncode != 0:
            raise SystemExit(f"facade consumer failed to compile:\n{result.stderr}")
        if not should_pass and result.returncode == 0:
            raise SystemExit(f"compile-fail fixture unexpectedly passed: {source_name}")
        if not should_pass and "private" not in result.stderr:
            raise SystemExit(
                f"compile-fail fixture failed for the wrong reason ({source_name}):\n{result.stderr}"
            )


def main():
    baseline = json.loads(BASELINE.read_text())
    actual = check_public_api(baseline)
    check_modules_and_globs(baseline)
    checked_packages = check_workspace_public_api_baselines()
    check_baseline_coverage(checked_packages)
    check_dependencies()
    compile_fixture("facade_consumer.rs", True)
    compile_fixture("internal_module_import.rs", False)
    compile_fixture("unchecked_constructor.rs", False)
    print(json.dumps({"status": "pass", "surfaces": actual}, sort_keys=True))


if __name__ == "__main__":
    main()
