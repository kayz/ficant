use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use syn::visit::{self, Visit};
use syn::{ItemUse, Path as SynPath, UseTree};

const FORBIDDEN_L2_MODULES: [&str; 4] =
    ["analytics", "curves", "futures_delivery", "futures_hedge"];

#[test]
fn research_sources_do_not_reference_l2_modules() {
    let research_root = workspace_root().join("crates/ficant-domain/src/research");
    let sources = rust_sources_under(&research_root);
    assert!(
        !sources.is_empty(),
        "architecture gate failed closed: no Rust sources found under {}",
        research_root.display()
    );

    let mut violations = Vec::new();
    for source_path in sources {
        let source = read_source(&source_path);
        violations.extend(scan_source(&source_path, &source));
    }

    assert!(
        violations.is_empty(),
        "L1 research must not reference L2 domain modules:\n{}",
        violations.join("\n")
    );
}

#[test]
fn syntax_gate_rejects_each_forbidden_route_and_accepts_primitives() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/r5d-layering");
    let forbidden = [
        ("normal_use.rs", "curves"),
        ("absolute_path.rs", "futures_delivery"),
        ("nested_path.rs", "futures_hedge"),
        ("facade_route.rs", "analytics"),
    ];

    for (fixture, expected_module) in forbidden {
        let path = fixture_root.join(fixture);
        let violations = scan_source(&path, &read_source(&path));
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains(expected_module)),
            "negative fixture {fixture} did not trip {expected_module}: {violations:?}"
        );
    }

    let legal_path = fixture_root.join("legal_primitives.rs");
    let legal_violations = scan_source(&legal_path, &read_source(&legal_path));
    assert!(
        legal_violations.is_empty(),
        "legal L0 primitives route was rejected: {legal_violations:?}"
    );
}

#[test]
fn cargo_metadata_matches_frozen_workspace_adjacency() {
    let root = workspace_root();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--offline",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .current_dir(&root)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "architecture gate failed closed: cargo metadata could not start in {}: {error}",
                root.display()
            )
        });
    assert!(
        output.status.success(),
        "architecture gate failed closed: cargo metadata exited {:?}:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let metadata: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("cargo metadata returned invalid JSON: {error}"));
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .expect("cargo metadata omitted packages array");
    let workspace_members = metadata
        .get("workspace_members")
        .and_then(Value::as_array)
        .expect("cargo metadata omitted workspace_members array")
        .iter()
        .map(|member| {
            member
                .as_str()
                .expect("workspace member id must be a string")
                .to_owned()
        })
        .collect::<BTreeSet<_>>();

    let expected = expected_workspace_adjacency();
    assert_eq!(
        expected.len(),
        18,
        "R6B freezes the workspace architecture at exactly 18 Cargo packages"
    );
    let mut actual = BTreeMap::new();
    let mut package_ids = BTreeSet::new();
    for package in packages {
        let name = package
            .get("name")
            .and_then(Value::as_str)
            .expect("cargo metadata package omitted name")
            .to_owned();
        let id = package
            .get("id")
            .and_then(Value::as_str)
            .expect("cargo metadata package omitted id")
            .to_owned();
        assert!(
            package_ids.insert(id),
            "cargo metadata returned a duplicate package id"
        );
        assert!(
            expected.contains_key(&name),
            "unknown workspace package {name}; update requires an explicit architecture decision"
        );

        let mut edges = BTreeSet::new();
        for dependency in package
            .get("dependencies")
            .and_then(Value::as_array)
            .expect("cargo metadata package omitted dependencies array")
        {
            if dependency.get("path").is_none_or(Value::is_null) {
                continue;
            }
            let dependency_name = dependency
                .get("name")
                .and_then(Value::as_str)
                .expect("path dependency omitted name");
            if expected.contains_key(dependency_name) {
                edges.insert(dependency_name.to_owned());
            }
        }
        assert!(
            actual.insert(name.clone(), edges).is_none(),
            "cargo metadata returned duplicate workspace package name {name}"
        );
    }

    assert_eq!(
        package_ids, workspace_members,
        "--no-deps package set must exactly match workspace_members"
    );
    assert_eq!(
        actual, expected,
        "workspace package or dependency edge drifted; update requires an explicit architecture decision"
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("contract test manifest must live under <workspace>/crates")
        .to_path_buf()
}

fn rust_sources_under(root: &Path) -> Vec<PathBuf> {
    fn collect(directory: &Path, sources: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(directory).unwrap_or_else(|error| {
            panic!(
                "architecture gate failed closed: cannot read {}: {error}",
                directory.display()
            )
        });
        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!(
                    "architecture gate failed closed: cannot inspect {}: {error}",
                    directory.display()
                )
            });
            let path = entry.path();
            let file_type = entry.file_type().unwrap_or_else(|error| {
                panic!(
                    "architecture gate failed closed: cannot inspect {}: {error}",
                    path.display()
                )
            });
            if file_type.is_dir() {
                collect(&path, sources);
            } else if file_type.is_file() && path.extension() == Some(OsStr::new("rs")) {
                sources.push(path);
            }
        }
    }

    let mut sources = Vec::new();
    collect(root, &mut sources);
    sources.sort();
    sources
}

fn read_source(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| {
        panic!(
            "architecture gate failed closed: cannot read {}: {error}",
            path.display()
        )
    })
}

