#!/usr/bin/env python3

import argparse
import base64
import concurrent.futures
import hashlib
import io
import json
import pathlib
import re
import sys
import tarfile
import time
import tomllib
import urllib.parse
import urllib.error
import urllib.request
import zipfile

GENERATOR = {"name": "ficant-license-inventory", "version": 2}
ECOSYSTEMS = {"cargo": "crates.io", "pypi": "PyPI", "npm": "npm"}


def fail(message):
    print(f"license-inventory: {message}", file=sys.stderr)
    raise SystemExit(2)


def read_json(path):
    try:
        return json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    except Exception as exc:
        fail(f"invalid JSON {path}: {exc}")


def sha256(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def native_lf_bytes(path):
    payload = pathlib.Path(path).read_bytes()
    normalized = payload.replace(b"\r\n", b"\n")
    if b"\r" in normalized:
        fail(f"unsupported text line ending: {path}")
    return normalized


def native_lf_sha256(path):
    return hashlib.sha256(native_lf_bytes(path)).hexdigest()


def checkout_independent_tree_sha256(path):
    payload = pathlib.Path(path).read_bytes()
    if b"\0" not in payload:
        try:
            payload.decode("utf-8")
            payload = payload.replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
        except UnicodeDecodeError:
            pass
    return hashlib.sha256(payload).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def inventory_digest(packages):
    return hashlib.sha256(canonical(packages)).hexdigest()


def tree_integrity(root, relative):
    base = pathlib.Path(root) / relative; entries = []
    if not base.is_dir(): fail(f"first-party source missing: {relative}")
    for path in sorted(item for item in base.rglob("*") if item.is_file()):
        rel = path.relative_to(base)
        if any(part in {".git", "target", "node_modules", ".venv", "__pycache__", ".pytest_cache"} for part in rel.parts): continue
        entries.append({"path": rel.as_posix(), "sha256": checkout_independent_tree_sha256(path)})
    return "sha256:" + hashlib.sha256(canonical(entries)).hexdigest()


def package_key(artifact):
    purl = artifact.get("purl")
    name = artifact.get("name")
    version = artifact.get("version")
    if not all(isinstance(value, str) and value for value in (purl, name, version)):
        fail("Syft package key is incomplete")
    match = re.match(r"^pkg:(cargo|pypi|npm)/", purl)
    if not match:
        return None
    return {"purl": purl, "ecosystem": ECOSYSTEMS[match.group(1)], "name": name, "version": version}


def syft_keys(path):
    document = read_json(path)
    artifacts = document.get("artifacts")
    if not isinstance(artifacts, list):
        fail("Syft artifacts missing")
    keys = [key for artifact in artifacts if (key := package_key(artifact)) is not None]
    encoded = [canonical(key) for key in keys]
    if len(encoded) != len(set(encoded)):
        fail("duplicate Syft package key")
    return sorted(keys, key=lambda item: (item["ecosystem"], item["name"], item["version"], item["purl"]))


def cargo_sources(path):
    data = tomllib.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    result = {}
    for package in data.get("package", []):
        checksum = package.get("checksum")
        if checksum:
            result[(package["name"], package["version"])] = {
                "integrity": f"sha256:{checksum}",
                "locator": f"https://crates.io/api/v1/crates/{urllib.parse.quote(package['name'], safe='')}/{package['version']}/download",
            }
    return result


def uv_sources(path):
    data = tomllib.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    result = {}
    for package in data.get("package", []):
        artifacts = []
        if isinstance(package.get("sdist"), dict):
            artifacts.append(package["sdist"])
        artifacts.extend(package.get("wheels", []))
        pairs = {(item.get("url"), item.get("hash")) for item in artifacts if item.get("url") and item.get("hash")}
        if pairs:
            result[(package["name"], package["version"])] = pairs
    return result


def pnpm_sources(path):
    lines = pathlib.Path(path).read_text(encoding="utf-8").splitlines()
    result = {}
    in_packages = False
    current = None
    for line in lines:
        if line == "packages:":
            in_packages = True
            continue
        if in_packages and line and not line.startswith(" "):
            break
        match = re.match(r"^  (?:'([^']+)'|([^:]+)):$", line)
        if in_packages and match:
            raw = (match.group(1) or match.group(2)).split("(", 1)[0]
            if "@" not in raw:
                current = None
                continue
            name, version = raw.rsplit("@", 1)
            current = (name, version)
            continue
        integrity = re.search(r"integrity:\s*([^,}\s]+)", line)
        if in_packages and current and integrity:
            value = integrity.group(1)
            encoded = urllib.parse.quote(current[0], safe="")
            result[current] = {
                "integrity": value,
                "locator": f"https://registry.npmjs.org/{encoded}/-/{current[0].split('/')[-1]}-{current[1]}.tgz",
            }
    return result


def sources(cargo_lock, uv_lock, pnpm_lock):
    return {
        "crates.io": cargo_sources(cargo_lock),
        "PyPI": uv_sources(uv_lock),
        "npm": pnpm_sources(pnpm_lock),
    }


def source_for(key, source_maps):
    value = source_maps[key["ecosystem"]].get((key["name"], key["version"]))
    if value is None:
        fail(f"package missing lock source integrity: {key['purl']}")
    if key["ecosystem"] == "PyPI":
        return value
    return {(value["locator"], value["integrity"])}


def source_matches(key, locator, integrity, locked):
    if key["ecosystem"] != "npm":
        return (locator, integrity) in locked
    parsed = urllib.parse.urlparse(str(locator))
    expected_integrities = {item[1] for item in locked}
    package_file = f"{key['name'].split('/')[-1]}-{key['version']}.tgz"
    return (integrity in expected_integrities and parsed.scheme == "https"
            and parsed.netloc == "registry.npmjs.org" and parsed.path.endswith("/" + package_file))


def fetch_json(url):
    request = urllib.request.Request(url, headers={"User-Agent": "ficant-license-inventory/1"})
    for attempt in range(1, 4):
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                return json.load(response)
        except (OSError, urllib.error.URLError):
            if attempt == 3:
                raise
            time.sleep(attempt)


def license_text_from_artifact(url, integrity):
    payload = None
    for attempt in range(1, 4):
        try:
            with urllib.request.urlopen(url, timeout=30) as response:
                payload = response.read()
            break
        except (OSError, urllib.error.URLError):
            if attempt == 3:
                raise
            time.sleep(attempt)
    if not integrity.startswith("sha256:") or hashlib.sha256(payload).hexdigest() != integrity.split(":", 1)[1]:
        fail("primary license artifact integrity mismatch")
    files = []
    try:
        with tarfile.open(fileobj=io.BytesIO(payload), mode="r:*") as archive:
            for member in archive.getmembers():
                if member.isfile() and re.search(r"(^|/)(license|copying)(\.[^/]*)?$", member.name, re.I):
                    extracted = archive.extractfile(member)
                    if extracted: files.append(extracted.read().decode("utf-8", "replace"))
    except tarfile.TarError:
        try:
            with zipfile.ZipFile(io.BytesIO(payload)) as archive:
                for name in archive.namelist():
                    if re.search(r"(^|/)(license|copying)(\.[^/]*)?$", name, re.I):
                        files.append(archive.read(name).decode("utf-8", "replace"))
        except zipfile.BadZipFile:
            fail("primary license artifact is not a supported archive")
    return "\n".join(files).lower()


def locked_payload(url, integrity):
    payload = None
    for attempt in range(1, 4):
        try:
            with urllib.request.urlopen(url, timeout=30) as response: payload = response.read()
            break
        except (OSError, urllib.error.URLError):
            if attempt == 3: raise
            time.sleep(attempt)
    if integrity.startswith("sha256:"):
        actual = hashlib.sha256(payload).hexdigest(); expected = integrity.split(":", 1)[1]
    elif integrity.startswith("sha512-"):
        actual = base64.b64encode(hashlib.sha512(payload).digest()).decode(); expected = integrity.split("-", 1)[1]
    else:
        fail("unsupported source integrity algorithm")
    if actual != expected: fail("notice source integrity mismatch")
    return payload


def artifact_license_text(url, integrity):
    payload = locked_payload(url, integrity); texts = []
    try:
        with tarfile.open(fileobj=io.BytesIO(payload), mode="r:*") as archive:
            for member in archive.getmembers():
                if member.isfile() and re.search(r"(^|/)(license|copying|notice)(\.[^/]*)?$", member.name, re.I):
                    extracted = archive.extractfile(member)
                    if extracted: texts.append(extracted.read().decode("utf-8", "replace").strip())
    except tarfile.TarError:
        fail("notice source is not a supported archive")
    if not texts: fail("notice source contains no license text")
    return "\n\n".join(sorted(set(texts)))


def notice_document(supply_lock):
    data = read_json(supply_lock); lines = ["# 第三方许可证与署名", "", "本文件由固定 source asset 机械生成；例外仅适用于下列精确包、版本和完整性，不扩大全局许可证允许表。", ""]
    for item in data.get("license_scoped_exceptions", []):
        text = artifact_license_text(item["source_locator"], item["source_integrity"])
        digest_value = hashlib.sha256(text.encode()).hexdigest()
        if digest_value != item.get("license_text_sha256"): fail(f"license text drift: {item['purl']}")
        display_text = "\n".join(line.rstrip() for line in text.splitlines())
        lines.extend([f"## {item['purl']}", "", f"- SPDX：`{item['license_expression']}`", f"- 来源：`{item['source_locator']}`", f"- 完整性：`{item['source_integrity']}`", f"- 署名：{item['attribution']}", f"- 许可证文本 SHA-256：`{digest_value}`", "", "```text", display_text, "```", ""])
    return "\n".join(lines)


def notices(args):
    pathlib.Path(args.output).write_text(notice_document(args.supply_lock), encoding="utf-8")


def verify_notices(args):
    if pathlib.Path(args.notice).read_text(encoding="utf-8") != notice_document(args.supply_lock): fail("tracked third-party notice drift")


def pypi_license(info, artifact_url, artifact_integrity):
    value = info.get("license_expression") or info.get("license")
    if isinstance(value, str) and value.strip():
        return value
    classifiers = set(info.get("classifiers") or [])
    exact = {
        "License :: OSI Approved :: MIT License": "MIT",
        "License :: OSI Approved :: Apache Software License": "Apache-2.0",
        "License :: OSI Approved :: Python Software Foundation License": "PSF-2.0",
        "License :: OSI Approved :: Mozilla Public License 2.0 (MPL 2.0)": "MPL-2.0",
        "License :: OSI Approved :: ISC License (ISCL)": "ISC",
    }
    matches = {spdx for classifier, spdx in exact.items() if classifier in classifiers}
    if len(matches) == 1:
        return matches.pop()
    if "License :: OSI Approved :: BSD License" in classifiers:
        text = license_text_from_artifact(artifact_url, artifact_integrity)
        if "redistribution and use in source and binary forms" in text and "neither the name" in text:
            return "BSD-3-Clause"
        if "redistribution and use in source and binary forms" in text:
            return "BSD-2-Clause"
    return None


def primary_metadata(key, locked_sources):
    name, version, ecosystem = key["name"], key["version"], key["ecosystem"]
    if ecosystem == "crates.io":
        document = fetch_json(f"https://crates.io/api/v1/crates/{urllib.parse.quote(name, safe='')}/{version}")
        item = document.get("version", {})
        return item.get("license"), f"https://crates.io/api/v1/crates/{urllib.parse.quote(name, safe='')}/{version}/download", f"sha256:{item.get('checksum', '')}"
    if ecosystem == "PyPI":
        document = fetch_json(f"https://pypi.org/pypi/{urllib.parse.quote(name, safe='')}/{version}/json")
        info = document.get("info", {})
        available = {(item.get("url"), f"sha256:{item.get('digests', {}).get('sha256', '')}") for item in document.get("urls", [])}
        matches = sorted(locked_sources & available)
        if not matches:
            fail(f"PyPI primary artifact does not match uv lock: {key['purl']}")
        return pypi_license(info, matches[0][0], matches[0][1]), matches[0][0], matches[0][1]
    encoded = urllib.parse.quote(name, safe="")
    document = fetch_json(f"https://registry.npmjs.org/{encoded}/{version}")
    license_value = document.get("license")
    if isinstance(license_value, dict):
        license_value = license_value.get("type")
    dist = document.get("dist", {})
    return license_value, dist.get("tarball"), dist.get("integrity")


def normalize_license(value, subject="package"):
    if not isinstance(value, str) or not value.strip() or value.strip().upper() in {"NOASSERTION", "UNKNOWN", "UNLICENSED"}:
        fail(f"primary metadata license is unknown: {subject}")
    normalized = value.strip()
    aliases = {
        "3-Clause BSD License": "BSD-3-Clause",
        "BSD 3-Clause License": "BSD-3-Clause",
        "BSD-3-Clause License": "BSD-3-Clause",
        "MIT License": "MIT",
        "Apache License 2.0": "Apache-2.0",
        "Apache 2.0": "Apache-2.0",
    }
    normalized = aliases.get(normalized, normalized)
    legacy_dual = re.fullmatch(r"([A-Za-z0-9][A-Za-z0-9.+-]*)\s*/\s*([A-Za-z0-9][A-Za-z0-9.+-]*)", normalized)
    if legacy_dual:
        return f"{legacy_dual.group(1)} OR {legacy_dual.group(2)}"
    return normalized


def evaluate_spdx(expression, allowlist, subject="package"):
    tokens = []
    position = 0
    token_re = re.compile(r"\s*(AND|OR|WITH|\(|\)|[A-Za-z0-9][A-Za-z0-9.+-]*)")
    while position < len(expression):
        match = token_re.match(expression, position)
        if not match:
            fail(f"malformed SPDX expression: {subject}: {expression}")
        tokens.append(match.group(1)); position = match.end()
    index = 0
    def primary():
        nonlocal index
        if index >= len(tokens): fail(f"malformed SPDX expression: {subject}: {expression}")
        if tokens[index] == "(":
            index += 1; value = or_expression()
            if index >= len(tokens) or tokens[index] != ")": fail(f"malformed SPDX expression: {subject}: {expression}")
            index += 1; return value
        token = tokens[index]
        if token in {"AND", "OR", "WITH", ")"}: fail(f"malformed SPDX expression: {subject}: {expression}")
        index += 1
        if index < len(tokens) and tokens[index] == "WITH":
            index += 1
            if index >= len(tokens) or tokens[index] in {"AND", "OR", "WITH", "(", ")"}: fail(f"malformed SPDX expression: {subject}: {expression}")
            token = f"{token} WITH {tokens[index]}"; index += 1
        return token in allowlist
    def and_expression():
        nonlocal index
        value = primary()
        while index < len(tokens) and tokens[index] == "AND":
            index += 1; value = primary() and value
        return value
    def or_expression():
        nonlocal index
        value = and_expression()
        while index < len(tokens) and tokens[index] == "OR":
            index += 1; value = and_expression() or value
        return value
    if not tokens: fail(f"malformed SPDX expression: {subject}: {expression}")
    accepted = or_expression()
    if index != len(tokens): fail(f"malformed SPDX expression: {subject}: {expression}")
    if not accepted: fail(f"license expression disallowed: {subject}: {expression}")


def header(keys, packages, cargo_lock, uv_lock, pnpm_lock, supply_lock):
    lock_hashes = {"Cargo.lock": native_lf_sha256(cargo_lock), "python/uv.lock": native_lf_sha256(uv_lock), "web-dm/pnpm-lock.yaml": native_lf_sha256(pnpm_lock)}
    supply = read_json(supply_lock)
    syft = next(item for item in supply["tools"] if item["name"] == "syft")
    first_party_sources = sorted([
        {name: package[name] for name in ("purl", "source_locator", "source_integrity")}
        for package in packages if package.get("classification") == "first-party-internal"
    ], key=lambda item: item["purl"])
    input_digest = hashlib.sha256(canonical({"locks": lock_hashes, "package_keys": keys, "first_party_sources": first_party_sources})).hexdigest()
    return {
        "schema_version": 1,
        "generator": GENERATOR,
        "tool": {"name": "syft", "version": syft["version"], "sha256": syft["sha256"]},
        "lock_sha256": lock_hashes,
        "input_tree_digest": input_digest,
        "inventory_digest": inventory_digest(packages),
    }


def generate(args):
    keys = syft_keys(args.syft)
    source_maps = sources(args.cargo_lock, args.uv_lock, args.pnpm_lock)
    fixed_metadata = read_json(args.metadata) if args.metadata else None
    cache_root = pathlib.Path(args.cache_dir) if args.cache_dir else None
    if cache_root: cache_root.mkdir(parents=True, exist_ok=True)
    def resolve(key):
        locked = source_for(key, source_maps)
        if fixed_metadata is not None:
            item = fixed_metadata.get(key["purl"])
            if not isinstance(item, dict):
                fail(f"fixture metadata missing: {key['purl']}")
            license_value, locator, integrity = item.get("license"), item.get("source_locator"), item.get("integrity")
        else:
            cache_path = cache_root / (hashlib.sha256(key["purl"].encode()).hexdigest() + ".json") if cache_root else None
            cached = read_json(cache_path) if cache_path and cache_path.is_file() else None
            if cached:
                license_value, locator, integrity = cached.get("license"), cached.get("source_locator"), cached.get("integrity")
            else:
                license_value, locator, integrity = primary_metadata(key, locked)
                if cache_path:
                    cache_path.write_text(json.dumps({"purl": key["purl"], "license": license_value, "source_locator": locator, "integrity": integrity}, sort_keys=True, separators=(",", ":")) + "\n")
        if not source_matches(key, locator, integrity, locked):
            fail(f"primary source integrity mismatch: {key['purl']}")
        package = dict(key)
        package.update({"license_expression": normalize_license(license_value, key["purl"]), "source_locator": locator, "source_integrity": integrity})
        return package
    if fixed_metadata is not None:
        packages = [resolve(key) for key in keys]
    else:
        def guarded(key):
            try:
                return resolve(key), None
            except BaseException as exc:
                return None, f"{key['purl']}: {exc}"
        with concurrent.futures.ThreadPoolExecutor(max_workers=16) as executor:
            resolved = list(executor.map(guarded, keys))
        errors = [error for _, error in resolved if error]
        if errors:
            fail("primary metadata resolution failed: " + "; ".join(errors))
        packages = [package for package, _ in resolved]
    document = header(keys, packages, args.cargo_lock, args.uv_lock, args.pnpm_lock, args.supply_lock)
    document["packages"] = packages
    if args.unresolved_keys:
        unresolved = read_json(args.unresolved_keys)
        if not isinstance(unresolved, list) or not unresolved:
            fail("unresolved first-party key evidence invalid")
        document["status"] = "blocked_first_party_license_decision"
        document["unresolved_first_party_keys"] = unresolved
    else:
        document["status"] = "complete"
    pathlib.Path(args.output).write_text(json.dumps(document, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")


def finalize_first_party(args):
    document = read_json(args.inventory); keys = syft_keys(args.syft); supply = read_json(args.supply_lock)
    policy = supply.get("first_party_packages")
    if not isinstance(policy, list) or not policy: fail("first-party policy missing")
    expected_unresolved = sorted(document.get("unresolved_first_party_keys", []), key=lambda item: item["purl"])
    policy_keys = sorted([{name: item[name] for name in ("purl", "ecosystem", "name", "version")} for item in policy], key=lambda item: item["purl"])
    if expected_unresolved != policy_keys: fail("blocked first-party keys do not match frozen policy")
    packages = []
    for package in document.get("packages", []):
        item = dict(package)
        item["license_expression"] = normalize_license(item.get("license_expression"), item.get("purl", "package"))
        item["classification"] = "third-party"
        packages.append(item)
    for item in policy:
        package = {name: item[name] for name in ("purl", "ecosystem", "name", "version")}
        package.update({"classification": "first-party-internal", "authorization": "internal-no-open-source-grant", "source_locator": f"release-tree:{item['source']}", "source_integrity": tree_integrity(args.release_root, item["source"])})
        packages.append(package)
    packages.sort(key=lambda item: (item["ecosystem"], item["name"], item["version"], item["purl"]))
    package_keys = [{name: item[name] for name in ("purl", "ecosystem", "name", "version")} for item in packages]
    if package_keys != keys: fail("final first-party partition does not equal Syft universe")
    final = header(keys, packages, args.cargo_lock, args.uv_lock, args.pnpm_lock, args.supply_lock)
    final.update({"status": "complete", "first_party_packages": policy, "packages": packages})
    pathlib.Path(args.output).write_text(json.dumps(final, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")


def verify(args):
    if args.require_native_lf:
        for path in (args.cargo_lock, args.uv_lock, args.pnpm_lock):
            if pathlib.Path(path).read_bytes() != native_lf_bytes(path):
                fail(f"candidate archive input is not native LF: {path}")
    document = read_json(args.inventory)
    packages = document.get("packages")
    if not isinstance(packages, list):
        fail("inventory packages missing")
    keys = syft_keys(args.syft)
    inventory_keys = [{name: item.get(name) for name in ("purl", "ecosystem", "name", "version")} for item in packages if isinstance(item, dict)]
    if len(inventory_keys) != len(packages) or len({canonical(item) for item in inventory_keys}) != len(inventory_keys):
        fail("duplicate or invalid inventory key")
    if sorted(inventory_keys, key=lambda item: (item["ecosystem"], item["name"], item["version"], item["purl"])) != keys:
        fail("Syft and license inventory package keys differ")
    expected_header = header(keys, packages, args.cargo_lock, args.uv_lock, args.pnpm_lock, args.supply_lock)
    if {name: document.get(name) for name in expected_header} != expected_header:
        fail("inventory header or digest drift")
    supply = read_json(args.supply_lock)
    allowlist = set(supply["license_allowlist"])
    source_maps = sources(args.cargo_lock, args.uv_lock, args.pnpm_lock)
    first_policy = {item["purl"]: item for item in supply.get("first_party_packages", [])}
    if args.require_first_party:
        if document.get("status") != "complete" or document.get("first_party_packages") != supply.get("first_party_packages"):
            fail("first-party policy binding missing")
    for package in packages:
        if package.get("classification") == "first-party-internal":
            policy = first_policy.get(package.get("purl"))
            if not args.release_root or not policy or package.get("authorization") != "internal-no-open-source-grant" or package.get("source_locator") != f"release-tree:{policy['source']}" or package.get("source_integrity") != tree_integrity(args.release_root, policy["source"]):
                fail(f"first-party package binding mismatch: {package.get('purl')}")
            continue
        if args.require_first_party and package.get("classification") != "third-party": fail(f"third-party classification missing: {package.get('purl')}")
        locked = source_for(package, source_maps)
        if not source_matches(package, package.get("source_locator"), package.get("source_integrity"), locked):
            fail(f"source integrity mismatch: {package.get('purl')}")
    for package in packages:
        if package.get("classification") == "first-party-internal": continue
        scoped = [item for item in supply.get("license_scoped_exceptions", []) if item.get("purl") == package.get("purl")]
        scoped_allow = set()
        for item in scoped:
            expected = {name: package.get(name) for name in ("purl", "name", "version", "source_locator", "source_integrity", "license_expression")}
            if any(item.get(name) != value for name, value in expected.items()): fail(f"scoped license exception drift: {package.get('purl')}")
            scoped_allow.add(item["license_expression"])
        evaluate_spdx(normalize_license(package.get("license_expression"), package.get("purl", "package")), allowlist | scoped_allow, package.get("purl", "package"))
    print(document["inventory_digest"])


def digest(args):
    document = read_json(args.inventory)
    packages = document.get("packages")
    if not isinstance(packages, list):
        fail("inventory packages missing")
    actual = inventory_digest(packages)
    if document.get("inventory_digest") != actual:
        fail("inventory digest mismatch")
    print(actual)


def verify_provenance(args):
    document = read_json(args.inventory)
    provenance = read_json(args.provenance)
    digest_value = inventory_digest(document.get("packages", []))
    topology = provenance.get("topology", {})
    binding = provenance.get("license_inventory", {})
    if topology.get("candidate") != args.candidate or topology.get("candidate_tree") != args.tree or binding.get("digest") != digest_value:
        fail("runtime provenance license binding mismatch")


def parser():
    result = argparse.ArgumentParser()
    sub = result.add_subparsers(dest="command", required=True)
    for name in ("generate", "verify"):
        item = sub.add_parser(name)
        item.add_argument("--syft", required=True); item.add_argument("--cargo-lock", required=True)
        item.add_argument("--uv-lock", required=True); item.add_argument("--pnpm-lock", required=True)
        item.add_argument("--supply-lock", required=True)
        if name == "generate":
            item.add_argument("--metadata"); item.add_argument("--cache-dir"); item.add_argument("--unresolved-keys"); item.add_argument("--output", required=True)
        else:
            item.add_argument("--inventory", required=True); item.add_argument("--release-root"); item.add_argument("--require-first-party", action="store_true"); item.add_argument("--require-native-lf", action="store_true")
    item = sub.add_parser("finalize-first-party")
    item.add_argument("--inventory", required=True); item.add_argument("--syft", required=True); item.add_argument("--release-root", required=True)
    item.add_argument("--cargo-lock", required=True); item.add_argument("--uv-lock", required=True); item.add_argument("--pnpm-lock", required=True); item.add_argument("--supply-lock", required=True); item.add_argument("--output", required=True)
    item = sub.add_parser("digest"); item.add_argument("--inventory", required=True)
    item = sub.add_parser("verify-provenance")
    item.add_argument("--inventory", required=True); item.add_argument("--provenance", required=True)
    item.add_argument("--candidate", required=True); item.add_argument("--tree", required=True)
    item = sub.add_parser("notices"); item.add_argument("--supply-lock", required=True); item.add_argument("--output", required=True)
    item = sub.add_parser("verify-notices"); item.add_argument("--supply-lock", required=True); item.add_argument("--notice", required=True)
    return result


def main():
    args = parser().parse_args()
    {"generate": generate, "finalize-first-party": finalize_first_party, "verify": verify, "digest": digest, "verify-provenance": verify_provenance, "notices": notices, "verify-notices": verify_notices}[args.command](args)


if __name__ == "__main__":
    main()
