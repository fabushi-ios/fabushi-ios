#!/usr/bin/env python3
"""Static & semantic opaque-box quality auditor for `mobile/ios/FabushiUITests/Parity/`.

Enforces:
1. Strict Import Whitelist: `ALLOWED_SWIFT_IMPORTS = {"XCTest", "Foundation"}` ONLY.
   Forbids `@testable`, `import Fabushi`, C-ABI bridge symbols, `dlopen`/`dlsym`, `SQLite3`, etc.
2. Anti-Facade & Anti-Skip Gate:
   - Zero `XCTSkip`, `XCTSkipIf`, `XCTSkipUnless`, `XCTExpectFailure`.
   - Zero constant tautologies (`XCTAssertTrue(true)`, `XCTAssertFalse(false)`,
     `XCTAssertNil(nil)`, `XCTAssertEqual(x, x)`).
   - Every test method must execute >= 1 genuine `XCTAssert*` / `XCTUnwrap` check AND
     interact with the SUT (`XCUIApplication` / `launch` / `waitForElement` / `element` / `peerClient`).
3. 4-Tier Naming & Per-Feature Quota (`F01..F44`):
   - Tier 1 (`Parity/Tier1/*.swift`): `testTier1_F<01..44>_*`, `>= 5` per feature when `min_tier1 >= 220`.
   - Tier 2 (`Parity/Tier2/*.swift`): `testTier2_F<01..44>_*`, `>= 5` per feature when `min_tier2 >= 220`.
   - Tier 3 (`Parity/Tier3/*.swift`): `testTier3_Pair<NN>_F<XX>_F<YY>_*`.
   - Tier 4 (`Parity/Tier4/*.swift`): `testTier4_Scenario<NN>_*`.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Any

ALLOWED_SWIFT_IMPORTS = {"XCTest", "Foundation"}

FORBIDDEN_SOURCE_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    ("@testable import", re.compile(r"@testable\b")),
    ("import Fabushi", re.compile(r"^\s*import\s+Fabushi\b", re.MULTILINE)),
    ("C-ABI host symbol", re.compile(r"\b(?:telegram_app_host_|mahayana_app_host)\w*")),
    ("dynamic symbol bypass", re.compile(r"\b(?:dlopen|dlsym)\s*\(")),
    ("direct SQLite3 import", re.compile(r"^\s*import\s+SQLite3\b", re.MULTILINE)),
]

FORBIDDEN_TEST_BODY_PATTERNS: list[tuple[str, re.Pattern[str]]] = [
    ("XCTSkip bypass", re.compile(r"\bXCTSkip(?:If|Unless)?\b")),
    ("XCTExpectFailure bypass", re.compile(r"\bXCTExpectFailure\b")),
    ("constant XCTAssertTrue(true)", re.compile(r"\bXCTAssertTrue\s*\(\s*true\b")),
    ("constant XCTAssert(true)", re.compile(r"\bXCTAssert\s*\(\s*true\b")),
    ("constant XCTAssertFalse(false)", re.compile(r"\bXCTAssertFalse\s*\(\s*false\b")),
    ("constant XCTAssertNil(nil)", re.compile(r"\bXCTAssertNil\s*\(\s*nil\b")),
]

EQUAL_TAUTOLOGY_RE = re.compile(
    r"\bXCTAssertEqual\s*\(\s*([A-Za-z0-9_.]+)\s*,\s*\1\s*[,)]"
)
IMPORT_RE = re.compile(r"^\s*import\s+(?:struct\s+|class\s+|enum\s+|func\s+)?([A-Za-z0-9_]+)", re.MULTILINE)
CLASS_DECL_RE = re.compile(
    r"\b(?:final\s+|open\s+|public\s+)*class\s+([A-Za-z0-9_]+)\s*:\s*([A-Za-z0-9_]+)"
)
FUNC_TEST_RE = re.compile(
    r"\bfunc\s+(test[A-Za-z0-9_]*)\s*\(\s*\)"
)

ASSERTION_TOKEN_RE = re.compile(
    r"\b(?:XCTAssert(?:True|False|Equal|NotEqual|Nil|NotNil|GreaterThan|GreaterThanOrEqual|LessThan|LessThanOrEqual|ThrowsError|NoThrow)?|XCTUnwrap)\s*\("
)
INTERACTION_TOKEN_RE = re.compile(
    r"\b(?:XCUIApplication|launchApp|launchAuthenticatedUser|waitForElement|waitForElementDisappearance|element|peerClient|client|tap|typeText|press|swipeLeft|swipeRight|swipeUp|swipeDown|pinch)\b"
)

TIER1_NAME_RE = re.compile(r"^testTier1_(F(?:0[1-9]|[1-3][0-9]|4[0-4]))_[A-Za-z0-9_]+$")
TIER2_NAME_RE = re.compile(r"^testTier2_(F(?:0[1-9]|[1-3][0-9]|4[0-4]))_[A-Za-z0-9_]+$")
TIER3_NAME_RE = re.compile(
    r"^testTier3_Pair(?:0[1-9]|[1-9][0-9])_(F(?:0[1-9]|[1-3][0-9]|4[0-4]))_(F(?:0[1-9]|[1-3][0-9]|4[0-4]))_[A-Za-z0-9_]+$"
)
TIER4_NAME_RE = re.compile(r"^testTier4_Scenario(?:0[1-9]|[1-9][0-9])_[A-Za-z0-9_]+$")

ALL_FEATURES = [f"F{i:02d}" for i in range(1, 45)]


def strip_comments_and_strings(source: str) -> str:
    """Replace Swift comments and string literals with spaces while preserving line structure."""
    out: list[str] = []
    i = 0
    n = len(source)
    while i < n:
        # Line comment
        if source[i : i + 2] == "//":
            while i < n and source[i] != "\n":
                out.append(" ")
                i += 1
            continue
        # Block comment (supports nesting)
        if source[i : i + 2] == "/*":
            depth = 1
            out.extend([" ", " "])
            i += 2
            while i < n and depth > 0:
                if source[i : i + 2] == "/*":
                    depth += 1
                    out.extend([" ", " "])
                    i += 2
                elif source[i : i + 2] == "*/":
                    depth -= 1
                    out.extend([" ", " "])
                    i += 2
                else:
                    out.append("\n" if source[i] == "\n" else " ")
                    i += 1
            continue
        # Multiline string literal
        if source[i : i + 3] == '"""':
            out.extend(['"', '"', '"'])
            i += 3
            while i < n:
                if source[i] == "\\" and i + 1 < n:
                    out.extend([" ", " "])
                    i += 2
                    continue
                if source[i : i + 3] == '"""':
                    out.extend(['"', '"', '"'])
                    i += 3
                    break
                out.append("\n" if source[i] == "\n" else " ")
                i += 1
            continue
        # Single-line string literal
        if source[i] == '"':
            out.append('"')
            i += 1
            while i < n and source[i] != '"':
                if source[i] == "\\" and i + 1 < n:
                    out.extend([" ", " "])
                    i += 2
                    continue
                out.append("\n" if source[i] == "\n" else " ")
                i += 1
            if i < n:
                out.append('"')
                i += 1
            continue
        out.append(source[i])
        i += 1
    return "".join(out)


