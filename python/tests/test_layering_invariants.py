"""Independent Python layering invariant scanner, shared fixture catalog, and cross-runtime parity tests.

CLI mode (stdlib-only, no pytest required):
    python python/tests/test_layering_invariants.py --emit-fixtures
"""

from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from typing import Any

try:
    import pytest
except ModuleNotFoundError:  # pragma: no cover - stdlib-only CLI execution
    pytest = None  # type: ignore[assignment]


REPO_ROOT = Path(__file__).resolve().parents[2]
GATE_SCRIPT = REPO_ROOT / "scripts" / "check-layering.ps1"

# Dynamic token assembly avoids AC03 whole-repo self-triggers and raw literal leakage.
_COUNTRY_CHAR_1 = chr(67)
_COUNTRY_CHAR_2 = chr(78)
_COUNTRY_CODE = _COUNTRY_CHAR_1 + _COUNTRY_CHAR_2
_LEGACY_NOTIONAL_CONST = "".join(("CGB_", "FUTURES_", "CONTRACT_", "NOTIONAL"))

_ISSUER_CDB = "".join((chr(67), chr(68), chr(66)))
_ISSUER_EIBC = "".join((chr(69), chr(73), chr(66), chr(67)))
_ISSUER_ADBC = "".join((chr(65), chr(68), chr(66), chr(67)))
_ISSUER_MOF = "".join((chr(77), chr(79), chr(70)))
_ISSUER_TOKENS = (_ISSUER_CDB, _ISSUER_EIBC, _ISSUER_ADBC, _ISSUER_MOF)

_PROD_TS = "".join((chr(84), chr(83)))
_PROD_TF = "".join((chr(84), chr(70)))
_PROD_TL = "".join((chr(84), chr(76)))
_PROD_T = chr(84)
_FUTURES_PRODUCT_TOKENS = (_PROD_TS, _PROD_TF, _PROD_TL, _PROD_T)

_AC03_EXTENSIONS = frozenset(
    {
        ".c",
        ".cc",
        ".cpp",
        ".cxx",
        ".h",
        ".hh",
        ".hpp",
        ".hxx",
        ".rs",
        ".proto",
        ".py",
        ".ps1",
        ".psm1",
        ".ts",
        ".tsx",
        ".js",
        ".jsx",
        ".json",
        ".yaml",
        ".yml",
        ".toml",
        ".sql",
    }
)

_EXCLUDED_DIR_NAMES = frozenset(
    {
        ".git",
        "target",
        "build",
        "node_modules",
        "vendor",
        "generated",
        "fixtures",
        ".venv",
        "venv",
        "__pycache__",
        "dist",
        "coverage",
    }
)

_EXCLUDED_DIR_RE = re.compile(
    r"(?i)(^|/)(\.git|target|build|node_modules|vendor|generated|fixtures|\.venv|venv|__pycache__|dist|coverage)(/|$)"
)
_EXCLUDED_LOCKFILE_RE = re.compile(
    r"(?i)(^|/)(Cargo\.lock|pnpm-lock\.yaml|uv\.lock)$"
)

