#!/usr/bin/env python3
from __future__ import annotations

import json
import re
import sys
from pathlib import Path


INLINE_TEST_PATTERN = re.compile(
    r"(?m)^\s*(?:#\[[^\]]+\]\s*)*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+tests\s*\{"
)
BLOCK_COMMENT_PATTERN = re.compile(r"/\*.*?\*/", re.DOTALL)
LINE_COMMENT_PATTERN = re.compile(r"//.*$", re.MULTILINE)
REQUIRED_BASELINE_KEYS = (
    "production_soft_limit",
    "production_hard_limit",
    "test_soft_limit",
    "test_hard_limit",
    "legacy_production_soft_size_ceilings",
    "legacy_production_size_ceilings",
    "legacy_test_soft_size_ceilings",
    "legacy_test_size_ceilings",
    "legacy_inline_test_files",
)


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def relative_path(root: Path, path: Path) -> str:
    return path.relative_to(root).as_posix()


def is_test_file(path: str) -> bool:
    rel = Path(path)
    return "tests" in rel.parts or rel.name == "tests.rs"


def is_production_file(path: str) -> bool:
    rel = Path(path)
    return "src" in rel.parts and not is_test_file(path)


def line_count(path: Path) -> int:
    with path.open("r", encoding="utf-8") as handle:
        return sum(1 for _ in handle)


def load_baseline(path: Path) -> dict[str, object]:
    baseline = json.loads(path.read_text(encoding="utf-8"))
    missing = [key for key in REQUIRED_BASELINE_KEYS if key not in baseline]
    if missing:
        missing_keys = ", ".join(sorted(missing))
        raise ValueError(
            f"baseline file {path.as_posix()} is missing required keys: {missing_keys}"
        )
    return baseline


def strip_comments(text: str) -> str:
    return LINE_COMMENT_PATTERN.sub("", BLOCK_COMMENT_PATTERN.sub("", text))


def contains_inline_test_module(text: str) -> bool:
    return INLINE_TEST_PATTERN.search(strip_comments(text)) is not None


def summarize_group(title: str, entries: list[str]) -> None:
    if not entries:
        return
    print(f"[modularization-gate] {title} ({len(entries)})")
    for entry in entries:
        print(f"  - {entry}")


def main() -> int:
    root = repo_root()
    baseline_path = root / "scripts" / "modularization_baseline.json"
    policy_path = root / "docs" / "ARCHITECTURE_CODEBASE_MODULARIZATION_POLICY.md"

    try:
        baseline = load_baseline(baseline_path)
    except ValueError as exc:
        print(f"[modularization-gate] error: {exc}", file=sys.stderr)
        return 1

    prod_soft = int(baseline["production_soft_limit"])
    prod_hard = int(baseline["production_hard_limit"])
    test_soft = int(baseline["test_soft_limit"])
    test_hard = int(baseline["test_hard_limit"])

    legacy_prod = {
        path: int(limit)
        for path, limit in baseline["legacy_production_size_ceilings"].items()
    }
    legacy_prod_soft = {
        path: int(limit)
        for path, limit in baseline["legacy_production_soft_size_ceilings"].items()
    }
    legacy_test = {
        path: int(limit)
        for path, limit in baseline["legacy_test_size_ceilings"].items()
    }
    legacy_test_soft = {
        path: int(limit)
        for path, limit in baseline["legacy_test_soft_size_ceilings"].items()
    }
    legacy_inline_tests = set(baseline["legacy_inline_test_files"])

    rust_files = sorted((root / "crates").rglob("*.rs"))
    prod_files = []
    test_files = []
    for path in rust_files:
        rel = relative_path(root, path)
        if is_production_file(rel):
            prod_files.append((rel, path))
        elif is_test_file(rel):
            test_files.append((rel, path))

    warnings: list[str] = []
    errors: list[str] = []
    prod_soft_warnings: list[str] = []
    test_soft_warnings: list[str] = []
    legacy_inline_warnings: list[str] = []

    for rel, path in prod_files:
        count = line_count(path)
        if count > prod_hard:
            baseline_limit = legacy_prod.get(rel)
            if baseline_limit is None:
                errors.append(
                    f"new production hard-limit violation: {rel} has {count} lines; "
                    f"hard limit is {prod_hard}"
                )
            elif count > baseline_limit:
                errors.append(
                    f"legacy production file grew past baseline: {rel} has {count} lines; "
                    f"baseline is {baseline_limit}"
                )
        elif count > prod_soft:
            baseline_limit = legacy_prod_soft.get(rel)
            if baseline_limit is None:
                errors.append(
                    f"new production soft-limit violation: {rel} has {count} lines; "
                    f"soft limit is {prod_soft}"
                )
            elif count > baseline_limit:
                errors.append(
                    f"legacy production soft-limit file grew past baseline: {rel} has {count} lines; "
                    f"baseline is {baseline_limit}"
                )
            else:
                prod_soft_warnings.append(
                    f"{rel} has {count} lines; soft limit is {prod_soft}"
                )

        text = path.read_text(encoding="utf-8")
        if contains_inline_test_module(text):
            if rel not in legacy_inline_tests:
                errors.append(
                    f"new inline test module in production file: {rel} contains `mod tests {{ ... }}`"
                )
            else:
                legacy_inline_warnings.append(rel)

    for rel, path in test_files:
        count = line_count(path)
        if count > test_hard:
            baseline_limit = legacy_test.get(rel)
            if baseline_limit is None:
                errors.append(
                    f"new test hard-limit violation: {rel} has {count} lines; "
                    f"hard limit is {test_hard}"
                )
            elif count > baseline_limit:
                errors.append(
                    f"legacy test file grew past baseline: {rel} has {count} lines; "
                    f"baseline is {baseline_limit}"
                )
        elif count > test_soft:
            baseline_limit = legacy_test_soft.get(rel)
            if baseline_limit is None:
                errors.append(
                    f"new test soft-limit violation: {rel} has {count} lines; "
                    f"soft limit is {test_soft}"
                )
            elif count > baseline_limit:
                errors.append(
                    f"legacy test soft-limit file grew past baseline: {rel} has {count} lines; "
                    f"baseline is {baseline_limit}"
                )
            else:
                test_soft_warnings.append(
                    f"{rel} has {count} lines; soft limit is {test_soft}"
                )

    warnings.extend(prod_soft_warnings)
    warnings.extend(test_soft_warnings)
    warnings.extend(
        f"legacy inline test module remains: {rel}" for rel in legacy_inline_warnings
    )

    print(
        "[modularization-gate] "
        f"policy={relative_path(root, policy_path)} "
        f"baseline={relative_path(root, baseline_path)}"
    )
    print(
        "[modularization-gate] "
        f"scanned {len(rust_files)} Rust files "
        f"({len(prod_files)} production, {len(test_files)} test)"
    )
    print(
        "[modularization-gate] "
        f"limits: production soft/hard={prod_soft}/{prod_hard}, "
        f"test soft/hard={test_soft}/{test_hard}"
    )

    summarize_group("production soft-limit warnings", prod_soft_warnings)
    summarize_group("test soft-limit warnings", test_soft_warnings)
    summarize_group("legacy inline test modules", legacy_inline_warnings)

    if errors:
        summarize_group("errors", errors)
        print("[modularization-gate] failed")
        return 1

    if warnings:
        print(
            "[modularization-gate] passed with warnings; "
            "see policy doc for stage-0 constraints"
        )
    else:
        print("[modularization-gate] passed with no warnings")
    return 0


if __name__ == "__main__":
    sys.exit(main())
