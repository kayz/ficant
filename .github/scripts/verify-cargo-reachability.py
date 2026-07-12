#!/usr/bin/env python3
import argparse, hashlib, json, pathlib, sys, tomllib

CONFIGURATION = {"locked": True, "all_features": True, "target": "all", "command": "cargo tree", "format": "{p}"}

def fail(message):
    print(f"cargo-reachability: {message}", file=sys.stderr)
    raise SystemExit(2)

def sha(path): return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()

def manifests(root):
    root = pathlib.Path(root)
    entries = []
    for path in sorted(root.rglob("Cargo.toml")):
        relative = path.relative_to(root)
        if any(part in {".git", "target", ".worktrees"} for part in relative.parts): continue
        entries.append({"path": relative.as_posix(), "sha256": sha(path)})
    if not entries: fail("Cargo manifests missing")
    return hashlib.sha256(json.dumps(entries, sort_keys=True, separators=(",", ":")).encode()).hexdigest()

def keys_from_lock(path):
    data = tomllib.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    return sorted({(p["name"], p["version"]) for p in data.get("package", [])})

def reachable_from_graph(path):
    import re
    result = set()
    for line in pathlib.Path(path).read_text(encoding="utf-8").splitlines():
        match = re.match(r"^([^ ]+) v([^ ]+)(?: .*)?$", line.strip())
        if match: result.add((match.group(1), match.group(2)))
    if not result: fail("Cargo resolved graph missing")
    return sorted(result)

def build(args):
    if not args.cargo_version.startswith("cargo 1.96.1 "):
        fail("Cargo tool identity drift")
    locked = keys_from_lock(args.cargo_lock)
    reachable = reachable_from_graph(args.resolved_graph)
    if any(key not in locked for key in reachable): fail("resolved package absent from Cargo.lock")
    unreachable = sorted(set(locked) - set(reachable))
    return {
        "schema_version": 1,
        "cargo_version": args.cargo_version,
        "configuration": CONFIGURATION,
        "cargo_lock_sha256": sha(args.cargo_lock),
        "manifests_digest": manifests(args.manifest_root),
        "resolved_graph_sha256": sha(args.resolved_graph),
        "reachable": [{"name": name, "version": version} for name, version in reachable],
        "unreachable_lock_only": [{"name": name, "version": version} for name, version in unreachable],
    }

def generate(args):
    pathlib.Path(args.output).write_text(json.dumps(build(args), sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")

def verify(args):
    actual = json.loads(pathlib.Path(args.evidence).read_text(encoding="utf-8"))
    expected = build(args)
    if actual != expected: fail("reachability evidence drift or forged classification")

def main():
    parser = argparse.ArgumentParser(); sub = parser.add_subparsers(dest="command", required=True)
    for command in ("generate", "verify"):
        item = sub.add_parser(command)
        item.add_argument("--resolved-graph", required=True); item.add_argument("--cargo-lock", required=True)
        item.add_argument("--manifest-root", required=True); item.add_argument("--cargo-version", required=True)
        if command == "generate": item.add_argument("--output", required=True)
        else: item.add_argument("--evidence", required=True)
    args = parser.parse_args(); {"generate": generate, "verify": verify}[args.command](args)

if __name__ == "__main__": main()
