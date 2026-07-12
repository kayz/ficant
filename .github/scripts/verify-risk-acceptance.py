#!/usr/bin/env python3
import argparse, datetime, hashlib, json, pathlib, re, sys


def fail(message):
    print(f"risk-acceptance: {message}", file=sys.stderr)
    raise SystemExit(2)


def read(path):
    try:
        return json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    except Exception as exc:
        fail(f"invalid JSON {path}: {exc}")


def exact_policy(lock):
    items = lock.get("risk_acceptances")
    if not isinstance(items, list) or len(items) != 1:
        fail("iteration-2 risk acceptance set drift")
    item = items[0]
    expected = {
        "id": "iteration-2-async-std-1.13.2",
        "status": "accepted-unfixed",
        "iteration": 2,
        "purl": "pkg:cargo/async-std@1.13.2",
        "name": "async-std",
        "version": "1.13.2",
        "source_locator": "https://crates.io/api/v1/crates/async-std/1.13.2/download",
        "source_integrity": "sha256:2c8e079a4ab67ae52b7403632e4618815d6db36d2a010cfe41b02c1b1578f93b",
        "reachable_via": {
            "purl": "pkg:cargo/minio@0.4.0", "name": "minio", "version": "0.4.0",
            "source_integrity": "sha256:cc93d8cbf49952a55414ac584a372cd785b6b988dca897b119bb8b0b3252f455",
        },
        "owner": "Architecture/Delivery",
        "decision_date": "2026-07-13",
        "expires_on": "2026-10-13",
        "reassess_by": "iteration-3-entry-or-first-external-release-or-2026-10-13",
        "advisory_ids": ["RUSTSEC-2025-0052"],
        "advisory_type": {"informational": "unmaintained"},
        "advisory_license": "CC0-1.0",
        "advisory_source": "https://github.com/rustsec/advisory-db/blob/osv/crates/RUSTSEC-2025-0052.json",
        "patched_versions": [],
    }
    try:
        if datetime.date.today() > datetime.date.fromisoformat(item.get("expires_on", "")):
            fail("iteration-2 risk acceptance expired")
    except (TypeError, ValueError):
        fail("iteration-2 risk acceptance expiry invalid")
    if item != expected:
        fail("iteration-2 risk acceptance policy drift")
    return item


def validate_advisory(vulnerability, policy):
    if not isinstance(vulnerability, dict) or vulnerability.get("id") not in policy["advisory_ids"]:
        fail("async-std advisory ID drift")
    if vulnerability.get("database_specific") != {"license": policy["advisory_license"]}:
        fail("async-std advisory license metadata drift")
    affected = vulnerability.get("affected")
    if not isinstance(affected, list):
        fail("async-std advisory affected metadata missing")
    matching = [item for item in affected if isinstance(item, dict)
                and item.get("package", {}).get("ecosystem") == "crates.io"
                and item.get("package", {}).get("name") == policy["name"]]
    if len(matching) != 1:
        fail("async-std advisory affected package drift")
    classification = matching[0].get("database_specific")
    if classification != {
        "categories": [],
        "cvss": None,
        "informational": policy["advisory_type"]["informational"],
        "source": policy["advisory_source"],
    }:
        fail("async-std advisory informational classification drift")
    ranges = matching[0].get("ranges")
    if ranges != [{"type": "SEMVER", "events": [{"introduced": "0.0.0-0"}]}]:
        fail("async-std advisory affected range drift")
    patched = [event["fixed"] for item in ranges for event in item["events"] if "fixed" in event]
    if patched != policy["patched_versions"]:
        fail("async-std advisory patched versions drift")


def evaluate(args):
    policy = exact_policy(read(args.supply_lock))
    vulnerabilities = read(args.vulnerabilities)
    reachability = read(args.reachability)
    reachable = {(item.get("name"), item.get("version")) for item in reachability.get("reachable", [])}
    key = (policy["name"], policy["version"])
    accepted = []
    for result in vulnerabilities.get("results", []):
        for package in result.get("packages", []):
            identity = package.get("package", {})
            package_key = (identity.get("name"), identity.get("version"))
            if identity.get("ecosystem") == "crates.io" and package_key == key:
                for vulnerability in package.get("vulnerabilities", []) or []:
                    accepted.append(vulnerability)
    if accepted:
        accepted_ids = [str(vulnerability.get("id", "UNKNOWN")) for vulnerability in accepted]
        if accepted_ids != policy["advisory_ids"]:
            fail("async-std advisory set drift")
        for vulnerability in accepted:
            validate_advisory(vulnerability, policy)
        if key not in reachable:
            fail("accepted-unfixed package is not reachable")
        chain_path = pathlib.Path(args.chain)
        if not chain_path.is_file():
            fail("accepted-unfixed dependency chain missing")
        chain = chain_path.read_text(encoding="utf-8")
        lines = {line.strip() for line in chain.splitlines() if line.strip()}
        if not any(re.match(r"^async-std v1\.13\.2(?:\s|$)", line) for line in lines) or not any(re.match(r"^minio v0\.4\.0(?:\s|$)", line) for line in lines):
            fail("accepted-unfixed minio dependency chain drift")
    candidate = vulnerabilities.get("candidate")
    if not isinstance(candidate, dict):
        fail("vulnerability candidate binding missing")
    document = {
        "schema_version": 1,
        "candidate": candidate,
        "status": "accepted-unfixed" if accepted else "none",
        "acceptances": [{
            "id": policy["id"], "purl": policy["purl"], "status": policy["status"],
            "reassess_by": policy["reassess_by"], "reachable_via": policy["reachable_via"],
            "vulnerability_ids": policy["advisory_ids"],
        }] if accepted else [],
        "inputs": {
            "vulnerabilities_sha256": hashlib.sha256(pathlib.Path(args.vulnerabilities).read_bytes()).hexdigest(),
            "reachability_sha256": hashlib.sha256(pathlib.Path(args.reachability).read_bytes()).hexdigest(),
            "chain_sha256": hashlib.sha256(pathlib.Path(args.chain).read_bytes()).hexdigest() if accepted else None,
        },
    }
    return document


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=("generate", "verify"))
    parser.add_argument("--supply-lock", required=True); parser.add_argument("--vulnerabilities", required=True)
    parser.add_argument("--reachability", required=True); parser.add_argument("--chain", required=True); parser.add_argument("--output", required=True)
    args = parser.parse_args(); document = evaluate(args); path = pathlib.Path(args.output)
    encoded = json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n"
    if args.command == "generate": path.write_text(encoded, encoding="utf-8")
    elif not path.is_file() or path.read_text(encoding="utf-8") != encoded: fail("accepted-unfixed evidence drift")


if __name__ == "__main__":
    main()
