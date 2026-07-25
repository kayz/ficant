#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import json
import pathlib
import shutil
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[3]
TOOL = ROOT / "deploy" / "verify-storage-runtime.py"
LOCK = ROOT / "deploy" / "storage-runtime.lock.json"


class StorageRuntimeLockTests(unittest.TestCase):
    def run_tool(self, *arguments: str, cwd: pathlib.Path = ROOT) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["python", str(TOOL), *arguments],
            cwd=cwd,
            text=True,
            capture_output=True,
            check=False,
        )

    def test_repository_lock_passes(self) -> None:
        result = self.run_tool("verify-lock", "--lock", str(LOCK), "--root", str(ROOT))
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_build_input_drift_fails(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            checkout = pathlib.Path(directory) / "checkout"
            subprocess.run(["git", "clone", "--quiet", str(ROOT), str(checkout)], check=True)
            shutil.copy2(TOOL, checkout / "deploy/verify-storage-runtime.py")
            shutil.copy2(LOCK, checkout / "deploy/storage-runtime.lock.json")
            path = checkout / "deploy/dev/Ceph.Dockerfile"
            path.write_text(path.read_text(encoding="utf-8") + "\n# drift\n", encoding="utf-8")
            result = self.run_tool(
                "verify-lock", "--lock", str(checkout / "deploy/storage-runtime.lock.json"),
                "--root", str(checkout), cwd=checkout,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("build inputs", result.stderr)

    def test_checkout_line_endings_do_not_drift(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            checkout = pathlib.Path(directory) / "checkout"
            subprocess.run(["git", "clone", "--quiet", str(ROOT), str(checkout)], check=True)
            shutil.copy2(TOOL, checkout / "deploy/verify-storage-runtime.py")
            shutil.copy2(LOCK, checkout / "deploy/storage-runtime.lock.json")
            for relative in (
                ".dockerignore",
                "deploy/dev/Ceph.Dockerfile",
                "deploy/dev/ceph-entrypoint.sh",
            ):
                path = checkout / relative
                lf = path.read_bytes().replace(b"\r\n", b"\n")
                path.write_bytes(lf.replace(b"\n", b"\r\n"))
            result = self.run_tool(
                "verify-lock", "--lock", str(checkout / "deploy/storage-runtime.lock.json"),
                "--root", str(checkout), cwd=checkout,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_oci_identity_and_size_are_verified(self) -> None:
        lock = json.loads(LOCK.read_text(encoding="utf-8"))
        manifest = {
            "schemaVersion": 2,
            "config": {"digest": lock["oci"]["config_digest"], "size": 1},
            "layers": [{"digest": "sha256:" + "1" * 64, "size": lock["oci"]["compressed_layers_bytes"]}],
        }
        manifest_raw = json.dumps(manifest, separators=(",", ":")).encode()
        manifest_digest = "sha256:" + hashlib.sha256(manifest_raw).hexdigest()
        index = {
            "schemaVersion": 2,
            "manifests": [{
                "digest": manifest_digest,
                "size": len(manifest_raw),
                "platform": {"architecture": "amd64", "os": "linux"},
            }],
        }
        index_raw = json.dumps(index, separators=(",", ":")).encode()
        lock["oci"]["platform_manifest_digest"] = manifest_digest
        lock["oci"]["index_digest"] = "sha256:" + hashlib.sha256(index_raw).hexdigest()
        with tempfile.TemporaryDirectory() as directory:
            temporary = pathlib.Path(directory)
            lock_path = temporary / "lock.json"
            index_path = temporary / "index.json"
            manifest_path = temporary / "manifest.json"
            lock_path.write_text(json.dumps(lock), encoding="utf-8")
            index_path.write_bytes(index_raw)
            manifest_path.write_bytes(manifest_raw)
            passing = self.run_tool(
                "verify-oci", "--lock", str(lock_path),
                "--index", str(index_path), "--manifest", str(manifest_path),
            )
            self.assertEqual(passing.returncode, 0, passing.stderr)
            lock["oci"]["config_digest"] = "sha256:" + "f" * 64
            lock_path.write_text(json.dumps(lock), encoding="utf-8")
            failing = self.run_tool(
                "verify-oci", "--lock", str(lock_path),
                "--index", str(index_path), "--manifest", str(manifest_path),
            )
            self.assertNotEqual(failing.returncode, 0)
            self.assertIn("config digest", failing.stderr)


if __name__ == "__main__":
    unittest.main()