fn scan_source(path: &Path, source: &str) -> Vec<String> {
    let syntax = syn::parse_file(source).unwrap_or_else(|error| {
        panic!(
            "architecture gate failed closed: cannot parse {}: {error}",
            path.display()
        )
    });
    let mut visitor = LayerVisitor {
        path,
        violations: BTreeSet::new(),
    };
    visitor.visit_file(&syntax);
    visitor.violations.into_iter().collect()
}

struct LayerVisitor<'a> {
    path: &'a Path,
    violations: BTreeSet<String>,
}

impl LayerVisitor<'_> {
    fn inspect_use_tree(&mut self, tree: &UseTree, prefix: &mut Vec<String>) {
        match tree {
            UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.inspect_segments(prefix);
                self.inspect_use_tree(&path.tree, prefix);
                prefix.pop();
            }
            UseTree::Name(name) => {
                prefix.push(name.ident.to_string());
                self.inspect_segments(prefix);
                prefix.pop();
            }
            UseTree::Rename(rename) => {
                prefix.push(rename.ident.to_string());
                self.inspect_segments(prefix);
                prefix.pop();
            }
            UseTree::Glob(_) => self.inspect_segments(prefix),
            UseTree::Group(group) => {
                for item in &group.items {
                    self.inspect_use_tree(item, prefix);
                }
            }
        }
    }

    fn inspect_segments(&mut self, segments: &[String]) {
        if let Some(module) = forbidden_module(segments) {
            self.violations.insert(format!(
                "{}: forbidden L1 -> L2 route {} (module {module})",
                self.path.display(),
                segments.join("::")
            ));
        }
    }
}

impl<'ast> Visit<'ast> for LayerVisitor<'_> {
    fn visit_item_use(&mut self, node: &'ast ItemUse) {
        self.inspect_use_tree(&node.tree, &mut Vec::new());
        visit::visit_item_use(self, node);
    }

    fn visit_path(&mut self, node: &'ast SynPath) {
        let segments = node
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        self.inspect_segments(&segments);
        visit::visit_path(self, node);
    }
}

fn forbidden_module(segments: &[String]) -> Option<&str> {
    let candidate = match segments.first().map(String::as_str) {
        Some("crate" | "ficant_domain") => segments.get(1).map(String::as_str),
        Some("super") => segments
            .iter()
            .map(String::as_str)
            .find(|segment| *segment != "super"),
        _ => None,
    }?;
    FORBIDDEN_L2_MODULES
        .contains(&candidate)
        .then_some(candidate)
}

#[allow(clippy::too_many_lines)]
fn expected_workspace_adjacency() -> BTreeMap<String, BTreeSet<String>> {
    const EXPECTED: &[(&str, &[&str])] = &[
        (
            "ficant-acceptance",
            &[
                "ficant-application",
                "ficant-domain",
                "ficant-runtime",
                "ficant-storage",
            ],
        ),
        (
            "ficant-api",
            &[
                "ficant-application",
                "ficant-cgb-futures-pack",
                "ficant-contracts",
                "ficant-data",
                "ficant-domain",
                "ficant-fixed-income-native",
                "ficant-funding-pack",
                "ficant-runtime",
                "ficant-tax-pack",
            ],
        ),
        ("ficant-application", &["ficant-domain", "ficant-runtime"]),
        ("ficant-bootstrap", &[]),
        (
            "ficant-cgb-futures-pack",
            &["ficant-application", "ficant-contracts", "ficant-domain"],
        ),
        ("ficant-contract-tests", &["ficant-contracts"]),
        ("ficant-contracts", &[]),
        (
            "ficant-data",
            &["ficant-application", "ficant-domain", "ficant-storage"],
        ),
        ("ficant-domain", &[]),
        (
            "ficant-fixed-income-native",
            &[
                "ficant-application",
                "ficant-cgb-futures-pack",
                "ficant-domain",
                "ficant-kernel-sys",
            ],
        ),
        (
            "ficant-funding-pack",
            &["ficant-application", "ficant-contracts", "ficant-domain"],
        ),
        ("ficant-kernel-sys", &[]),
        (
            "ficant-native-nodes",
            &[
                "ficant-api",
                "ficant-contracts",
                "ficant-domain",
                "ficant-fixed-income-native",
                "ficant-runtime",
            ],
        ),
        ("ficant-runtime", &["ficant-domain"]),
        (
            "ficant-server",
            &[
                "ficant-api",
                "ficant-application",
                "ficant-cgb-futures-pack",
                "ficant-contracts",
                "ficant-domain",
                "ficant-fixed-income-native",
                "ficant-funding-pack",
                "ficant-native-nodes",
                "ficant-runtime",
                "ficant-storage",
                "ficant-tax-pack",
            ],
        ),
        (
            "ficant-storage",
            &[
                "ficant-application",
                "ficant-contracts",
                "ficant-domain",
                "ficant-fixed-income-native",
                "ficant-runtime",
            ],
        ),
        (
            "ficant-tax-pack",
            &["ficant-application", "ficant-contracts", "ficant-domain"],
        ),
        (
            "ficant-worker",
            &[
                "ficant-api",
                "ficant-application",
                "ficant-bootstrap",
                "ficant-contracts",
                "ficant-domain",
                "ficant-native-nodes",
                "ficant-runtime",
                "ficant-server",
                "ficant-storage",
            ],
        ),
    ];

    EXPECTED
        .iter()
        .map(|(package, dependencies)| {
            (
                (*package).to_owned(),
                dependencies
                    .iter()
                    .map(|dependency| (*dependency).to_owned())
                    .collect(),
            )
        })
        .collect()
}