def extract_test_methods(sanitized: str) -> list[tuple[str, str, int]]:
    """Return list of (method_name, body_text, line_number) for `func test...()` declarations."""
    methods: list[tuple[str, str, int]] = []
    for match in FUNC_TEST_RE.finditer(sanitized):
        name = match.group(1)
        line_no = sanitized.count("\n", 0, match.start()) + 1
        brace_idx = sanitized.find("{", match.end())
        if brace_idx < 0:
            methods.append((name, "", line_no))
            continue
        depth = 1
        pos = brace_idx + 1
        end_pos = len(sanitized)
        while pos < end_pos and depth > 0:
            ch = sanitized[pos]
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
            pos += 1
        body = sanitized[brace_idx + 1 : pos - 1]
        methods.append((name, body, line_no))
    return methods


def audit_parity_suite(
    parity_root: Path,
    min_tier1: int = 220,
    min_tier2: int = 0,
    min_tier3: int = 0,
    min_tier4: int = 0,
    min_total: int = 220,
) -> dict[str, Any]:
    errors: list[str] = []
    if not parity_root.is_dir():
        return {
            "ok": False,
            "errors": [f"Parity directory does not exist: {parity_root}"],
        }

    swift_files = sorted(parity_root.rglob("*.swift"))
    if not swift_files:
        return {
            "ok": False,
            "errors": [f"No Swift files found under {parity_root}"],
        }

    tier_counts = {"tier1": 0, "tier2": 0, "tier3": 0, "tier4": 0}
    tier1_by_feature = {f: 0 for f in ALL_FEATURES}
    tier2_by_feature = {f: 0 for f in ALL_FEATURES}
    tier3_features_touched: set[str] = set()
    seen_methods: dict[str, str] = {}
    test_index: list[dict[str, Any]] = []

    for swift_file in swift_files:
        rel_str = str(swift_file.relative_to(parity_root))
        raw = swift_file.read_text(encoding="utf-8")
        sanitized = strip_comments_and_strings(raw)

        # 1. Check imports
        for imp_match in IMPORT_RE.finditer(sanitized):
            mod_name = imp_match.group(1)
            if mod_name not in ALLOWED_SWIFT_IMPORTS:
                errors.append(
                    f"{rel_str}: forbidden import '{mod_name}' (allowed: {sorted(ALLOWED_SWIFT_IMPORTS)})"
                )

        # 2. Check forbidden patterns
        for label, pat in FORBIDDEN_SOURCE_PATTERNS:
            if pat.search(sanitized):
                errors.append(f"{rel_str}: forbidden {label} detected")

        parts = swift_file.relative_to(parity_root).parts
        top_folder = parts[0] if parts else ""
        class_match = CLASS_DECL_RE.search(sanitized)
        class_name = class_match.group(1) if class_match else swift_file.stem

        if top_folder == "Support":
            continue

        methods = extract_test_methods(sanitized)
        if not methods:
            errors.append(f"{rel_str}: test file contains 0 `func test...()` methods")
            continue

        for method_name, body, line_no in methods:
            loc = f"{rel_str}:{line_no} ({method_name})"
            if method_name in seen_methods:
                errors.append(
                    f"{loc}: duplicate test method name (already in {seen_methods[method_name]})"
                )
            seen_methods[method_name] = loc

            for label, pat in FORBIDDEN_TEST_BODY_PATTERNS:
                if pat.search(body):
                    errors.append(f"{loc}: forbidden {label}")

            taut = EQUAL_TAUTOLOGY_RE.search(body)
            if taut:
                errors.append(
                    f"{loc}: tautological XCTAssertEqual({taut.group(1)}, {taut.group(1)})"
                )

            assert_hits = len(ASSERTION_TOKEN_RE.findall(body))
            if assert_hits < 1:
                errors.append(f"{loc}: test method has 0 explicit XCTAssert*/XCTUnwrap calls")

            if not INTERACTION_TOKEN_RE.search(body):
                errors.append(
                    f"{loc}: test method has no SUT UI or LoopbackPeerClient interaction"
                )

            tier_key: str | None = None
            features_for_test: list[str] = []

            if top_folder == "Tier1":
                m = TIER1_NAME_RE.match(method_name)
                if not m:
                    errors.append(
                        f"{loc}: Tier1 method must match testTier1_F<01..44>_<Case>()"
                    )
                else:
                    feat = m.group(1)
                    tier_counts["tier1"] += 1
                    tier1_by_feature[feat] += 1
                    tier_key = "1"
                    features_for_test = [feat]
            elif top_folder == "Tier2":
                m = TIER2_NAME_RE.match(method_name)
                if not m:
                    errors.append(
                        f"{loc}: Tier2 method must match testTier2_F<01..44>_<BoundaryOrNegative>()"
                    )
                else:
                    feat = m.group(1)
                    tier_counts["tier2"] += 1
                    tier2_by_feature[feat] += 1
                    tier_key = "2"
                    features_for_test = [feat]
            elif top_folder == "Tier3":
                m = TIER3_NAME_RE.match(method_name)
                if not m:
                    errors.append(
                        f"{loc}: Tier3 method must match testTier3_Pair<NN>_F<XX>_F<YY>_<Interaction>()"
                    )
                else:
                    f1, f2 = m.group(1), m.group(2)
                    tier_counts["tier3"] += 1
                    tier3_features_touched.update([f1, f2])
                    tier_key = "3"
                    features_for_test = [f1, f2]
            elif top_folder == "Tier4":
                m = TIER4_NAME_RE.match(method_name)
                if not m:
                    errors.append(
                        f"{loc}: Tier4 method must match testTier4_Scenario<NN>_<Journey>()"
                    )
                else:
                    tier_counts["tier4"] += 1
                    tier_key = "4"
            else:
                errors.append(f"{loc}: unexpected subfolder '{top_folder}' under Parity/")

            if tier_key is not None:
                test_index.append(
                    {
                        "tier": tier_key,
                        "class": class_name,
                        "method": method_name,
                        "features": features_for_test,
                        "file": rel_str,
                        "assertions": assert_hits,
                    }
                )

    total_tests = sum(tier_counts.values())

    if tier_counts["tier1"] < min_tier1:
        errors.append(f"Tier 1 count {tier_counts['tier1']} < required minimum {min_tier1}")
    if min_tier1 >= 220:
        for feat in ALL_FEATURES:
            if tier1_by_feature[feat] < 5:
                errors.append(
                    f"Tier 1 feature {feat} has {tier1_by_feature[feat]} tests (requires >= 5)"
                )

    if tier_counts["tier2"] < min_tier2:
        errors.append(f"Tier 2 count {tier_counts['tier2']} < required minimum {min_tier2}")
    if min_tier2 >= 220:
        for feat in ALL_FEATURES:
            if tier2_by_feature[feat] < 5:
                errors.append(
                    f"Tier 2 feature {feat} has {tier2_by_feature[feat]} tests (requires >= 5)"
                )

    if tier_counts["tier3"] < min_tier3:
        errors.append(f"Tier 3 count {tier_counts['tier3']} < required minimum {min_tier3}")
    if min_tier3 >= 44:
        missing_t3 = sorted(set(ALL_FEATURES) - tier3_features_touched)
        if missing_t3:
            errors.append(
                f"Tier 3 pairwise matrix missing features: {', '.join(missing_t3)}"
            )

    if tier_counts["tier4"] < min_tier4:
        errors.append(f"Tier 4 count {tier_counts['tier4']} < required minimum {min_tier4}")

    if total_tests < min_total:
        errors.append(f"Total parity tests {total_tests} < required minimum {min_total}")

    return {
        "ok": len(errors) == 0,
        "allowed_imports": sorted(ALLOWED_SWIFT_IMPORTS),
        "swift_files_scanned": len(swift_files),
        "tier_counts": tier_counts,
        "total_tests": total_tests,
        "tier1_by_feature": tier1_by_feature,
        "tier2_by_feature": tier2_by_feature,
        "tier3_features_covered": sorted(tier3_features_touched),
        "thresholds": {
            "min_tier1": min_tier1,
            "min_tier2": min_tier2,
            "min_tier3": min_tier3,
            "min_tier4": min_tier4,
            "min_total": min_total,
        },
        "test_index": test_index,
        "errors": errors,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Audit Opaque-Box Parity XCUITest Suite")
    parser.add_argument(
        "--parity-root",
        default="mobile/ios/FabushiUITests/Parity",
        help="Path to Parity XCTest directory",
    )
    parser.add_argument("--min-tier1", type=int, default=220)
    parser.add_argument("--min-tier2", type=int, default=0)
    parser.add_argument("--min-tier3", type=int, default=0)
    parser.add_argument("--min-tier4", type=int, default=0)
    parser.add_argument("--min-total", type=int, default=220)
    parser.add_argument("--json-out", default=None)
    args = parser.parse_args(argv)

    report = audit_parity_suite(
        parity_root=Path(args.parity_root).resolve(),
        min_tier1=args.min_tier1,
        min_tier2=args.min_tier2,
        min_tier3=args.min_tier3,
        min_tier4=args.min_tier4,
        min_total=args.min_total,
    )

    summary_report = {k: v for k, v in report.items() if k != "test_index"}
    summary_report["indexed_tests_count"] = len(report.get("test_index", []))

    if args.json_out:
        out_path = Path(args.json_out).resolve()
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(summary_report, indent=2) + "\n", encoding="utf-8")

    print(json.dumps(summary_report, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
