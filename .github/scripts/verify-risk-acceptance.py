#!/usr/bin/env python3
import argparse, hashlib, json, pathlib, sys


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
    if items != []:
        fail("active risk acceptance set must be empty")


def evaluate(args):
    exact_policy(read(args.supply_lock))
    vulnerabilities = read(args.vulnerabilities)
    read(args.reachability)
    candidate = vulnerabilities.get("candidate")
    if not isinstance(candidate, dict):
        fail("vulnerability candidate binding missing")
    document = {
        "schema_version": 1,
        "candidate": candidate,
        "status": "none",
        "acceptances": [],
        "inputs": {
            "vulnerabilities_sha256": hashlib.sha256(pathlib.Path(args.vulnerabilities).read_bytes()).hexdigest(),
            "reachability_sha256": hashlib.sha256(pathlib.Path(args.reachability).read_bytes()).hexdigest(),
            "chain_sha256": None,
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
