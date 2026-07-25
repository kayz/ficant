#!/usr/bin/env python3
"""Create and fail-closed verify the immutable Ceph storage-runtime lock."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import tempfile


BUILD_INPUTS = (
    ".dockerignore",
    "deploy/dev/Ceph.Dockerfile",
    "deploy/dev/ceph-entrypoint.sh",
)
SHA256 = re.compile(r"^sha256:[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
IMAGE = re.compile(r"^ghcr\.io/[a-z0-9_.-]+/[a-z0-9_.-]+$")


def fail(message: str) -> None:
    raise SystemExit(f"storage-runtime-lock: {message}")


def normalized_bytes(data: bytes) -> bytes:
    return data.replace(b"\r\n", b"\n")


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def input_bindings(root: pathlib.Path) -> tuple[list[dict[str, str]], str]:
    bindings = []
    for relative in BUILD_INPUTS:
        path = root / relative
        if not path.is_file():
            fail(f"missing build input: {relative}")
        bindings.append({"path": relative, "sha256": digest(normalized_bytes(path.read_bytes()))})
    return bindings, digest(canonical(bindings))


def load_lock(path: pathlib.Path) -> dict:
    try:
        lock = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        fail(f"cannot read {path}: {exc}")
    if lock.get("schema_version") != 1:
        fail("schema_version must be 1")
    if not IMAGE.fullmatch(str(lock.get("image", ""))):
        fail("image must be a canonical GHCR repository")
    if not COMMIT.fullmatch(str(lock.get("source_commit", ""))):
        fail("source_commit must be a full commit SHA")
    if lock.get("platform") != "linux/amd64":
        fail("platform must be linux/amd64")
    oci = lock.get("oci", {})
    for field in ("index_digest", "platform_manifest_digest", "config_digest"):
        if not SHA256.fullmatch(str(oci.get(field, ""))):
            fail(f"oci.{field} must be a canonical sha256 digest")
    if not isinstance(oci.get("compressed_layers_bytes"), int) or oci["compressed_layers_bytes"] <= 0:
        fail("oci.compressed_layers_bytes must be positive")
    return lock


def git_blob(root: pathlib.Path, commit: str, relative: str) -> bytes:
    result = subprocess.run(
        ["git", "-C", str(root), "show", f"{commit}:{relative}"],
        check=False,
        capture_output=True,
    )
    if result.returncode:
        fail(f"cannot resolve {relative} at source_commit {commit}")
    return normalized_bytes(result.stdout)


def verify_build(lock: dict, root: pathlib.Path) -> None:
    bindings, tree_digest = input_bindings(root)
    build = lock.get("build", {})
    if build.get("inputs") != bindings:
        fail("current Ceph build inputs do not match the lock")
    if build.get("input_digest") != tree_digest:
        fail("build.input_digest is not canonical")
    for item in bindings:
        source_sha = digest(git_blob(root, lock["source_commit"], item["path"]))
        if source_sha != item["sha256"]:
            fail(f"{item['path']} differs from source_commit")


def verify_oci_bytes(lock: dict, index_raw: bytes, manifest_raw: bytes) -> None:
    index = json.loads(index_raw)
    manifest = json.loads(manifest_raw)
    oci = lock["oci"]
    if f"sha256:{digest(index_raw)}" != oci["index_digest"]:
        fail("OCI index payload digest does not match")
    matches = [
        item for item in index.get("manifests", [])
        if item.get("platform") == {"architecture": "amd64", "os": "linux"}
    ]
    if len(matches) != 1 or matches[0].get("digest") != oci["platform_manifest_digest"]:
        fail("OCI index does not bind the expected linux/amd64 manifest")
    if f"sha256:{digest(manifest_raw)}" != oci["platform_manifest_digest"]:
        fail("OCI platform manifest payload digest does not match")
    if manifest.get("config", {}).get("digest") != oci["config_digest"]:
        fail("OCI config digest does not match")
    layer_bytes = sum(item.get("size", -1) for item in manifest.get("layers", []))
    if layer_bytes != oci["compressed_layers_bytes"]:
        fail("OCI compressed layer size does not match")


def inspect_raw(reference: str) -> bytes:
    result = subprocess.run(
        ["docker", "buildx", "imagetools", "inspect", "--raw", reference],
        check=False,
        capture_output=True,
    )
    if result.returncode:
        fail(f"cannot inspect OCI reference {reference}: {result.stderr.decode(errors='replace')}")
    return result.stdout


def verify_remote(lock: dict) -> None:
    image = lock["image"]
    oci = lock["oci"]
    index_raw = inspect_raw(f"{image}@{oci['index_digest']}")
    manifest_raw = inspect_raw(f"{image}@{oci['platform_manifest_digest']}")
    verify_oci_bytes(lock, index_raw, manifest_raw)


def refresh(lock_path: pathlib.Path, root: pathlib.Path) -> None:
    lock = load_lock(lock_path)
    bindings, tree_digest = input_bindings(root)
    for item in bindings:
        if digest(git_blob(root, lock["source_commit"], item["path"])) != item["sha256"]:
            fail(f"refusing refresh: {item['path']} differs from source_commit")
    lock["build"] = {"inputs": bindings, "input_digest": tree_digest}
    rendered = json.dumps(lock, indent=2, ensure_ascii=False) + "\n"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", newline="\n", dir=lock_path.parent, delete=False
    ) as handle:
        handle.write(rendered)
        temporary = pathlib.Path(handle.name)
    temporary.replace(lock_path)
    verify_build(load_lock(lock_path), root)
    print(f"storage-runtime-lock: refreshed {lock_path}")


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    for name in ("verify-lock", "refresh-build-bindings"):
        child = subparsers.add_parser(name)
        child.add_argument("--lock", type=pathlib.Path, required=True)
        child.add_argument("--root", type=pathlib.Path, default=pathlib.Path("."))
    oci = subparsers.add_parser("verify-oci")
    oci.add_argument("--lock", type=pathlib.Path, required=True)
    oci.add_argument("--index", type=pathlib.Path, required=True)
    oci.add_argument("--manifest", type=pathlib.Path, required=True)
    remote = subparsers.add_parser("verify-remote")
    remote.add_argument("--lock", type=pathlib.Path, required=True)
    args = parser.parse_args()
    root = args.root.resolve() if hasattr(args, "root") else pathlib.Path(".").resolve()
    lock = load_lock(args.lock)
    if args.command == "refresh-build-bindings":
        refresh(args.lock, root)
    elif args.command == "verify-lock":
        verify_build(lock, root)
        print("storage-runtime-lock: PASS")
    elif args.command == "verify-oci":
        verify_oci_bytes(lock, args.index.read_bytes(), args.manifest.read_bytes())
        print("storage-runtime-oci: PASS")
    else:
        verify_remote(lock)
        print("storage-runtime-remote: PASS")


if __name__ == "__main__":
    main()
