#!/usr/bin/env python3

import importlib.util
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest


TOOL = pathlib.Path(__file__).resolve().parents[1] / "verify-license-inventory.py"
SPEC = importlib.util.spec_from_file_location("verify_license_inventory", TOOL)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class LicenseInventoryBindingsTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temporary.name)
        self.release = self.root / "release"
        (self.release / "internal-component").mkdir(parents=True)
        (self.release / "vendor-component").mkdir(parents=True)
        (self.release / "internal-component" / "source.txt").write_text("internal source\n", encoding="utf-8")
        (self.release / "vendor-component" / "source.txt").write_text("vendored source\n", encoding="utf-8")
        self.cargo_lock = self.root / "Cargo.lock"
        self.uv_lock = self.root / "uv.lock"
        self.pnpm_lock = self.root / "pnpm-lock.yaml"
        self.cargo_lock.write_text(
            'version = 4\n\n[[package]]\nname = "internal-component"\nversion = "0.1.0"\n\n'
            '[[package]]\nname = "vendor-component"\nversion = "1.0.0"\n',
            encoding="utf-8",
        )
        self.uv_lock.write_text("version = 1\n", encoding="utf-8")
        self.pnpm_lock.write_text("lockfileVersion: '9.0'\npackages:\n", encoding="utf-8")
        self.first_party = {
            "name": "internal-component",
            "version": "0.1.0",
            "purl": "pkg:cargo/internal-component@0.1.0",
            "ecosystem": "crates.io",
            "source": "internal-component",
        }
        self.vendored = {
            "name": "vendor-component",
            "version": "1.0.0",
            "purl": "pkg:cargo/vendor-component@1.0.0",
            "ecosystem": "crates.io",
            "source": "vendor-component",
            "license_expression": "Apache-2.0",
            "upstream_source_locator": "https://crates.io/api/v1/crates/vendor-component/1.0.0/download",
            "upstream_source_integrity": "sha256:" + ("d" * 64),
        }
        self.supply_lock = self.root / "supply.json"
        self.supply_lock.write_text(
            json.dumps(
                {
                    "tools": [{"name": "syft", "version": "1.46.0", "sha256": "0" * 64}],
                    "license_allowlist": ["MIT", "Apache-2.0"],
                    "license_scoped_exceptions": [],
                    "first_party_packages": [self.first_party],
                    "vendored_third_party_packages": [self.vendored],
                },
                sort_keys=True,
                separators=(",", ":"),
            )
            + "\n",
            encoding="utf-8",
        )
        self.inventory = self.root / "inventory.json"
        packages = [
            {
                "classification": MODULE.FIRST_PARTY_CLASSIFICATION,
                "ecosystem": "crates.io",
                "license_expression": MODULE.FIRST_PARTY_LICENSE,
                "name": "internal-component",
                "purl": self.first_party["purl"],
                "source_integrity": MODULE.tree_integrity(self.release, self.first_party["source"]),
                "source_locator": "release-tree:internal-component",
                "version": "0.1.0",
            },
            {
                "classification": "third-party",
                "ecosystem": "crates.io",
                "license_expression": "Apache-2.0",
                "name": "vendor-component",
                "purl": self.vendored["purl"],
                "source_integrity": MODULE.tree_integrity(self.release, self.vendored["source"]),
                "source_locator": "release-tree:vendor-component",
                "version": "1.0.0",
            },
        ]
        keys = [
            {name: package[name] for name in ("purl", "ecosystem", "name", "version")}
            for package in packages
        ]
        document = MODULE.header(
            keys,
            packages,
            self.cargo_lock,
            self.uv_lock,
            self.pnpm_lock,
            self.supply_lock,
        )
        document.update(
            {
                "status": "complete",
                "first_party_packages": [self.first_party],
                "packages": packages,
            }
        )
        self.inventory.write_text(
            json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )

    def tearDown(self):
        self.temporary.cleanup()

    def command(self, command="verify-bindings", inventory=None, output=None):
        arguments = [
            sys.executable,
            str(TOOL),
            command,
            "--inventory",
            str(inventory or self.inventory),
            "--cargo-lock",
            str(self.cargo_lock),
            "--uv-lock",
            str(self.uv_lock),
            "--pnpm-lock",
            str(self.pnpm_lock),
            "--supply-lock",
            str(self.supply_lock),
            "--release-root",
            str(self.release),
        ]
        if output:
            arguments.extend(["--output", str(output)])
        return subprocess.run(arguments, capture_output=True, text=True, check=False)

    def test_valid_bindings_pass(self):
        result = self.command()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_cargo_lock_drift_fails(self):
        self.cargo_lock.write_text(self.cargo_lock.read_text(encoding="utf-8") + "\n# drift\n", encoding="utf-8")
        result = self.command()
        self.assertEqual(result.returncode, 2)
        self.assertIn("inventory header or digest drift", result.stderr)

    def test_first_party_source_drift_fails(self):
        (self.release / "internal-component" / "source.txt").write_text("changed\n", encoding="utf-8")
        result = self.command()
        self.assertEqual(result.returncode, 2)
        self.assertIn("first-party package binding mismatch", result.stderr)

    def test_lf_and_crlf_are_checkout_independent(self):
        for path in (
            self.cargo_lock,
            self.uv_lock,
            self.pnpm_lock,
            self.release / "internal-component" / "source.txt",
            self.release / "vendor-component" / "source.txt",
        ):
            payload = path.read_bytes().replace(b"\r\n", b"\n")
            path.write_bytes(payload.replace(b"\n", b"\r\n"))
        result = self.command()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_canonical_header_fields_are_enforced(self):
        original = json.loads(self.inventory.read_text(encoding="utf-8"))
        for field, value in (
            ("generator", {"name": "wrong", "version": 4}),
            ("tool", {"name": "syft", "version": "0", "sha256": "0" * 64}),
            ("inventory_digest", "0" * 64),
            ("input_tree_digest", "0" * 64),
        ):
            with self.subTest(field=field):
                candidate = dict(original)
                candidate[field] = value
                path = self.root / f"{field}.json"
                path.write_text(
                    json.dumps(candidate, sort_keys=True, separators=(",", ":")) + "\n",
                    encoding="utf-8",
                )
                result = self.command(inventory=path)
                self.assertEqual(result.returncode, 2)
                self.assertIn("inventory header or digest drift", result.stderr)

    def test_release_tree_policy_packages_cannot_disappear(self):
        original = json.loads(self.inventory.read_text(encoding="utf-8"))
        for purl, message in (
            (self.first_party["purl"], "first-party package set does not match frozen policy"),
            (self.vendored["purl"], "vendored third-party package set does not match frozen policy"),
        ):
            with self.subTest(purl=purl):
                candidate = dict(original)
                candidate["packages"] = [
                    dict(item) for item in original["packages"] if item["purl"] != purl
                ]
                keys = [
                    {name: package[name] for name in ("purl", "ecosystem", "name", "version")}
                    for package in candidate["packages"]
                ]
                candidate.update(
                    MODULE.header(
                        keys,
                        candidate["packages"],
                        self.cargo_lock,
                        self.uv_lock,
                        self.pnpm_lock,
                        self.supply_lock,
                    )
                )
                path = self.root / f"missing-{purl.rsplit('/', 1)[-1]}.json"
                path.write_text(
                    json.dumps(candidate, sort_keys=True, separators=(",", ":")) + "\n",
                    encoding="utf-8",
                )
                result = self.command(inventory=path)
                self.assertEqual(result.returncode, 2)
                self.assertIn(message, result.stderr)

    def test_refresh_bindings_only_updates_derived_bindings(self):
        before = json.loads(self.inventory.read_text(encoding="utf-8"))
        (self.release / "internal-component" / "source.txt").write_text("changed\n", encoding="utf-8")
        refreshed = self.root / "refreshed.json"
        result = self.command(command="refresh-bindings", output=refreshed)
        self.assertEqual(result.returncode, 0, result.stderr)
        verify = self.command(inventory=refreshed)
        self.assertEqual(verify.returncode, 0, verify.stderr)
        after = json.loads(refreshed.read_text(encoding="utf-8"))
        before_by_purl = {item["purl"]: item for item in before["packages"]}
        after_by_purl = {item["purl"]: item for item in after["packages"]}
        self.assertEqual(before_by_purl.keys(), after_by_purl.keys())
        changed = [
            purl
            for purl in before_by_purl
            if before_by_purl[purl] != after_by_purl[purl]
        ]
        self.assertEqual(changed, [self.first_party["purl"]])
        before_package = dict(before_by_purl[changed[0]])
        after_package = dict(after_by_purl[changed[0]])
        before_package.pop("source_integrity")
        after_package.pop("source_integrity")
        self.assertEqual(before_package, after_package)

    def test_refresh_rejects_license_policy_drift_without_overwrite(self):
        document = json.loads(self.inventory.read_text(encoding="utf-8"))
        vendored = next(
            item for item in document["packages"] if item["purl"] == self.vendored["purl"]
        )
        vendored["license_expression"] = "MIT"
        self.inventory.write_text(
            json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        before = self.inventory.read_bytes()
        result = self.command(command="refresh-bindings", output=self.inventory)
        self.assertEqual(result.returncode, 2)
        self.assertIn("vendored third-party package policy drift", result.stderr)
        self.assertEqual(self.inventory.read_bytes(), before)


class R5DFirstPartyPolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.repository = TOOL.parents[2]
        cls.inventory = cls.repository / ".github/scripts/license-inventory.lock.json"
        cls.supply_lock = cls.repository / ".github/scripts/supply-chain.lock.json"
        cls.cargo_lock = cls.repository / "Cargo.lock"
        cls.uv_lock = cls.repository / "python/uv.lock"
        cls.pnpm_lock = cls.repository / "web-dm/pnpm-lock.yaml"

    def command(self, inventory):
        return subprocess.run(
            [
                sys.executable,
                str(TOOL),
                "verify-bindings",
                "--inventory",
                str(inventory),
                "--cargo-lock",
                str(self.cargo_lock),
                "--uv-lock",
                str(self.uv_lock),
                "--pnpm-lock",
                str(self.pnpm_lock),
                "--supply-lock",
                str(self.supply_lock),
                "--release-root",
                str(self.repository),
                "--require-first-party",
            ],
            capture_output=True,
            text=True,
            check=False,
        )

    def candidate(self, packages, path):
        document = json.loads(self.inventory.read_text(encoding="utf-8"))
        document["packages"] = packages
        keys = [
            {name: package[name] for name in ("purl", "ecosystem", "name", "version")}
            for package in packages
        ]
        document.update(
            MODULE.header(
                keys,
                packages,
                self.cargo_lock,
                self.uv_lock,
                self.pnpm_lock,
                self.supply_lock,
            )
        )
        path.write_text(
            json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )

    def test_policy_is_exactly_nineteen_cargo_packages_plus_python_sdk(self):
        supply = json.loads(self.supply_lock.read_text(encoding="utf-8"))
        policy = supply["first_party_packages"]
        purls = [item["purl"] for item in policy]
        self.assertEqual(len(policy), 20)
        self.assertEqual(len(set(purls)), 20)
        self.assertEqual(sum(item["ecosystem"] == "crates.io" for item in policy), 19)
        self.assertEqual(
            [item["purl"] for item in policy if item["ecosystem"] == "PyPI"],
            ["pkg:pypi/ficant-sdk@0.1.0"],
        )

    def test_each_r5d_pack_is_required_by_the_exact_policy(self):
        original = json.loads(self.inventory.read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory() as temporary:
            for name in ("ficant-cgb-futures-pack", "ficant-funding-pack", "ficant-tax-pack"):
                purl = f"pkg:cargo/{name}@0.1.0"
                with self.subTest(purl=purl):
                    path = pathlib.Path(temporary) / f"missing-{name}.json"
                    packages = [
                        dict(item) for item in original["packages"] if item["purl"] != purl
                    ]
                    self.candidate(packages, path)
                    result = self.command(path)
                    self.assertEqual(result.returncode, 2)
                    self.assertIn(
                        "first-party package set does not match frozen policy", result.stderr
                    )

    def test_duplicate_first_party_purl_is_rejected(self):
        original = json.loads(self.inventory.read_text(encoding="utf-8"))
        packages = [dict(item) for item in original["packages"]]
        duplicate = next(
            item for item in packages if item["purl"] == "pkg:cargo/ficant-tax-pack@0.1.0"
        )
        packages.append(dict(duplicate))
        with tempfile.TemporaryDirectory() as temporary:
            path = pathlib.Path(temporary) / "duplicate-purl.json"
            self.candidate(packages, path)
            result = self.command(path)
        self.assertEqual(result.returncode, 2)
        self.assertIn("duplicate or invalid inventory key", result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
