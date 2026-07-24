use std::env;
use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};

const DOMAIN: &[u8] = b"ficant/native-node-relevant-source/v1";
const SOURCES: &[(&str, &str)] = &[
    ("Cargo.toml", "Cargo.toml"),
    ("build.rs", "build.rs"),
    ("src/lib.rs", "src/lib.rs"),
    (
        "interface/proto/ficant/rates/v1/analytics.proto",
        "../../interface/proto/ficant/rates/v1/analytics.proto",
    ),
];

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory is set"));
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    for (canonical_label, relative) in SOURCES {
        let path = manifest_dir.join(relative);
        let contents = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        update_bytes(&mut hasher, canonical_label.as_bytes());
        update_bytes(&mut hasher, &contents);
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!(
        "cargo:rustc-env=FICANT_NATIVE_NODES_SOURCE_DIGEST={}",
        encode_hex(&hasher.finalize())
    );
}

fn update_bytes(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn encode_hex(value: &[u8]) -> String {
    use std::fmt::Write as _;

    value.iter().fold(
        String::with_capacity(value.len() * 2),
        |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        },
    )
}
