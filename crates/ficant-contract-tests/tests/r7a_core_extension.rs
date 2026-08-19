use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST: &str = include_str!("fixtures/r7a-core-extension/core-source-sha256.tsv");
const RECURSIVE_ROOTS: [&str; 6] = [
    "crates/ficant-domain/src/primitives",
    "crates/ficant-domain/src/research",
    "crates/ficant-fixed-income-native/src",
    "crates/ficant-kernel-sys/src",
    "cpp/fixed-income-kernel/include",
    "cpp/fixed-income-kernel/src",
];
const INDIVIDUAL_FILES: [&str; 6] = [
    "crates/ficant-domain/src/subject.rs",
    "crates/ficant-domain/src/analytics.rs",
    "crates/ficant-domain/src/curves.rs",
    "crates/ficant-domain/src/futures_delivery.rs",
    "crates/ficant-domain/src/futures_hedge.rs",
    "crates/ficant-kernel-sys/build.rs",
];

#[test]
fn r7a_fictional_market_keeps_every_l0_l1_l2_production_source_exact() {
    let expected = parse_manifest();
    assert_eq!(
        expected.len(),
        47,
        "the R7A execution base freezes exactly 47 core production sources"
    );
    let actual = production_source_digests();
    assert_eq!(
        actual.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "the protected L0/L1/L2 production source set drifted"
    );
    assert_eq!(
        actual, expected,
        "a protected L0/L1/L2 production source changed after the R7A execution base"
    );
}

#[test]
fn core_manifest_rejects_a_real_single_bit_source_drift() {
    let root = workspace_root();
    let relative = "cpp/fixed-income-kernel/src/bond_math.cpp";
    let bytes = fs::read(root.join(relative)).expect("protected source is readable");
    let expected = parse_manifest()
        .remove(relative)
        .expect("protected source is present in the manifest");
    assert_eq!(digest(&bytes), expected);

    let mut drifted = bytes;
    let index = drifted
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .expect("protected source is nonempty");
    drifted[index] ^= 1;
    assert_ne!(
        digest(&drifted),
        expected,
        "the negative fixture must trip on one changed source bit"
    );
}

fn parse_manifest() -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for (index, line) in MANIFEST.lines().enumerate() {
        let (path, hash) = line
            .split_once('\t')
            .unwrap_or_else(|| panic!("manifest line {} is malformed", index + 1));
        assert!(
            path.contains('/') && !path.contains('\\'),
            "manifest paths must be repository-relative POSIX paths"
        );
        assert!(
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "manifest line {} has a noncanonical SHA-256",
            index + 1
        );
        assert!(
            values.insert(path.to_owned(), hash.to_owned()).is_none(),
            "manifest contains duplicate path {path}"
        );
    }
    values
}

fn production_source_digests() -> BTreeMap<String, String> {
    let root = workspace_root();
    let mut paths = Vec::new();
    for relative in RECURSIVE_ROOTS {
        collect_files(&root.join(relative), &mut paths);
    }
    paths.extend(INDIVIDUAL_FILES.iter().map(|relative| root.join(relative)));
    paths.sort();
    paths.dedup();

    paths
        .into_iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .expect("protected source lives below workspace")
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            (relative, digest(&bytes))
        })
        .collect()
}

fn collect_files(root: &Path, output: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("cannot enumerate {}: {error}", root.display()));
    for entry in entries {
        let entry = entry.expect("protected source directory entry is readable");
        let path = entry.path();
        let kind = entry
            .file_type()
            .unwrap_or_else(|error| panic!("cannot inspect {}: {error}", path.display()));
        if kind.is_dir() {
            collect_files(&path, output);
        } else if kind.is_file() {
            output.push(path);
        }
    }
}

fn digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    sha256_words(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, word| {
            write!(output, "{word:08x}").expect("writing to String cannot fail");
            output
        })
}

fn sha256_words(bytes: &[u8]) -> [u32; 8] {
    const ROUND: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let bit_length = u64::try_from(bytes.len())
        .expect("protected source length fits u64")
        .checked_mul(8)
        .expect("protected source bit length fits u64");
    let mut padded = bytes.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_length.to_be_bytes());

    for block in padded.chunks_exact(64) {
        let mut schedule = [0_u32; 64];
        for (word, bytes) in schedule[..16].iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_be_bytes(bytes.try_into().expect("SHA-256 word has four bytes"));
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let sum1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ (!e & g);
            let temporary1 = h
                .wrapping_add(sum1)
                .wrapping_add(choose)
                .wrapping_add(ROUND[index])
                .wrapping_add(schedule[index]);
            let sum0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temporary2 = sum0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temporary1);
            d = c;
            c = b;
            b = a;
            a = temporary1.wrapping_add(temporary2);
        }
        for (value, addition) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *value = (*value).wrapping_add(addition);
        }
    }
    state
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("contract test crate lives under <workspace>/crates")
        .to_path_buf()
}