_DOMAIN_EXTENSIONS = frozenset({".rs", ".py", ".ts", ".tsx"})
_CPP_EXTENSIONS = frozenset({".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".hxx"})
_PHASE2C_CPP_EXTENSIONS = frozenset({".cpp", ".hpp", ".h"})

_BP1_PRODUCTION_ROOTS = (
    "cpp/fixed-income-kernel/include",
    "cpp/fixed-income-kernel/src",
    "crates/ficant-kernel-sys/src",
    "crates/ficant-fixed-income-native/src",
    "crates/ficant-domain/src",
    "crates/ficant-application/src",
    "crates/ficant-api/src",
    "crates/ficant-blotter-store/src",
)
_BP1_PRODUCTION_EXTENSIONS = _CPP_EXTENSIONS | frozenset({".rs"})

_PURE_LAYER_ROOT_CRATES = ("ficant-domain", "ficant-application")
_FORBIDDEN_PURE_LAYER_CRATES = frozenset(
    {
        "rusqlite",
        "libsqlite3-sys",
        "tokio",
        "tokio-stream",
        "tonic",
        "tonic-prost",
        "tonic-web",
        "prost",
        "prost-types",
        "hyper",
        "http",
        "serde_json",
        "ficant-blotter-store",
        "ficant-contracts",
        "ficant-api",
    }
)


@dataclass(frozen=True)
class Violation:
    gate: str
    path: str
    line: int
    category: str
    text: str


@dataclass(frozen=True)
class ScanReport:
    exit_code: int
    allowlist_count: int
    violations: tuple[Violation, ...]
    error: str | None = None


def _rel_posix(repo_root: Path, path: Path) -> str:
    return path.relative_to(repo_root).as_posix()


def _is_excluded_path(repo_root: Path, path: Path) -> bool:
    rel = _rel_posix(repo_root, path)
    return bool(_EXCLUDED_DIR_RE.search(rel) or _EXCLUDED_LOCKFILE_RE.search(rel))


def _iter_files(
    repo_root: Path,
    scan_root: Path,
    extensions: frozenset[str],
    apply_exclusions: bool = True,
) -> list[Path]:
    if not scan_root.is_dir():
        return []
    collected: list[Path] = []
    for dirpath, dirnames, filenames in os.walk(scan_root):
        if apply_exclusions:
            dirnames[:] = [
                d for d in dirnames if d.lower() not in _EXCLUDED_DIR_NAMES
            ]
        base = Path(dirpath)
        for name in filenames:
            ext = Path(name).suffix.lower()
            if ext not in extensions:
                continue
            full = base / name
            if apply_exclusions and _is_excluded_path(repo_root, full):
                continue
            collected.append(full)
    collected.sort()
    return collected


def _read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def _line_snippet_at_offset(content: str, offset: int) -> tuple[int, str]:
    line_number = content.count("\n", 0, offset) + 1
    line_start = content.rfind("\n", 0, offset)
    line_start = 0 if line_start < 0 else line_start + 1
    line_end = content.find("\n", offset)
    if line_end < 0:
        line_end = len(content)
    return line_number, content[line_start:line_end].strip()


def _read_layering_allowlist(repo_root: Path) -> list[Any]:
    allowlist_path = repo_root / "scripts" / "layering-allowlist.json"
    if not allowlist_path.is_file():
        raise ValueError(f"Layering allowlist is missing: {allowlist_path}")
    raw = _read_text(allowlist_path).strip()
    if not raw or not raw.startswith("["):
        raise ValueError("Layering allowlist must be a JSON array.")
    decoded = json.loads(raw)
    if not isinstance(decoded, list):
        raise ValueError("Layering allowlist must be a JSON array.")
    if len(decoded) != 0:
        raise ValueError(
            "Layering allowlist must be empty after the R2 ratchet; entries may only be removed."
        )
    return decoded


def _compile_ac03_patterns() -> tuple[re.Pattern[str], ...]:
    cc = _COUNTRY_CODE
    c1 = _COUNTRY_CHAR_1
    c2 = _COUNTRY_CHAR_2
    raw_patterns = (
        rf"""\bmarket(?:\.[A-Za-z_][A-Za-z0-9_]*)?\s*(?:==|===|!=|!==)\s*["']{cc}["']""",
        rf"""["']{cc}["']\s*(?:==|===|!=|!==)\s*\bmarket(?:\.[A-Za-z_][A-Za-z0-9_]*)?""",
        rf"""\bmarket(?:\.[A-Za-z_][A-Za-z0-9_]*)?\s*(?:==|===|!=|!==)\s*(?:Market::)?{cc}\b""",
        rf"""\b(?:Market::)?{cc}\s*=>""",
        rf"""["']{cc}["']\s*=>""",
        rf"""\bcase\s+["']{cc}["']""",
        rf"""["']{cc}["']\s*:""",
        rf"""\(\s*["']{cc}["']\s*,""",
        rf"""["']{c1}["']\s*\+\s*["']{c2}["']""",
    )
    return tuple(re.compile(p, re.IGNORECASE) for p in raw_patterns)


_AC03_PATTERNS = _compile_ac03_patterns()

_AC01_RULES: tuple[tuple[str, re.Pattern[str]], ...] = (
    (
        "term-months-table",
        re.compile(r"\b(?:original_term_months|residual_term_bounds)\b", re.IGNORECASE),
    ),
    (
        "residual-term-range",
        re.compile(
            r"\(\s*(?:18|48|78|300)\s*,\s*(?:Some\(\s*(?:27|63)\s*\)|None)\s*\)",
            re.IGNORECASE,
        ),
    ),
    (
        "tax-rate",
        re.compile(
            r"\b(?:tax|vat)[A-Za-z_]*rate\b\s*(?:=|:)\s*[-+]?\d+(?:\.\d+)?",
            re.IGNORECASE,
        ),
    ),
    (
        "conversion-factor",
        re.compile(
            r"\bconversion[_ -]*factor\b\s*(?:=|:)\s*[-+]?\d+(?:\.\d+)?",
            re.IGNORECASE,
        ),
    ),
)

_PHASE2C_RULES: tuple[tuple[str, re.Pattern[str]], ...] = (
    (
        "eligibility-bounds",
        re.compile(
            r"\b(?:original_term_max_months|residual_min_months|residual_max_months)\b\s*(?:=|:)\s*\d+",
            re.IGNORECASE,
        ),
    ),
    (
        "delivery-months",
        re.compile(
            r"\bdelivery_months\b[^\r\n]*[=:\{][^\r\n]*\b(?:3|6|9|12)\b",
            re.IGNORECASE,
        ),
    ),
    (
        "standard-coupon",
        re.compile(
            r"\b(?:nominal_coupon|standard_coupon)\b\s*(?:=|:)\s*(?:\d+\.\d+|\.\d+)",
            re.IGNORECASE,
        ),
    ),
    (
        "face-quote-basis",
        re.compile(
            r"\b(?:face_quote_basis|face_per_hundred)\b\s*(?:=|:)\s*100(?:\.0+)?\b",
            re.IGNORECASE,
        ),
    ),
    (
        "rounding-scale",
        re.compile(
            r"\b(?:conversion_factor_rounding_places|accrued_interest_rounding_places)\b\s*(?:=|:)\s*\d+",
            re.IGNORECASE,
        ),
    ),
    (
        "annual-day-basis",
        re.compile(r"\bannual_day_basis\b\s*(?:=|:)\s*\d+", re.IGNORECASE),
    ),
)

_R3A_PATTERN = re.compile(
    r"\b(?:annual_)?(?:funding|financing)[A-Za-z_]*rate\b\s*(?:=|:)\s*[-+]?(?:\d+\.\d+|\.\d+|\d+)",
    re.IGNORECASE,
)

_R3B_CODE_PATTERN = re.compile(
    r"\b(?:coupon_)?(?:tax|vat)[A-Za-z_]*rate\b\s*(?:=|:)\s*[-+]?(?:\d+\.\d+|\.\d+|\d+)",
    re.IGNORECASE,
)
_R3B_PROTO_PATTERN = re.compile(
    r"\b(?:double|float)\s+(?:coupon_)?(?:tax|vat)[A-Za-z_]*rate\b\s*=\s*[-+]?(?:\d+\.\d+|\.\d+)",
    re.IGNORECASE,
)


def _compile_bp1_rules() -> tuple[tuple[str, re.Pattern[str]], ...]:
    ident_pattern = (
        r"\b(?:"
        r"[A-Za-z0-9_]*contract_notional|"
        r"[A-Za-z0-9_]*face_quote(?:_basis)?|"
        r"[A-Za-z0-9_]*quote_face(?:_basis)?|"
        r"[A-Za-z0-9_]*face_per_hundred|"
        r"[A-Za-z0-9_]*contract_size(?:_in_quote_units)?"
        r")\b"
    )
    num_pattern = (
        r"[-+]?(?:\d[\d_']*(?:\.\d[\d_']*)?|\.\d[\d_']*)"
        r"(?:[eE][-+]?\d[\d_']*)?"
        r"(?:_?(?:[iu](?:8|16|32|64|128|size)|f(?:32|64)|[uUlLfF]{1,3}))?"
    )
    inner_val_pattern = (
        rf"(?:(?:[A-Za-z_][A-Za-z0-9_]*::)*FixedDecimal\s*::\s*from_scaled\s*\(\s*{num_pattern}\s*\)|{num_pattern})"
    )
    rhs_val_pattern = (
        rf"(?:(?:[A-Za-z_][A-Za-z0-9_]*::)*Some\s*\(\s*{inner_val_pattern}\s*\)|\{{\s*{inner_val_pattern}\s*\}}|{inner_val_pattern})"
    )
    rust_type_pattern = (
        r"(?:&(?:'[A-Za-z_][A-Za-z0-9_]*\s+)?(?:mut\s+)?)?"
        r"(?:[A-Za-z_][A-Za-z0-9_]*::)*[A-Za-z_][A-Za-z0-9_]*"
        r"(?:\s*<\s*[A-Za-z0-9_:<>,&'\s]+\s*>)?"
    )
    hardcoded_numeric_pattern = (
        ident_pattern
        + r"(?:"
        + rf"(?:\s*(?<!:):(?!:)\s*{rust_type_pattern})?\s*(?<![=!<>])=(?![=>])\s*{rhs_val_pattern}"
        + rf"|\s*(?<!:):(?!:)\s*{rhs_val_pattern}"
        + rf"|\s*\{{\s*{inner_val_pattern}\s*\}}"
        + rf"|\s*\(\s*{inner_val_pattern}\s*\)"
        + r")"
    )

    issuer_group = "(?:" + "|".join(_ISSUER_TOKENS) + ")"
    issuer_enum = (
        r"(?<![A-Za-z0-9_])(?:[A-Za-z_][A-Za-z0-9_]*::)*(?:Bond)?Issuer::"
        + issuer_group
        + r"\b"
    )
    product_group = "(?:" + "|".join(_FUTURES_PRODUCT_TOKENS) + ")"

    return (
        (
            "bp1-legacy-notional-constant",
            re.compile(rf"\b{_LEGACY_NOTIONAL_CONST}\b", re.IGNORECASE),
        ),
        (
            "bp1-hardcoded-futures-rule-value",
            re.compile(hardcoded_numeric_pattern, re.IGNORECASE),
        ),
        (
            "bp1-issuer-string-branch",
            re.compile(rf"""(?:==|===|!=|!==)\s*["']{issuer_group}["']""", re.IGNORECASE),
        ),
        (
            "bp1-issuer-string-branch",
            re.compile(rf"""["']{issuer_group}["']\s*(?:==|===|!=|!==)""", re.IGNORECASE),
        ),
        (
            "bp1-issuer-string-branch",
            re.compile(
                rf"""["']{issuer_group}["'](?:\s*\|\s*(?:["'][^"'\r\n]+["']|[A-Za-z_][A-Za-z0-9_:]*))*\s*(?:\bif\b[^;{{}}\r\n]+?)?=>""",
                re.IGNORECASE,
            ),
        ),
        (
            "bp1-issuer-string-branch",
            re.compile(rf"""\bcase\s+["']{issuer_group}["']\s*:""", re.IGNORECASE),
        ),
        (
            "bp1-issuer-enum-branch",
            re.compile(rf"(?:==|===|!=|!==)\s*{issuer_enum}", re.IGNORECASE),
        ),
        (
            "bp1-issuer-enum-branch",
            re.compile(rf"{issuer_enum}\s*(?:==|===|!=|!==)", re.IGNORECASE),
        ),
        (
            "bp1-issuer-enum-branch",
            re.compile(
                rf"{issuer_enum}(?:\s*\|\s*[A-Za-z0-9_:]+)*\s*(?:\bif\b[^;{{}}\r\n]+?)?=>",
                re.IGNORECASE,
            ),
        ),
        (
            "bp1-issuer-enum-branch",
            re.compile(rf"\bcase\s+{issuer_enum}\s*:", re.IGNORECASE),
        ),
        (
            "bp1-futures-product-branch",
            re.compile(rf'(?:==|===|!=|!==)\s*"{product_group}"'),
        ),
        (
            "bp1-futures-product-branch",
            re.compile(rf'"{product_group}"\s*(?:==|===|!=|!==)'),
        ),
        (
            "bp1-futures-product-branch",
            re.compile(
                rf'"{product_group}"(?:\s*\|\s*"[^"\r\n]+")*\s*(?:\bif\b[^;{{}}\r\n]+?)?=>'
            ),
        ),
        (
            "bp1-futures-product-branch",
            re.compile(rf'\bcase\s+"{product_group}"\s*:'),
        ),
    )


_BP1_RULES = _compile_bp1_rules()


def _scan_ac03(repo_root: Path) -> list[Violation]:
    violations: list[Violation] = []
    for path in _iter_files(repo_root, repo_root, _AC03_EXTENSIONS, apply_exclusions=True):
        rel = _rel_posix(repo_root, path)
        for line_no, line in enumerate(_read_text(path).splitlines(), start=1):
            for pat in _AC03_PATTERNS:
                if pat.search(line):
                    violations.append(
                        Violation("AC03", rel, line_no, "market-branch", line.strip())
                    )
                    break
    return violations


def _scan_ac01_domain(repo_root: Path) -> list[Violation]:
    domain_root = repo_root / "crates" / "ficant-domain" / "src"
    if not domain_root.is_dir():
        raise ValueError(f"Domain source root is missing: {domain_root}")
    violations: list[Violation] = []
    for path in _iter_files(repo_root, domain_root, _DOMAIN_EXTENSIONS, apply_exclusions=True):
        rel = _rel_posix(repo_root, path)
        for line_no, line in enumerate(_read_text(path).splitlines(), start=1):
            for category, pat in _AC01_RULES:
                if pat.search(line):
                    violations.append(
                        Violation("AC01", rel, line_no, category, line.strip())
                    )
                    break
    return violations


def _scan_phase2c(repo_root: Path) -> list[Violation]:
    candidates: set[str] = {
        "cpp/fixed-income-kernel/include/ficant_kernel.h",
        "crates/ficant-kernel-sys/src/lib.rs",
        "crates/ficant-fixed-income-native/src/lib.rs",
    }
    cpp_src = repo_root / "cpp" / "fixed-income-kernel" / "src"
    if cpp_src.is_dir():
        for entry in cpp_src.iterdir():
            if (
                entry.is_file()
                and entry.stem.lower().startswith("futures_")
                and entry.suffix.lower() in _PHASE2C_CPP_EXTENSIONS
            ):
                candidates.add(_rel_posix(repo_root, entry))

    violations: list[Violation] = []
    for rel in sorted(candidates):
        path = repo_root / Path(rel)
        if not path.is_file():
            continue
        for line_no, line in enumerate(_read_text(path).splitlines(), start=1):
            for category, pat in _PHASE2C_RULES:
                if pat.search(line):
                    violations.append(
                        Violation("Phase2C", rel, line_no, category, line.strip())
                    )
                    break
    return violations


def _cpp_kernel_files(repo_root: Path) -> set[str]:
    cpp_root = repo_root / "cpp" / "fixed-income-kernel"
    if not cpp_root.is_dir():
        return set()
    return {
        _rel_posix(repo_root, p)
        for p in _iter_files(repo_root, cpp_root, _CPP_EXTENSIONS, apply_exclusions=False)
    }


def _scan_r3a_funding(repo_root: Path) -> list[Violation]:
    candidates = {
        "interface/proto/ficant/core/v1/subject.proto",
        "interface/proto/ficant/core/v1/subject_state.proto",
        "crates/ficant-domain/src/subject.rs",
        "crates/ficant-domain/src/subject_state.rs",
        "crates/ficant-kernel-sys/src/lib.rs",
        "crates/ficant-fixed-income-native/src/lib.rs",
    } | _cpp_kernel_files(repo_root)

    violations: list[Violation] = []
    for rel in sorted(candidates):
        path = repo_root / Path(rel)
        if not path.is_file():
            continue
        for line_no, line in enumerate(_read_text(path).splitlines(), start=1):
            if _R3A_PATTERN.search(line):
                violations.append(
                    Violation("R3a", rel, line_no, "funding-rate-value", line.strip())
                )
    return violations


def _scan_r3b_tax(repo_root: Path) -> list[Violation]:
    candidates = {
        "interface/proto/ficant/core/v1/subject.proto",
        "interface/proto/ficant/core/v1/subject_state.proto",
        "crates/ficant-domain/src/subject.rs",
        "crates/ficant-domain/src/market/bond.rs",
        "crates/ficant-kernel-sys/src/lib.rs",
        "crates/ficant-fixed-income-native/src/lib.rs",
    } | _cpp_kernel_files(repo_root)

    violations: list[Violation] = []
    for rel in sorted(candidates):
        path = repo_root / Path(rel)
        if not path.is_file():
            continue
        pat = _R3B_PROTO_PATTERN if path.suffix.lower() == ".proto" else _R3B_CODE_PATTERN
        for line_no, line in enumerate(_read_text(path).splitlines(), start=1):
            if pat.search(line):
                violations.append(
                    Violation("R3b", rel, line_no, "tax-rate-value", line.strip())
                )
    return violations


def _scan_bp1_production_rules(repo_root: Path) -> list[Violation]:
    files: list[Path] = []
    seen: set[Path] = set()
    for rel_root in _BP1_PRODUCTION_ROOTS:
        abs_root = repo_root / Path(rel_root)
        for path in _iter_files(
            repo_root, abs_root, _BP1_PRODUCTION_EXTENSIONS, apply_exclusions=True
        ):
            if path not in seen:
                seen.add(path)
                files.append(path)
    files.sort()

    violations: list[Violation] = []
    for path in files:
        content = _read_text(path)
        if not content:
            continue
        rel = _rel_posix(repo_root, path)
        for category, pat in _BP1_RULES:
            for match in pat.finditer(content):
                line_no, snippet = _line_snippet_at_offset(content, match.start())
                violations.append(Violation("BP-1", rel, line_no, category, snippet))
    return violations


def scan_repository(repo_root: Path) -> ScanReport:
    """Independently execute all layering checks against repo_root without invoking pwsh."""
    root = repo_root.resolve()
    if not root.is_dir():
        return ScanReport(1, 0, (), f"Repository root is missing: {root}")

    try:
        allowlist = _read_layering_allowlist(root)
        violations: list[Violation] = []
        violations.extend(_scan_ac03(root))
        violations.extend(_scan_ac01_domain(root))
        violations.extend(_scan_phase2c(root))
        violations.extend(_scan_r3a_funding(root))
        violations.extend(_scan_r3b_tax(root))
        violations.extend(_scan_bp1_production_rules(root))
    except Exception as exc:  # noqa: BLE001
        return ScanReport(1, 0, (), str(exc))

    exit_code = 1 if violations else 0
    return ScanReport(exit_code, len(allowlist), tuple(violations), None)


def build_fixture_catalog() -> list[dict[str, Any]]:
    """Return the deterministic shared fixture catalog used by both Python and PowerShell."""
    cc = _COUNTRY_CODE
    q = chr(34)
    sq = chr(39)
    concat_cc = f"{q}{_COUNTRY_CHAR_1}{q} + {q}{_COUNTRY_CHAR_2}{q}"
    legacy_const = _LEGACY_NOTIONAL_CONST
    cdb = _ISSUER_CDB
    eibc = _ISSUER_EIBC
    adbc = _ISSUER_ADBC
    mof = _ISSUER_MOF
    ts = _PROD_TS
    tf = _PROD_TF
    tl = _PROD_TL
    t_code = _PROD_T

    fixtures: list[dict[str, Any]] = [
        # --- Baseline & Allowlist ---
        {
            "id": "baseline_clean_empty_allowlist",
            "category": "baseline",
            "relative_path": "crates/ficant-domain/src/placeholder.rs",
            "content": "pub const CLEAN: u32 = 1;\n",
            "expected_exit_code": 0,
        },
        {
            "id": "allowlist_nonempty_rejected",
            "category": "allowlist",
            "relative_path": "scripts/layering-allowlist.json",
            "content": '[{"path":"crates/ficant-domain/src/futures_delivery.rs","violation":"market-rule-values","removal_round":"R2"}]\n',
            "expected_exit_code": 1,
        },
        # --- Legacy AC03 (whole-repo root '.', including outside 7 dirs + exclusion verification) ---
        {
            "id": "legacy_ac03_rust_market_comparison",
            "category": "ac03",
            "relative_path": "crates/ficant-domain/src/market_comparison.rs",
            "content": f"let supported = market == {q}{cc}{q};\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_ac03_rust_market_match",
            "category": "ac03",
            "relative_path": "crates/ficant-domain/src/market_match.rs",
            "content": f"match market {{ {q}{cc}{q} => 1, _ => 0 }}\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_ac03_rust_market_map",
            "category": "ac03",
            "relative_path": "crates/ficant-domain/src/market_map.rs",
            "content": f"let values = HashMap::from([({q}{cc}{q}, 1)]);\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_ac03_rust_market_concatenation",
            "category": "ac03",
            "relative_path": "crates/ficant-domain/src/market_concatenation.rs",
            "content": f"let supported = market == ({concat_cc});\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_ac03_cpp_market_branch",
            "category": "ac03",
            "relative_path": "cpp/market_branch.cpp",
            "content": f"if (market == {q}{cc}{q}) {{ return 1; }}\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_ac03_outside_7dirs_test_source",
            "category": "ac03",
            "relative_path": "tests/market_branch.rs",
            "content": f"match market {{ {q}{cc}{q} => 1, _ => 0 }}\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_ac03_outside_7dirs_migration_sql",
            "category": "ac03",
            "relative_path": "migrations/market_branch.sql",
            "content": f"CASE {sq}{cc}{sq} WHEN {sq}x{sq} THEN 1 END\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_ac03_excluded_fixtures_dir_allowed",
            "category": "ac03-exclusion",
            "relative_path": "fixtures/market_branch.rs",
            "content": f"match market {{ {q}{cc}{q} => 1, _ => 0 }}\n",
            "expected_exit_code": 0,
        },
        # --- Legacy AC01 Domain Rule Values ---
        {
            "id": "legacy_ac01_domain_original_term_months",
            "category": "ac01",
            "relative_path": "crates/ficant-domain/src/market_rules.rs",
            "content": "fn original_term_months() -> u32 { 120 }\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_ac01_domain_residual_term_bounds",
            "category": "ac01",
            "relative_path": "crates/ficant-domain/src/other_market_rules.rs",
            "content": "fn residual_term_bounds() -> (u32, Option<u32>) { (48, Some(63)) }\n",
            "expected_exit_code": 1,
        },
        # --- Legacy Phase 2C C++/FFI Production Rule Values ---
        {
            "id": "legacy_phase2c_cpp_eligibility_bounds",
            "category": "phase2c",
            "relative_path": "cpp/fixed-income-kernel/src/futures_rules.cpp",
            "content": "static const uint32_t original_term_max_months = 120;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_phase2c_cpp_delivery_months",
            "category": "phase2c",
            "relative_path": "cpp/fixed-income-kernel/src/futures_rules.cpp",
            "content": "static const uint32_t delivery_months[] = {3, 6, 9, 12};\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_phase2c_cpp_standard_coupon",
            "category": "phase2c",
            "relative_path": "cpp/fixed-income-kernel/src/futures_rules.cpp",
            "content": "static const double nominal_coupon = 0.03;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_phase2c_cpp_face_quote_basis",
            "category": "phase2c",
            "relative_path": "cpp/fixed-income-kernel/src/futures_rules.cpp",
            "content": "static const double face_quote_basis = 100.0;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_phase2c_cpp_rounding_scale",
            "category": "phase2c",
            "relative_path": "cpp/fixed-income-kernel/src/futures_rules.cpp",
            "content": "static const uint32_t conversion_factor_rounding_places = 4;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_phase2c_cpp_annual_day_basis",
            "category": "phase2c",
            "relative_path": "cpp/fixed-income-kernel/src/futures_rules.cpp",
            "content": "static const uint32_t annual_day_basis = 365;\n",
            "expected_exit_code": 1,
        },
        # --- Legacy R3a Funding Rule Values ---
        {
            "id": "legacy_r3a_subject_proto_funding_rate",
            "category": "r3a",
            "relative_path": "interface/proto/ficant/core/v1/subject.proto",
            "content": "double annual_financing_rate = 0.018;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_r3a_subject_state_proto_funding_rate",
            "category": "r3a",
            "relative_path": "interface/proto/ficant/core/v1/subject_state.proto",
            "content": "double financing_rate = 0.018;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_r3a_domain_funding_rate",
            "category": "r3a",
            "relative_path": "crates/ficant-domain/src/subject.rs",
            "content": "let funding_rate = 0.018;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_r3a_cpp_funding_rate",
            "category": "r3a",
            "relative_path": "cpp/fixed-income-kernel/src/funding.cpp",
            "content": "const double financing_rate = 0.018;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_r3a_ffi_funding_rate",
            "category": "r3a",
            "relative_path": "crates/ficant-kernel-sys/src/lib.rs",
            "content": "let annual_financing_rate = 0.018;\n",
            "expected_exit_code": 1,
        },
        # --- Legacy R3b Tax Rule Values ---
        {
            "id": "legacy_r3b_subject_proto_tax_rate",
            "category": "r3b",
            "relative_path": "interface/proto/ficant/core/v1/subject.proto",
            "content": "double coupon_tax_rate = 0.13;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_r3b_domain_bond_tax_rate",
            "category": "r3b",
            "relative_path": "crates/ficant-domain/src/market/bond.rs",
            "content": "let coupon_tax_rate = 0.13;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_r3b_cpp_tax_rate",
            "category": "r3b",
            "relative_path": "cpp/fixed-income-kernel/src/tax.cpp",
            "content": "const double coupon_tax_rate = 0.13;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "legacy_r3b_ffi_tax_rate",
            "category": "r3b",
            "relative_path": "crates/ficant-kernel-sys/src/lib.rs",
            "content": "let coupon_tax_rate = 0.13;\n",
            "expected_exit_code": 1,
        },
        # --- BP-1 7-Dir Negative: Legacy Constant & Hardcoded Futures Numerics ---
        {
            "id": "bp1_legacy_notional_constant_rust",
            "category": "bp1-notional",
            "relative_path": "crates/ficant-api/src/rates.rs",
            "content": f"pub fn notional() -> f64 {{ {legacy_const} }}\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_legacy_notional_constant_cpp",
            "category": "bp1-notional",
            "relative_path": "cpp/fixed-income-kernel/include/ficant_kernel.h",
            "content": f"#define {legacy_const} 1000000.0\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_rust_multiline_typed_from_scaled",
            "category": "bp1-notional",
            "relative_path": "crates/ficant-domain/src/futures_hedge.rs",
            "content": (
                "pub const CONTRACT_NOTIONAL: FixedDecimal =\n"
                "    FixedDecimal::from_scaled(\n"
                "        2_000_000_000_000_000_000\n"
                "    );\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_rust_field_colon_from_scaled",
            "category": "bp1-notional",
            "relative_path": "crates/ficant-api/src/rates.rs",
            "content": (
                "let input = HedgeReq {\n"
                "    contract_notional:\n"
                "        FixedDecimal::from_scaled(1_000_000_000_000_000_000),\n"
                "};\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_rust_field_colon_some_separated",
            "category": "bp1-notional",
            "relative_path": "crates/ficant-domain/src/futures_delivery.rs",
            "content": (
                "let rule = Rule {\n"
                "    contract_size_in_quote_units: Some(\n"
                "        20_000\n"
                "    ),\n"
                "};\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_rust_typed_let_some_from_scaled",
            "category": "bp1-notional",
            "relative_path": "crates/ficant-application/src/use_cases/futures_hedge.rs",
            "content": (
                "let contract_notional: Option<FixedDecimal> = Some(\n"
                "    FixedDecimal::from_scaled(1_000_000_000_000_000_000)\n"
                ");\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_rust_assignment_separated_float",
            "category": "bp1-notional",
            "relative_path": "crates/ficant-fixed-income-native/src/lib.rs",
            "content": (
                "let mut contract_notional = notional;\n"
                "contract_notional = 2_000_000.0;\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_rust_face_quote_basis_colon_literal",
            "category": "bp1-notional",
            "relative_path": "crates/ficant-kernel-sys/src/lib.rs",
            "content": (
                "let raw = RawInput {\n"
                "    face_quote_basis: 100.0,\n"
                "};\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_cpp_direct_brace_notional",
            "category": "bp1-notional",
            "relative_path": "cpp/fixed-income-kernel/src/hedge_math.cpp",
            "content": "static constexpr double CONTRACT_NOTIONAL{1000000.0};\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_cpp_equals_brace_face_quote",
            "category": "bp1-notional",
            "relative_path": "cpp/fixed-income-kernel/src/hedge_api.cpp",
            "content": "constexpr double FACE_QUOTE = {100.0};\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_cpp_multiline_digit_separator_notional",
            "category": "bp1-notional",
            "relative_path": "cpp/fixed-income-kernel/src/hedge_math.cpp",
            "content": (
                "constexpr double futures_contract_notional =\n"
                f"    1{sq}000{sq}000.0;\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_cpp_header_equals_brace_multiline",
            "category": "bp1-notional",
            "relative_path": "cpp/fixed-income-kernel/include/ficant_kernel.h",
            "content": (
                "static constexpr double face_quote_basis = {\n"
                "    100.0\n"
                "};\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_rust_typed_face_quote_from_scaled",
            "category": "bp1-notional",
            "relative_path": "crates/ficant-domain/src/futures_delivery.rs",
            "content": "const FACE_QUOTE_BASIS: FixedDecimal = FixedDecimal::from_scaled(100_000_000_000_000);\n",
            "expected_exit_code": 1,
        },
        # --- BP-1 7-Dir Negative: Issuer String & Enum Branches + Futures Product Branches ---
        {
            "id": "bp1_issuer_string_eq_rhs_rust",
            "category": "bp1-issuer",
            "relative_path": "crates/ficant-api/src/rates.rs",
            "content": f"let flag = issuer == {q}{cdb}{q};\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_issuer_string_eq_lhs_rust",
            "category": "bp1-issuer",
            "relative_path": "crates/ficant-application/src/use_cases/bond_analytics.rs",
            "content": f"let flag = {q}{cdb}{q} == issuer;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_issuer_string_ne_rhs_rust",
            "category": "bp1-issuer",
            "relative_path": "crates/ficant-domain/src/analytics.rs",
            "content": f"if issuer != {q}{adbc}{q} {{ return; }}\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_issuer_string_ne_lhs_cpp",
            "category": "bp1-issuer",
            "relative_path": "cpp/fixed-income-kernel/src/bond_math.cpp",
            "content": f"if ({q}{mof}{q} != issuer) {{ return 0.0; }}\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_issuer_string_match_rust",
            "category": "bp1-issuer",
            "relative_path": "crates/ficant-domain/src/analytics.rs",
            "content": (
                "let weight = match issuer_code {\n"
                f"    {q}{eibc}{q} => 1,\n"
                "    _ => 0,\n"
                "};\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_issuer_enum_eq_lhs_rust",
            "category": "bp1-issuer",
            "relative_path": "crates/ficant-application/src/use_cases/bond_analytics.rs",
            "content": f"if Issuer::{cdb.title()} == issuer {{ return; }}\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_issuer_enum_ne_rhs_rust",
            "category": "bp1-issuer",
            "relative_path": "crates/ficant-api/src/rates.rs",
            "content": f"if issuer != Issuer::{eibc.title()} {{ return; }}\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_bond_issuer_enum_ne_lhs_rust",
            "category": "bp1-issuer",
            "relative_path": "crates/ficant-domain/src/analytics.rs",
            "content": f"if BondIssuer::{adbc} != issuer {{ return; }}\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_issuer_enum_match_rust",
            "category": "bp1-issuer",
            "relative_path": "crates/ficant-domain/src/analytics.rs",
            "content": (
                "let tax = match issuer {\n"
                f"    Issuer::{mof.title()} => 0,\n"
                "    _ => 1,\n"
                "};\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_cpp_case_issuer_cdb",
            "category": "bp1-issuer",
            "relative_path": "cpp/fixed-income-kernel/src/bond_math.cpp",
            "content": (
                "switch (issuer) {\n"
                f"    case Issuer::{cdb.title()}:\n"
                "        return 1.0;\n"
                "}\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_cpp_case_bond_issuer_adbc",
            "category": "bp1-issuer",
            "relative_path": "cpp/fixed-income-kernel/src/kernel_api.cpp",
            "content": (
                "switch (issuer) {\n"
                f"    case BondIssuer::{adbc}:\n"
                "        return 2.0;\n"
                "}\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_futures_product_code_match_rust",
            "category": "bp1-futures-product",
            "relative_path": "crates/ficant-api/src/rates.rs",
            "content": (
                "let mult = match product_code {\n"
                f"    {q}{ts}{q} => 2,\n"
                "    _ => 1,\n"
                "};\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp1_futures_product_code_eq_lhs_rust",
            "category": "bp1-futures-product",
            "relative_path": "crates/ficant-application/src/use_cases/futures_hedge.rs",
            "content": f"if {q}{tf}{q} == product_code {{ return; }}\n",
            "expected_exit_code": 1,
        },
        # --- BP-1 Positive Controls (must exit 0) ---
        {
            "id": "bp1_positive_rust_type_declarations",
            "category": "bp1-positive",
            "relative_path": "crates/ficant-kernel-sys/src/lib.rs",
            "content": (
                "#[repr(C)]\n"
                "pub struct HedgeInputV2 {\n"
                "    pub contract_notional: f64,\n"
                "    pub face_quote_basis: f64,\n"
                "}\n"
            ),
            "expected_exit_code": 0,
        },
        {
            "id": "bp1_positive_cpp_header_declaration",
            "category": "bp1-positive",
            "relative_path": "cpp/fixed-income-kernel/include/ficant_kernel.h",
            "content": (
                "typedef struct {\n"
                "    double contract_notional;\n"
                "    double face_quote_basis;\n"
                "} hedge_input_v2;\n"
            ),
            "expected_exit_code": 0,
        },
        {
            "id": "bp1_positive_rust_field_passthrough",
            "category": "bp1-positive",
            "relative_path": "crates/ficant-api/src/rates.rs",
            "content": (
                "let input = FuturesHedgeInput {\n"
                "    contract_notional: notional,\n"
                "    face_quote_basis: rule.face_quote_basis(),\n"
                "    contract_size_in_quote_units: Some(units),\n"
                "};\n"
            ),
            "expected_exit_code": 0,
        },
        {
            "id": "bp1_positive_l3_rulepack_parser_allowed",
            "category": "bp1-positive",
            "relative_path": "crates/ficant-cgb-futures-pack/src/lib.rs",
            "content": (
                "const CONTRACT_NOTIONAL: f64 = 2_000_000.0;\n"
                "const FACE_QUOTE_BASIS: f64 = 100.0;\n"
                "fn check(product_code: &str, issuer: &str) -> bool {\n"
                f"    product_code == {q}{ts}{q} && issuer == {q}{cdb}{q}\n"
                "}\n"
            ),
            "expected_exit_code": 0,
        },
        {
            "id": "bp1_positive_external_rust_test_allowed",
            "category": "bp1-positive",
            "relative_path": "crates/ficant-api/tests/hedge_cases.rs",
            "content": (
                "#[test]\n"
                "fn sample() {\n"
                "    let contract_notional: FixedDecimal = FixedDecimal::from_scaled(2_000_000_000_000_000_000);\n"
                "    let face_quote_basis = 100.0;\n"
                f"    assert!(issuer != {q}{adbc}{q} && Issuer::{cdb.title()} == expected);\n"
                "}\n"
            ),
            "expected_exit_code": 0,
        },
        {
            "id": "bp1_positive_external_cpp_test_allowed",
            "category": "bp1-positive",
            "relative_path": "cpp/fixed-income-kernel/tests/test_hedge_cases.cpp",
            "content": (
                "static constexpr double CONTRACT_NOTIONAL{1000000.0};\n"
                "constexpr double FACE_QUOTE = {100.0};\n"
                f"switch (issuer) {{ case BondIssuer::{adbc}: break; }}\n"
            ),
            "expected_exit_code": 0,
        },
        {
            "id": "bp1_positive_enum_code_mapping_and_char_t",
            "category": "bp1-positive",
            "relative_path": "crates/ficant-domain/src/futures_delivery.rs",
            "content": (
                "pub const fn code(product: Product) -> &'static str {\n"
                "    match product {\n"
                f"        Product::TwoYear => {q}{ts}{q},\n"
                f"        Product::FiveYear => {q}{tf}{q},\n"
                f"        Product::TenYear => {q}{t_code}{q},\n"
                f"        Product::ThirtyYear => {q}{tl}{q},\n"
                "    }\n"
                "}\n"
                "pub fn is_iso_separator(ch: char) -> bool {\n"
                f"    ch == {sq}{t_code}{sq}\n"
                "}\n"
            ),
            "expected_exit_code": 0,
        },
        # --- BP-2 8th Production Root (crates/ficant-blotter-store/src) ---
        {
            "id": "bp2_blotter_store_positive_clean",
            "category": "bp2-blotter-store-positive",
            "relative_path": "crates/ficant-blotter-store/src/store.rs",
            "content": (
                "pub struct StoredHedgeMeta {\n"
                "    pub contract_notional: i128,\n"
                "    pub face_quote_basis: i128,\n"
                "}\n"
                "pub fn build_meta(notional: i128, basis: i128) -> StoredHedgeMeta {\n"
                "    StoredHedgeMeta {\n"
                "        contract_notional: notional,\n"
                "        face_quote_basis: basis,\n"
                "    }\n"
                "}\n"
            ),
            "expected_exit_code": 0,
        },
        {
            "id": "bp2_blotter_store_negative_futures_product",
            "category": "bp2-blotter-store-futures-product",
            "relative_path": "crates/ficant-blotter-store/src/store.rs",
            "content": (
                "pub fn lot_multiplier(product_code: &str) -> u32 {\n"
                "    match product_code {\n"
                f"        {q}{ts}{q} | {q}{tf}{q} | {q}{t_code}{q} | {q}{tl}{q} => 2,\n"
                "        _ => 1,\n"
                "    }\n"
                "}\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp2_blotter_store_negative_issuer_literal",
            "category": "bp2-blotter-store-issuer",
            "relative_path": "crates/ficant-blotter-store/src/codec.rs",
            "content": (
                "pub fn is_policy_bank(issuer: &str) -> bool {\n"
                f"    issuer == {q}{cdb}{q} || issuer == {q}{eibc}{q} || issuer == {q}{adbc}{q} || issuer == {q}{mof}{q}\n"
                "}\n"
            ),
            "expected_exit_code": 1,
        },
        {
            "id": "bp2_blotter_store_negative_contract_notional",
            "category": "bp2-blotter-store-notional",
            "relative_path": "crates/ficant-blotter-store/src/spi/trade_feed.rs",
            "content": "pub const CONTRACT_NOTIONAL: f64 = 2_000_000.0;\n",
            "expected_exit_code": 1,
        },
        {
            "id": "bp2_blotter_store_negative_face_quote",
            "category": "bp2-blotter-store-notional",
            "relative_path": "crates/ficant-blotter-store/src/spi/market_quote.rs",
            "content": "pub const FACE_QUOTE_BASIS: f64 = 100.0;\n",
            "expected_exit_code": 1,
        },
    ]

    ids = [item["id"] for item in fixtures]
    if len(ids) != len(set(ids)):
        raise ValueError("Duplicate fixture ID detected in shared fixture catalog.")
    return fixtures


def materialize_fixture(base_dir: Path, fixture: dict[str, Any]) -> Path:
    """Create an isolated repository root with clean baseline plus the single fixture file."""
    fixture_root = base_dir / str(fixture["id"])
    fixture_root.mkdir(parents=True, exist_ok=True)

    allowlist_file = fixture_root / "scripts" / "layering-allowlist.json"
    allowlist_file.parent.mkdir(parents=True, exist_ok=True)
    allowlist_file.write_text("[]\n", encoding="utf-8")

    placeholder_file = fixture_root / "crates" / "ficant-domain" / "src" / "placeholder.rs"
    placeholder_file.parent.mkdir(parents=True, exist_ok=True)
    placeholder_file.write_text("pub const CLEAN: u32 = 1;\n", encoding="utf-8")

    target_file = fixture_root / Path(str(fixture["relative_path"]))
    target_file.parent.mkdir(parents=True, exist_ok=True)
    target_file.write_text(str(fixture["content"]), encoding="utf-8")
    return fixture_root


def _run_pwsh_gate(pwsh_path: str, repo_root: Path) -> tuple[int, str]:
    completed = subprocess.run(
        [
            pwsh_path,
            "-NoProfile",
            "-NonInteractive",
            "-File",
            str(GATE_SCRIPT),
            "-RepositoryRoot",
            str(repo_root),
        ],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    output = (completed.stdout + "\n" + completed.stderr).strip()
    return completed.returncode, output


# --- Target-Filtered Enabled Normal Dependency Closure (AC-002) ---

_CFG_TOKEN_RE = re.compile(r'\s*(all|any|not|[A-Za-z_][A-Za-z0-9_]*|"[^"]*"|[(),=])')
_LIB_TARGET_KINDS = frozenset(
    {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}
)


def get_rustc_host_triple() -> str:
    completed = subprocess.run(
        ["rustc", "-vV"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"rustc -vV failed with exit {completed.returncode}: {completed.stderr.strip()}"
        )
    for line in completed.stdout.splitlines():
        if line.startswith("host:"):
            triple = line.split(":", 1)[1].strip()
            if triple:
                return triple
    raise RuntimeError(f"Could not parse host triple from rustc -vV:\n{completed.stdout}")


def get_rustc_target_cfg(
    host_triple: str,
) -> tuple[frozenset[str], dict[str, frozenset[str]]]:
    completed = subprocess.run(
        ["rustc", "--print", "cfg", "--target", host_triple],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"rustc --print cfg --target {host_triple} failed with exit "
            f"{completed.returncode}: {completed.stderr.strip()}"
        )
    flags: set[str] = set()
    kv_mut: dict[str, set[str]] = {}
    for raw_line in completed.stdout.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if "=" in line:
            key, val = line.split("=", 1)
            kv_mut.setdefault(key.strip(), set()).add(val.strip().strip('"'))
        else:
            flags.add(line)
    return frozenset(flags), {k: frozenset(v) for k, v in kv_mut.items()}


def derive_cfg_from_triple(
    host_triple: str,
) -> tuple[frozenset[str], dict[str, frozenset[str]]]:
    parts = host_triple.strip().split("-")
    raw_arch = parts[0] if len(parts) >= 1 else ""
    vendor = parts[1] if len(parts) >= 2 else ""
    raw_os = parts[2] if len(parts) >= 3 else ""
    env = parts[3] if len(parts) >= 4 else ""

    arch_map = {
        "i386": "x86",
        "i586": "x86",
        "i686": "x86",
        "amd64": "x86_64",
        "arm64": "aarch64",
    }
    arch = arch_map.get(raw_arch, raw_arch)

    os_name = "macos" if raw_os == "darwin" else raw_os
    if os_name == "windows":
        families = {"windows"}
        flags = {"windows"}
    elif os_name in {
        "linux",
        "macos",
        "freebsd",
        "netbsd",
        "openbsd",
        "dragonfly",
        "ios",
        "android",
        "solaris",
        "illumos",
        "aix",
        "haiku",
        "redox",
    }:
        families = {"unix"}
        flags = {"unix"}
    elif arch in {"wasm32", "wasm64"}:
        families = {"wasm"}
        flags = set()
    else:
        families = set()
        flags = set()

    ptr_width = (
        "64"
        if arch in {"x86_64", "aarch64", "riscv64", "powerpc64", "s390x", "wasm64"}
        else "32"
    )
    kv: dict[str, frozenset[str]] = {
        "target_arch": frozenset({arch}),
        "target_vendor": frozenset({vendor}),
        "target_os": frozenset({os_name}),
        "target_env": frozenset({env}),
        "target_family": frozenset(families),
        "target_pointer_width": frozenset({ptr_width}),
        "target_endian": frozenset({"little"}),
    }
    return frozenset(flags), kv


def _tokenize_cfg(expr: str) -> list[str]:
    tokens: list[str] = []
    pos = 0
    length = len(expr)
    while pos < length:
        if expr[pos].isspace():
            pos += 1
            continue
        match = _CFG_TOKEN_RE.match(expr, pos)
        if match is None:
            raise ValueError(f"Unsupported cfg expression syntax at offset {pos}: {expr!r}")
        tokens.append(match.group(1))
        pos = match.end()
    return tokens


def _eval_cfg_expr(
    expr: str,
    flags: frozenset[str],
    kv: dict[str, frozenset[str]],
) -> bool:
    tokens = _tokenize_cfg(expr)
    if not tokens:
        raise ValueError("Empty cfg(...) expression.")
    pos = 0

    def _expect(expected: str) -> None:
        nonlocal pos
        if pos >= len(tokens) or tokens[pos] != expected:
            actual = tokens[pos] if pos < len(tokens) else "<eof>"
            raise ValueError(
                f"Malformed cfg expression {expr!r}: expected {expected!r}, got {actual!r}"
            )
        pos += 1

    def _parse_pred() -> bool:
        nonlocal pos
        if pos >= len(tokens):
            raise ValueError(f"Unexpected end of cfg expression: {expr!r}")
        tok = tokens[pos]
        if tok in ("all", "any"):
            pos += 1
            _expect("(")
            vals: list[bool] = []
            while pos < len(tokens) and tokens[pos] != ")":
                vals.append(_parse_pred())
                if pos < len(tokens) and tokens[pos] == ",":
                    pos += 1
                    if pos < len(tokens) and tokens[pos] == ")":
                        break
                else:
                    break
            _expect(")")
            return all(vals) if tok == "all" else any(vals)
        if tok == "not":
            pos += 1
            _expect("(")
            val = _parse_pred()
            if pos < len(tokens) and tokens[pos] == ",":
                pos += 1
            _expect(")")
            return not val
        pos += 1
        if pos < len(tokens) and tokens[pos] == "=":
            pos += 1
            if pos >= len(tokens) or not (
                tokens[pos].startswith('"') and tokens[pos].endswith('"')
            ):
                raise ValueError(
                    f"Malformed key=value predicate in cfg expression: {expr!r}"
                )
            val_str = tokens[pos][1:-1]
            pos += 1
            return val_str in kv.get(tok, frozenset())
        return tok in flags

    result = _parse_pred()
    if pos != len(tokens):
        raise ValueError(
            f"Trailing tokens in cfg expression {expr!r}: {tokens[pos:]!r}"
        )
    return result


def target_matches_host(
    target: str | None,
    host_triple: str,
    host_cfg: tuple[frozenset[str], dict[str, frozenset[str]]] | None = None,
) -> bool:
    if target is None:
        return True
    stripped = target.strip()
    if not stripped:
        return True
    if stripped.startswith("cfg(") and stripped.endswith(")"):
        flags, kv = host_cfg if host_cfg is not None else derive_cfg_from_triple(host_triple)
        return _eval_cfg_expr(stripped[4:-1], flags, kv)
    return stripped == host_triple


def _is_optional_dep_enabled(
    opt_name: str,
    node_features: list[str],
    pkg_features: dict[str, list[str]],
) -> bool:
    active: set[str] = set()
    stack = list(node_features)
    while stack:
        feat = stack.pop()
        if feat in active:
            continue
        active.add(feat)
        for item in pkg_features.get(feat, ()):
            if item == f"dep:{opt_name}":
                return True
            if item.startswith(f"{opt_name}/"):
                # Explicit <opt_name>/<subfeat> (WITHOUT '?' weak syntax) enables opt_name.
                return True
            if "?" in item or "/" in item or item.startswith("dep:"):
                # Weak syntax (<opt_name>?/<subfeat>) or unrelated dep feature MUST NOT enable opt_name.
                continue
            if item in pkg_features:
                if item not in active:
                    stack.append(item)
            elif item == opt_name:
                return True

    if opt_name in active:
        explicit_entries = pkg_features.get(opt_name)
        if explicit_entries is None:
            return True
        if any(
            entry == f"dep:{opt_name}" or entry.startswith(f"{opt_name}/")
            for entry in explicit_entries
        ):
            return True
        has_explicit_dep_syntax = any(
            entry == f"dep:{opt_name}"
            for entries in pkg_features.values()
            for entry in entries
        )
        if not has_explicit_dep_syntax:
            return True
    return False


def _matching_normal_manifest_deps(
    pkg: dict[str, Any],
    dep_edge: dict[str, Any],
    target_pkg: dict[str, Any],
    host_triple: str,
    host_cfg: tuple[frozenset[str], dict[str, frozenset[str]]] | None,
) -> tuple[list[dict[str, Any]], bool]:
    target_name = str(target_pkg["name"])
    edge_name = str(dep_edge["name"])
    lib_names = {target_name.replace("-", "_")}
    for tgt in target_pkg.get("targets", ()):
        if any(kind in _LIB_TARGET_KINDS for kind in tgt.get("kind", ())):
            lib_names.add(str(tgt["name"]).replace("-", "_"))

    matched_host_normal: list[dict[str, Any]] = []
    has_any_manifest_entry = False
    for mdep in pkg.get("dependencies", ()):
        if str(mdep.get("name", "")) != target_name:
            continue
        rename = mdep.get("rename")
        if rename:
            if str(rename).replace("-", "_") != edge_name:
                continue
        else:
            if edge_name not in lib_names:
                continue
        has_any_manifest_entry = True
        if mdep.get("kind") is not None:
            continue
        if not target_matches_host(mdep.get("target"), host_triple, host_cfg):
            continue
        matched_host_normal.append(mdep)

    return matched_host_normal, has_any_manifest_entry


def compute_enabled_normal_closure(
    metadata: dict[str, Any],
    root_crate_names: tuple[str, ...],
    host_triple: str,
    host_cfg: tuple[frozenset[str], dict[str, frozenset[str]]] | None = None,
) -> set[str]:
    """Traverse cargo metadata resolve graph along host-matching enabled normal edges only."""
    packages_list = metadata.get("packages", [])
    pkgs_by_id: dict[str, dict[str, Any]] = {str(p["id"]): p for p in packages_list}
    resolve = metadata.get("resolve")
    if not isinstance(resolve, dict):
        raise ValueError("cargo metadata is missing 'resolve' section.")
    nodes_by_id: dict[str, dict[str, Any]] = {
        str(n["id"]): n for n in resolve.get("nodes", [])
    }

    workspace_member_ids = set(str(mid) for mid in metadata.get("workspace_members", []))
    candidate_ids = (
        [pid for pid in workspace_member_ids if pid in pkgs_by_id]
        if workspace_member_ids
        else list(pkgs_by_id.keys())
    )

    root_ids: list[str] = []
    found_root_names: set[str] = set()
    for pid in candidate_ids:
        pname = str(pkgs_by_id[pid]["name"])
        if pname in root_crate_names:
            root_ids.append(pid)
            found_root_names.add(pname)
    missing_roots = sorted(set(root_crate_names) - found_root_names)
    if missing_roots:
        raise ValueError(f"Missing root crate(s) in cargo metadata: {missing_roots}")

    visited: set[str] = set()
    stack = list(root_ids)
    while stack:
        uid = stack.pop()
        if uid in visited:
            continue
        visited.add(uid)
        pkg = pkgs_by_id[uid]
        node = nodes_by_id.get(uid)
        if node is None:
            raise ValueError(f"Package {uid} missing from cargo metadata resolve.nodes.")

        node_features = [str(f) for f in node.get("features", ())]
        pkg_features: dict[str, list[str]] = {
            str(k): [str(item) for item in v]
            for k, v in pkg.get("features", {}).items()
        }

        for dep_edge in node.get("deps", ()):
            dep_kinds = dep_edge.get("dep_kinds", ())
            active_normal_kinds = [
                dk
                for dk in dep_kinds
                if dk.get("kind") is None
                and target_matches_host(dk.get("target"), host_triple, host_cfg)
            ]
            if not active_normal_kinds:
                continue

            v_id = str(dep_edge["pkg"])
            v_pkg = pkgs_by_id[v_id]
            matched_mdeps, has_any_manifest_entry = _matching_normal_manifest_deps(
                pkg, dep_edge, v_pkg, host_triple, host_cfg
            )
            if not matched_mdeps:
                if has_any_manifest_entry:
                    # Manifest only declares this dep as dev/build or for a non-host target.
                    continue
            elif all(bool(m.get("optional", False)) for m in matched_mdeps):
                if not any(
                    _is_optional_dep_enabled(
                        str(m.get("rename") or m["name"]),
                        node_features,
                        pkg_features,
                    )
                    for m in matched_mdeps
                ):
                    continue

            stack.append(v_id)

    return {str(pkgs_by_id[pid]["name"]) for pid in visited}


def find_forbidden_normal_dependencies(
    metadata: dict[str, Any],
    root_crate_names: tuple[str, ...] = _PURE_LAYER_ROOT_CRATES,
    forbidden_crates: frozenset[str] = _FORBIDDEN_PURE_LAYER_CRATES,
    host_triple: str = "",
    host_cfg: tuple[frozenset[str], dict[str, frozenset[str]]] | None = None,
) -> list[str]:
    if not host_triple:
        host_triple = get_rustc_host_triple()
    closure = compute_enabled_normal_closure(
        metadata,
        root_crate_names=root_crate_names,
        host_triple=host_triple,
        host_cfg=host_cfg,
    )
    return sorted(closure & forbidden_crates)


def _load_cargo_metadata(repo_root: Path, host_triple: str) -> dict[str, Any]:
    completed = subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            host_triple,
            "--locked",
            "--offline",
        ],
        cwd=str(repo_root),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"cargo metadata failed with exit {completed.returncode}: {completed.stderr.strip()}"
        )
    return json.loads(completed.stdout)


def _load_cargo_tree_normal_crates(
    repo_root: Path,
    host_triple: str,
    root_crate_names: tuple[str, ...],
) -> set[str]:
    cmd = [
        "cargo",
        "tree",
        "-e",
        "normal",
        "--target",
        host_triple,
        "--locked",
        "--offline",
        "--prefix",
        "none",
    ]
    for crate_name in root_crate_names:
        cmd.extend(["-p", crate_name])
    completed = subprocess.run(
        cmd,
        cwd=str(repo_root),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"cargo tree failed with exit {completed.returncode}: {completed.stderr.strip()}"
        )
    crates: set[str] = set()
    for raw_line in completed.stdout.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        crates.add(line.split()[0])
    return crates


def _emit_fixtures_json() -> str:
    payload = {
        "version": 1,
        "fixtures": build_fixture_catalog(),
    }
    return json.dumps(payload, ensure_ascii=False, indent=2)


def _cli_main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Emit shared layering invariant fixtures as UTF-8 JSON."
    )
    parser.add_argument(
        "--emit-fixtures",
        action="store_true",
        help="Print the versioned shared fixture catalog JSON to stdout.",
    )
    args = parser.parse_args(argv)
    if args.emit_fixtures:
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stdout.write(_emit_fixtures_json() + "\n")
        return 0
    parser.print_help()
    return 2


if pytest is not None:
    FIXTURE_CATALOG = build_fixture_catalog()

    def test_emit_fixtures_cli_contract() -> None:
        completed = subprocess.run(
            [sys.executable, str(Path(__file__).resolve()), "--emit-fixtures"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=False,
        )
        assert completed.returncode == 0, completed.stderr
        payload = json.loads(completed.stdout)
        assert payload["version"] == 1
        fixtures = payload["fixtures"]
        assert isinstance(fixtures, list)
        assert len(fixtures) == len(FIXTURE_CATALOG)
        emitted_ids = [item["id"] for item in fixtures]
        expected_ids = [item["id"] for item in FIXTURE_CATALOG]
        assert emitted_ids == expected_ids
        assert len(set(emitted_ids)) == len(emitted_ids)
        for item in fixtures:
            assert item["expected_exit_code"] in (0, 1)
            assert item["relative_path"]
            assert item["category"]
            assert isinstance(item["content"], str)

    @pytest.mark.parametrize(
        "fixture",
        FIXTURE_CATALOG,
        ids=[item["id"] for item in FIXTURE_CATALOG],
    )
    def test_python_scanner_fixture(fixture: dict[str, Any], tmp_path: Path) -> None:
        fixture_root = materialize_fixture(tmp_path, fixture)
        report = scan_repository(fixture_root)
        assert report.exit_code == fixture["expected_exit_code"], (
            f"Fixture {fixture['id']} expected exit {fixture['expected_exit_code']} "
            f"but got {report.exit_code}; error={report.error}; violations={report.violations}"
        )
        if fixture["expected_exit_code"] == 0:
            assert report.error is None
            assert report.violations == ()
        else:
            assert report.error is not None or len(report.violations) > 0

    def test_python_scanner_real_repository() -> None:
        report = scan_repository(REPO_ROOT)
        assert report.error is None, report.error
        assert report.allowlist_count == 0
        assert report.violations == (), f"Unexpected violations: {report.violations}"
        assert report.exit_code == 0

    def test_powershell_and_python_cross_parity(tmp_path: Path) -> None:
        pwsh_path = shutil.which("pwsh")
        if pwsh_path is None:
            pytest.skip(
                "pwsh is not available on this host; pure Python scanner tests already ran in full."
            )

        # 1. Real repository tree parity
        py_real = scan_repository(REPO_ROOT)
        ps_real_code, ps_real_out = _run_pwsh_gate(pwsh_path, REPO_ROOT)
        assert py_real.exit_code == 0, f"Python failed on real repo: {py_real}"
        assert ps_real_code == 0, f"PowerShell failed on real repo: {ps_real_out}"

        # 2. Full fixture ID set parity across isolated temp roots
        roots: dict[str, Path] = {
            fixture["id"]: materialize_fixture(tmp_path, fixture)
            for fixture in FIXTURE_CATALOG
        }
        expected_codes: dict[str, int] = {
            fixture["id"]: int(fixture["expected_exit_code"])
            for fixture in FIXTURE_CATALOG
        }
        py_codes: dict[str, int] = {
            fid: scan_repository(root).exit_code for fid, root in roots.items()
        }

        def _eval_ps(item: tuple[str, Path]) -> tuple[str, int, str]:
            fid, root = item
            code, out = _run_pwsh_gate(pwsh_path, root)
            return fid, code, out

        ps_codes: dict[str, int] = {}
        ps_outputs: dict[str, str] = {}
        with ThreadPoolExecutor(max_workers=8) as executor:
            for fid, code, out in executor.map(_eval_ps, roots.items()):
                ps_codes[fid] = code
                ps_outputs[fid] = out

        assert set(py_codes.keys()) == set(expected_codes.keys())
        assert set(ps_codes.keys()) == set(expected_codes.keys())

        mismatches: list[str] = []
        for fid, exp in expected_codes.items():
            py_c = py_codes[fid]
            ps_c = ps_codes[fid]
            if py_c != exp or ps_c != exp or py_c != ps_c:
                mismatches.append(
                    f"{fid}: expected={exp} python={py_c} pwsh={ps_c} pwsh_out={ps_outputs[fid]!r}"
                )
        assert not mismatches, "Cross-runtime parity mismatches:\n" + "\n".join(mismatches)

    def test_pure_crates_enabled_normal_dependency_closure() -> None:
        host_triple = get_rustc_host_triple()
        host_cfg = get_rustc_target_cfg(host_triple)
        metadata = _load_cargo_metadata(REPO_ROOT, host_triple)

        closure = compute_enabled_normal_closure(
            metadata,
            root_crate_names=_PURE_LAYER_ROOT_CRATES,
            host_triple=host_triple,
            host_cfg=host_cfg,
        )
        assert set(_PURE_LAYER_ROOT_CRATES).issubset(closure)

        forbidden_hits = find_forbidden_normal_dependencies(
            metadata,
            root_crate_names=_PURE_LAYER_ROOT_CRATES,
            forbidden_crates=_FORBIDDEN_PURE_LAYER_CRATES,
            host_triple=host_triple,
            host_cfg=host_cfg,
        )
        assert forbidden_hits == [], (
            f"Pure crates {_PURE_LAYER_ROOT_CRATES} enabled normal closure contains "
            f"forbidden dependencies: {forbidden_hits}"
        )

        # Cross-verify exact agreement with `cargo tree -e normal --target <host-triple>`.
        cargo_tree_crates = _load_cargo_tree_normal_crates(
            REPO_ROOT, host_triple, _PURE_LAYER_ROOT_CRATES
        )
        assert closure == cargo_tree_crates, (
            "Mismatch between cargo metadata enabled normal closure and cargo tree:\n"
            f"  only_in_metadata={sorted(closure - cargo_tree_crates)}\n"
            f"  only_in_cargo_tree={sorted(cargo_tree_crates - closure)}"
        )
        assert sorted(cargo_tree_crates & _FORBIDDEN_PURE_LAYER_CRATES) == []

    def _make_synthetic_metadata(
        packages: list[dict[str, Any]],
        nodes: list[dict[str, Any]],
        workspace_members: list[str] | None = None,
    ) -> dict[str, Any]:
        normalized_pkgs: list[dict[str, Any]] = []
        for pkg in packages:
            name = str(pkg["name"])
            normalized_pkgs.append(
                {
                    "id": pkg.get("id", f"pkg#{name}@0.1.0"),
                    "name": name,
                    "version": pkg.get("version", "0.1.0"),
                    "dependencies": pkg.get("dependencies", []),
                    "features": pkg.get("features", {}),
                    "targets": pkg.get(
                        "targets",
                        [{"name": name.replace("-", "_"), "kind": ["lib"]}],
                    ),
                }
            )
        return {
            "packages": normalized_pkgs,
            "workspace_members": (
                workspace_members
                if workspace_members is not None
                else ["pkg#ficant-domain@0.1.0", "pkg#ficant-application@0.1.0"]
            ),
            "resolve": {"nodes": nodes},
        }

    def test_synthetic_dep_graph_dev_only_forbidden_allowed() -> None:
        """Synthetic Test 1: dev-only (and build-only) forbidden crate edges are allowed."""
        metadata = _make_synthetic_metadata(
            packages=[
                {
                    "name": "ficant-domain",
                    "dependencies": [
                        {"name": "rusqlite", "kind": "dev", "optional": False, "target": None},
                        {"name": "tokio", "kind": "dev", "optional": False, "target": None},
                        {"name": "prost", "kind": "build", "optional": False, "target": None},
                    ],
                },
                {
                    "name": "ficant-application",
                    "dependencies": [
                        {"name": "ficant-domain", "kind": None, "optional": False, "target": None},
                        {"name": "serde_json", "kind": "dev", "optional": False, "target": None},
                        {"name": "ficant-blotter-store", "kind": "dev", "optional": False, "target": None},
                    ],
                },
                {"name": "rusqlite"},
                {"name": "tokio"},
                {"name": "prost"},
                {"name": "serde_json"},
                {"name": "ficant-blotter-store"},
            ],
            nodes=[
                {
                    "id": "pkg#ficant-domain@0.1.0",
                    "features": [],
                    "deps": [
                        {
                            "name": "rusqlite",
                            "pkg": "pkg#rusqlite@0.1.0",
                            "dep_kinds": [{"kind": "dev", "target": None}],
                        },
                        {
                            "name": "tokio",
                            "pkg": "pkg#tokio@0.1.0",
                            "dep_kinds": [{"kind": "dev", "target": None}],
                        },
                        {
                            "name": "prost",
                            "pkg": "pkg#prost@0.1.0",
                            "dep_kinds": [{"kind": "build", "target": None}],
                        },
                    ],
                },
                {
                    "id": "pkg#ficant-application@0.1.0",
                    "features": [],
                    "deps": [
                        {
                            "name": "ficant_domain",
                            "pkg": "pkg#ficant-domain@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                        {
                            "name": "serde_json",
                            "pkg": "pkg#serde_json@0.1.0",
                            "dep_kinds": [{"kind": "dev", "target": None}],
                        },
                        {
                            "name": "ficant_blotter_store",
                            "pkg": "pkg#ficant-blotter-store@0.1.0",
                            "dep_kinds": [{"kind": "dev", "target": None}],
                        },
                    ],
                },
                {"id": "pkg#rusqlite@0.1.0", "features": [], "deps": []},
                {"id": "pkg#tokio@0.1.0", "features": [], "deps": []},
                {"id": "pkg#prost@0.1.0", "features": [], "deps": []},
                {"id": "pkg#serde_json@0.1.0", "features": [], "deps": []},
                {"id": "pkg#ficant-blotter-store@0.1.0", "features": [], "deps": []},
            ],
        )
        host = "x86_64-pc-windows-msvc"
        closure = compute_enabled_normal_closure(
            metadata, _PURE_LAYER_ROOT_CRATES, host_triple=host
        )
        assert closure == {"ficant-domain", "ficant-application"}
        assert find_forbidden_normal_dependencies(metadata, host_triple=host) == []

    def test_synthetic_dep_graph_unenabled_optional_weak_and_non_host_target_allowed() -> None:
        """Synthetic Test 2: unenabled optional dep, weak `dep?/feat` (`rust_decimal` `serde_json?/std`), and non-host target dep are allowed."""
        metadata = _make_synthetic_metadata(
            packages=[
                {
                    "name": "ficant-domain",
                    "dependencies": [
                        {"name": "rust_decimal", "kind": None, "optional": False, "target": None},
                        {"name": "tokio", "kind": None, "optional": True, "target": None},
                        {
                            "name": "rusqlite",
                            "kind": None,
                            "optional": False,
                            "target": 'cfg(target_os = "macos")',
                        },
                        {
                            "name": "http",
                            "kind": None,
                            "optional": False,
                            "target": "wasm32-unknown-unknown",
                        },
                    ],
                    "features": {
                        "async-runtime": ["dep:tokio"],
                    },
                },
                {
                    "name": "ficant-application",
                    "dependencies": [
                        {"name": "ficant-domain", "kind": None, "optional": False, "target": None},
                    ],
                },
                {
                    "name": "rust_decimal",
                    "dependencies": [
                        {"name": "serde_json", "kind": None, "optional": True, "target": None},
                    ],
                    "features": {
                        "std": ["serde_json?/std"],
                        "serde_json": ["dep:serde_json"],
                    },
                },
                {"name": "serde_json"},
                {"name": "tokio"},
                {"name": "rusqlite"},
                {"name": "http"},
            ],
            nodes=[
                {
                    "id": "pkg#ficant-domain@0.1.0",
                    "features": [],
                    "deps": [
                        {
                            "name": "rust_decimal",
                            "pkg": "pkg#rust_decimal@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                        {
                            "name": "tokio",
                            "pkg": "pkg#tokio@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                        {
                            "name": "rusqlite",
                            "pkg": "pkg#rusqlite@0.1.0",
                            "dep_kinds": [
                                {"kind": None, "target": 'cfg(target_os = "macos")'}
                            ],
                        },
                        {
                            "name": "http",
                            "pkg": "pkg#http@0.1.0",
                            "dep_kinds": [
                                {"kind": None, "target": "wasm32-unknown-unknown"}
                            ],
                        },
                    ],
                },
                {
                    "id": "pkg#ficant-application@0.1.0",
                    "features": [],
                    "deps": [
                        {
                            "name": "ficant_domain",
                            "pkg": "pkg#ficant-domain@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                    ],
                },
                {
                    "id": "pkg#rust_decimal@0.1.0",
                    "features": ["std"],
                    "deps": [
                        {
                            "name": "serde_json",
                            "pkg": "pkg#serde_json@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                    ],
                },
                {"id": "pkg#serde_json@0.1.0", "features": [], "deps": []},
                {"id": "pkg#tokio@0.1.0", "features": [], "deps": []},
                {"id": "pkg#rusqlite@0.1.0", "features": [], "deps": []},
                {"id": "pkg#http@0.1.0", "features": [], "deps": []},
            ],
        )

        host = "x86_64-pc-windows-msvc"
        closure = compute_enabled_normal_closure(
            metadata, _PURE_LAYER_ROOT_CRATES, host_triple=host
        )
        assert closure == {"ficant-domain", "ficant-application", "rust_decimal"}
        assert find_forbidden_normal_dependencies(metadata, host_triple=host) == []

        # Sensitivity check: matching non-host target triple DOES pick up target-specific dep.
        assert find_forbidden_normal_dependencies(
            metadata, host_triple="aarch64-apple-darwin"
        ) == ["rusqlite"]
        assert find_forbidden_normal_dependencies(
            metadata, host_triple="wasm32-unknown-unknown"
        ) == ["http"]

        # Sensitivity check: enabling optional dep via explicit `dep:` or `<opt>/<subfeat>` (no '?') DOES trigger.
        metadata_explicit_dep = json.loads(json.dumps(metadata))
        metadata_explicit_dep["resolve"]["nodes"][2]["features"] = ["std", "serde_json"]
        assert find_forbidden_normal_dependencies(
            metadata_explicit_dep, host_triple=host
        ) == ["serde_json"]

        metadata_nonweak_subfeat = json.loads(json.dumps(metadata))
        metadata_nonweak_subfeat["packages"][2]["features"]["std"] = ["serde_json/std"]
        assert find_forbidden_normal_dependencies(
            metadata_nonweak_subfeat, host_triple=host
        ) == ["serde_json"]

    def test_synthetic_dep_graph_transitive_normal_rusqlite_rejected() -> None:
        """Synthetic Test 3: transitive enabled normal edge introducing `rusqlite` is rejected."""
        metadata = _make_synthetic_metadata(
            packages=[
                {
                    "name": "ficant-domain",
                    "dependencies": [
                        {"name": "helper-ledger", "kind": None, "optional": False, "target": None},
                    ],
                },
                {
                    "name": "ficant-application",
                    "dependencies": [
                        {"name": "ficant-domain", "kind": None, "optional": False, "target": None},
                    ],
                },
                {
                    "name": "helper-ledger",
                    "dependencies": [
                        {"name": "rusqlite", "kind": None, "optional": False, "target": None},
                    ],
                },
                {
                    "name": "rusqlite",
                    "dependencies": [
                        {"name": "libsqlite3-sys", "kind": None, "optional": False, "target": None},
                    ],
                },
                {"name": "libsqlite3-sys"},
            ],
            nodes=[
                {
                    "id": "pkg#ficant-domain@0.1.0",
                    "features": [],
                    "deps": [
                        {
                            "name": "helper_ledger",
                            "pkg": "pkg#helper-ledger@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                    ],
                },
                {
                    "id": "pkg#ficant-application@0.1.0",
                    "features": [],
                    "deps": [
                        {
                            "name": "ficant_domain",
                            "pkg": "pkg#ficant-domain@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                    ],
                },
                {
                    "id": "pkg#helper-ledger@0.1.0",
                    "features": [],
                    "deps": [
                        {
                            "name": "rusqlite",
                            "pkg": "pkg#rusqlite@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                    ],
                },
                {
                    "id": "pkg#rusqlite@0.1.0",
                    "features": [],
                    "deps": [
                        {
                            "name": "libsqlite3_sys",
                            "pkg": "pkg#libsqlite3-sys@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                    ],
                },
                {"id": "pkg#libsqlite3-sys@0.1.0", "features": [], "deps": []},
            ],
        )
        host = "x86_64-pc-windows-msvc"
        hits = find_forbidden_normal_dependencies(metadata, host_triple=host)
        assert "rusqlite" in hits
        assert hits == ["libsqlite3-sys", "rusqlite"]

    def test_synthetic_dep_graph_transitive_normal_tokio_rejected() -> None:
        """Synthetic Test 4: transitive enabled normal edge introducing `tokio` is rejected."""
        metadata = _make_synthetic_metadata(
            packages=[
                {"name": "ficant-domain", "dependencies": []},
                {
                    "name": "ficant-application",
                    "dependencies": [
                        {"name": "ficant-domain", "kind": None, "optional": False, "target": None},
                        {"name": "helper-scheduler", "kind": None, "optional": False, "target": None},
                    ],
                },
                {
                    "name": "helper-scheduler",
                    "dependencies": [
                        {"name": "tokio", "kind": None, "optional": True, "target": None},
                    ],
                    "features": {
                        "default": ["rt"],
                        "rt": ["dep:tokio"],
                    },
                },
                {"name": "tokio"},
            ],
            nodes=[
                {"id": "pkg#ficant-domain@0.1.0", "features": [], "deps": []},
                {
                    "id": "pkg#ficant-application@0.1.0",
                    "features": [],
                    "deps": [
                        {
                            "name": "ficant_domain",
                            "pkg": "pkg#ficant-domain@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                        {
                            "name": "helper_scheduler",
                            "pkg": "pkg#helper-scheduler@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                    ],
                },
                {
                    "id": "pkg#helper-scheduler@0.1.0",
                    "features": ["default", "rt"],
                    "deps": [
                        {
                            "name": "tokio",
                            "pkg": "pkg#tokio@0.1.0",
                            "dep_kinds": [{"kind": None, "target": None}],
                        },
                    ],
                },
                {"id": "pkg#tokio@0.1.0", "features": [], "deps": []},
            ],
        )
        host = "x86_64-pc-windows-msvc"
        hits = find_forbidden_normal_dependencies(metadata, host_triple=host)
        assert hits == ["tokio"]


if __name__ == "__main__":
    raise SystemExit(_cli_main())
