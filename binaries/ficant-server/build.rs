use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

const MAX_WEB_ASSETS_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_WEB_ASSETS_FILE_COUNT: usize = 2048;
const PINNED_RULEPACK_ASSET_PATH: &str = "/rulepacks/cgb-futures-v2.bin";
const PINNED_RULEPACK_PAYLOAD_BYTES: usize = 159;
const PINNED_RULEPACK_PAYLOAD_SHA256: &str =
    "8d8a0b944557cf87ed3d1f598ab3e02fa7f0aec125c1c26c17280acbd24367c5";

#[cfg(windows)]
const WIN_FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

fn main() {
    println!("cargo:rerun-if-env-changed=FICANT_CODE_COMMIT_SHA");
    println!("cargo:rerun-if-env-changed=FICANT_CODE_TREE_SHA");
    println!("cargo:rerun-if-env-changed=FICANT_EMBED_WEB_DIST_DIR");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");

    let commit = source_identity("FICANT_CODE_COMMIT_SHA", &["rev-parse", "HEAD"]);
    let tree = source_identity("FICANT_CODE_TREE_SHA", &["rev-parse", "HEAD^{tree}"]);
    require_git_sha(&commit, "commit");
    require_git_sha(&tree, "tree");
    println!("cargo:rustc-env=FICANT_COMPILED_GIT_COMMIT_SHA={commit}");
    println!("cargo:rustc-env=FICANT_COMPILED_GIT_TREE_SHA={tree}");

    generate_embedded_web_assets();
}

fn source_identity(setting: &str, git_args: &[&str]) -> String {
    env::var(setting).unwrap_or_else(|_| {
        let output = Command::new("git")
            .args(git_args)
            .output()
            .unwrap_or_else(|error| panic!("failed to execute git for {setting}: {error}"));
        assert!(
            output.status.success(),
            "git failed while resolving {setting}"
        );
        String::from_utf8(output.stdout)
            .expect("git identity must be UTF-8")
            .trim()
            .to_owned()
    })
}

fn require_git_sha(value: &str, label: &str) {
    assert!(
        value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "compiled Git {label} must be one 40-character lowercase SHA"
    );
}

fn generate_embedded_web_assets() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set by Cargo"));
    let generated_rs = out_dir.join("embedded_web_assets.rs");
    let staging_dir = out_dir.join("embedded_web_dist");

    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir)
            .unwrap_or_else(|err| panic!("failed to clean embedded web dist staging dir: {err}"));
    }

    let raw_env = env::var("FICANT_EMBED_WEB_DIST_DIR").ok();
    let Some(raw_dir) = raw_env.filter(|v| !v.is_empty()) else {
        fs::write(
            &generated_rs,
            "pub static EMBEDDED_WEB_ASSETS: &[(&str, &[u8])] = &[];\n",
        )
        .unwrap_or_else(|err| panic!("failed to write empty embedded_web_assets.rs: {err}"));
        return;
    };

    assert!(
        !raw_dir.trim().is_empty(),
        "FICANT_EMBED_WEB_DIST_DIR must not be whitespace-only"
    );

    let verified_assets = load_and_verify_web_dist_for_build(Path::new(&raw_dir))
        .unwrap_or_else(|err| panic!("FICANT_EMBED_WEB_DIST_DIR validation failed: {err}"));

    fs::create_dir_all(&staging_dir)
        .unwrap_or_else(|err| panic!("failed to create embedded web dist staging dir: {err}"));

    let mut code = String::from("pub static EMBEDDED_WEB_ASSETS: &[(&str, &[u8])] = &[\n");
    for (idx, (url_path, bytes)) in verified_assets.iter().enumerate() {
        let bin_name = format!("{idx}.bin");
        let staged_file = staging_dir.join(&bin_name);
        fs::write(&staged_file, bytes).unwrap_or_else(|err| {
            panic!("failed to stage verified embedded asset '{url_path}': {err}")
        });
        let escaped_url = url_path.escape_default();
        let _ = writeln!(
            code,
            "    (\"{escaped_url}\", include_bytes!(concat!(env!(\"OUT_DIR\"), \"/embedded_web_dist/{bin_name}\"))),"
        );
    }
    code.push_str("];\n");

    fs::write(&generated_rs, code)
        .unwrap_or_else(|err| panic!("failed to write embedded_web_assets.rs: {err}"));
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObjectIdentity {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(windows)]
    file_id: String,
    #[cfg(windows)]
    creation_time: u64,
    #[cfg(windows)]
    last_write_time: u64,
    #[cfg(windows)]
    file_attributes: u32,
    len: u64,
    readonly: bool,
    is_dir: bool,
}

fn load_and_verify_web_dist_for_build(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let abs_root = normalize_and_verify_ancestors(root)?;
    let mut preflight_files = 0_usize;
    let mut preflight_bytes = 0_u64;
    preflight_check_limits(&abs_root, &mut preflight_files, &mut preflight_bytes)?;
    let root_id = inspect_object_identity(&abs_root, true)?;
    println!("cargo:rerun-if-changed={}", abs_root.display());

    let mut assets = BTreeMap::new();
    let mut total_bytes: usize = 0;
    let mut dir_stack: Vec<(PathBuf, ObjectIdentity)> = vec![(abs_root.clone(), root_id)];

    walk_verified_dir(
        &abs_root,
        &abs_root,
        "",
        &mut dir_stack,
        &mut assets,
        &mut total_bytes,
    )?;

    validate_required_assets(&assets)?;
    Ok(assets)
}

fn preflight_check_limits(
    dir: &Path,
    file_count: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), String> {
    for entry_res in
        fs::read_dir(dir).map_err(|err| format!("failed to read '{}': {err}", dir.display()))?
    {
        let entry = entry_res
            .map_err(|err| format!("failed to read entry in '{}': {err}", dir.display()))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|err| format!("failed to inspect '{}': {err}", path.display()))?;
        reject_symlink_or_reparse(&meta, &path)?;
        if meta.is_dir() {
            preflight_check_limits(&path, file_count, total_bytes)?;
        } else if meta.is_file() {
            *file_count = file_count.saturating_add(1);
            if *file_count > MAX_WEB_ASSETS_FILE_COUNT {
                return Err(format!(
                    "web asset file count exceeds limit of {MAX_WEB_ASSETS_FILE_COUNT}"
                ));
            }
            *total_bytes = total_bytes.saturating_add(meta.len());
            if *total_bytes > u64::try_from(MAX_WEB_ASSETS_TOTAL_BYTES).unwrap_or(u64::MAX) {
                return Err(format!(
                    "web asset total size exceeds limit of {MAX_WEB_ASSETS_TOTAL_BYTES} bytes"
                ));
            }
        } else {
            return Err(format!(
                "unsupported non-regular filesystem object at '{}'",
                path.display()
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn walk_verified_dir(
    abs_root: &Path,
    current_dir: &Path,
    rel_prefix: &str,
    dir_stack: &mut Vec<(PathBuf, ObjectIdentity)>,
    assets: &mut BTreeMap<String, Vec<u8>>,
    total_bytes: &mut usize,
) -> Result<(), String> {
    let mut entries = Vec::new();
    for entry_res in fs::read_dir(current_dir).map_err(|err| {
        format!(
            "failed to read directory '{}': {err}",
            current_dir.display()
        )
    })? {
        let entry = entry_res.map_err(|err| {
            format!(
                "failed to read directory entry in '{}': {err}",
                current_dir.display()
            )
        })?;
        entries.push(entry.path());
    }
    entries.sort();

    for entry_path in entries {
        let name = entry_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("non-UTF-8 entry name in '{}'", current_dir.display()))?;
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
            || name.bytes().any(|b| b < 0x20 || b == 0x7F)
        {
            return Err(format!("invalid asset entry name '{name}'"));
        }

        let meta = fs::symlink_metadata(&entry_path).map_err(|err| {
            format!(
                "failed to inspect metadata for '{}': {err}",
                entry_path.display()
            )
        })?;
        reject_symlink_or_reparse(&meta, &entry_path)?;

        let rel_url = if rel_prefix.is_empty() {
            format!("/{name}")
        } else {
            format!("{rel_prefix}/{name}")
        };

        if meta.is_dir() {
            let sub_id = inspect_object_identity(&entry_path, true)?;
            println!("cargo:rerun-if-changed={}", entry_path.display());
            dir_stack.push((entry_path.clone(), sub_id));
            walk_verified_dir(
                abs_root,
                &entry_path,
                &rel_url,
                dir_stack,
                assets,
                total_bytes,
            )?;
            dir_stack.pop();
        } else if meta.is_file() {
            if assets.len() >= MAX_WEB_ASSETS_FILE_COUNT {
                return Err(format!(
                    "web asset file count exceeds limit of {MAX_WEB_ASSETS_FILE_COUNT}"
                ));
            }
            println!("cargo:rerun-if-changed={}", entry_path.display());
            verify_parent_chain(abs_root, dir_stack)?;
            let pre_id = inspect_object_identity(&entry_path, false)?;
            let remaining = MAX_WEB_ASSETS_TOTAL_BYTES.saturating_sub(*total_bytes);
            if pre_id.len > u64::try_from(remaining).unwrap_or(u64::MAX) {
                return Err(format!(
                    "web asset total size exceeds limit of {MAX_WEB_ASSETS_TOTAL_BYTES} bytes"
                ));
            }

            let file = File::open(&entry_path)
                .map_err(|err| format!("failed to open '{}': {err}", entry_path.display()))?;
            let open_meta = file.metadata().map_err(|err| {
                format!(
                    "failed to inspect open handle metadata for '{}': {err}",
                    entry_path.display()
                )
            })?;
            verify_open_handle_matches_pre(&open_meta, &pre_id, &entry_path)?;

            let mut bytes = Vec::with_capacity(usize::try_from(open_meta.len()).unwrap_or(0));
            let take_limit = u64::try_from(remaining)
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            file.take(take_limit)
                .read_to_end(&mut bytes)
                .map_err(|err| format!("failed to read '{}': {err}", entry_path.display()))?;
            if bytes.len() > remaining {
                return Err(format!(
                    "web asset total size exceeds limit of {MAX_WEB_ASSETS_TOTAL_BYTES} bytes"
                ));
            }
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != open_meta.len() {
                return Err(format!(
                    "asset '{}' changed size during read",
                    entry_path.display()
                ));
            }

            verify_parent_chain(abs_root, dir_stack)?;
            let post_id = inspect_object_identity(&entry_path, false)?;
            if post_id != pre_id {
                return Err(format!(
                    "asset '{}' metadata or identity changed during load",
                    entry_path.display()
                ));
            }

            *total_bytes = total_bytes.saturating_add(bytes.len());
            if assets.insert(rel_url.clone(), bytes).is_some() {
                return Err(format!("duplicate asset path '{rel_url}'"));
            }
        } else {
            return Err(format!(
                "unsupported non-regular filesystem object at '{}'",
                entry_path.display()
            ));
        }
    }
    Ok(())
}

fn validate_required_assets(assets: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let Some(index_bytes) = assets.get("/index.html") else {
        return Err("web asset directory is missing required '/index.html'".to_owned());
    };
    if index_bytes.is_empty() {
        return Err("web asset '/index.html' must be non-empty".to_owned());
    }

    let Some(rulepack_bytes) = assets.get(PINNED_RULEPACK_ASSET_PATH) else {
        return Err(format!(
            "web asset directory is missing required '{PINNED_RULEPACK_ASSET_PATH}'"
        ));
    };
    if rulepack_bytes.len() != PINNED_RULEPACK_PAYLOAD_BYTES {
        return Err(format!(
            "RulePack '{PINNED_RULEPACK_ASSET_PATH}' length {} != expected {PINNED_RULEPACK_PAYLOAD_BYTES}",
            rulepack_bytes.len()
        ));
    }
    let digest = Sha256::digest(rulepack_bytes);
    let hex_digest = encode_hex(&digest);
    if hex_digest != PINNED_RULEPACK_PAYLOAD_SHA256 {
        return Err(format!(
            "RulePack '{PINNED_RULEPACK_ASSET_PATH}' SHA-256 '{hex_digest}' != expected '{PINNED_RULEPACK_PAYLOAD_SHA256}'"
        ));
    }
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0f)] as char);
    }
    out
}

fn normalize_and_verify_ancestors(path: &Path) -> Result<PathBuf, String> {
    let Some(raw) = path.to_str() else {
        return Err("path is not valid UTF-8".to_owned());
    };
    if raw.is_empty() || raw.contains('\0') {
        return Err("path is empty or contains NUL".to_owned());
    }
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|err| format!("failed to read current_dir: {err}"))?
            .join(path)
    };
    check_prefixes_no_reparse(&joined)?;
    let normalized = lexically_normalize_path(&joined)?;
    check_prefixes_no_reparse(&normalized)?;
    Ok(normalized)
}

fn lexically_normalize_path(path: &Path) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    let mut normal_depth = 0_usize;
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if normal_depth == 0 {
                    return Err("path escapes filesystem root via '..'".to_owned());
                }
                out.pop();
                normal_depth -= 1;
            }
            Component::Normal(seg) => {
                out.push(seg);
                normal_depth += 1;
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err("normalized path is empty".to_owned());
    }
    Ok(out)
}

fn check_prefixes_no_reparse(path: &Path) -> Result<(), String> {
    let mut cur = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => cur.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                cur.push("..");
                verify_existing_no_reparse(&cur)?;
            }
            Component::Normal(seg) => {
                cur.push(seg);
                verify_existing_no_reparse(&cur)?;
            }
        }
    }
    Ok(())
}

fn verify_existing_no_reparse(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|err| format!("failed to inspect '{}': {err}", path.display()))?;
    reject_symlink_or_reparse(&meta, path)
}

fn reject_symlink_or_reparse(meta: &fs::Metadata, path: &Path) -> Result<(), String> {
    if meta.file_type().is_symlink() {
        return Err(format!("symlink rejected at '{}'", path.display()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if (meta.file_attributes() & WIN_FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            return Err(format!(
                "Windows reparse point / junction rejected at '{}'",
                path.display()
            ));
        }
    }
    Ok(())
}

fn verify_parent_chain(
    abs_root: &Path,
    dir_stack: &[(PathBuf, ObjectIdentity)],
) -> Result<(), String> {
    check_prefixes_no_reparse(abs_root)?;
    for (dir_path, expected_id) in dir_stack {
        let actual_id = inspect_object_identity(dir_path, true)?;
        if &actual_id != expected_id {
            return Err(format!(
                "parent directory '{}' changed identity during asset load",
                dir_path.display()
            ));
        }
    }
    Ok(())
}

fn inspect_object_identity(path: &Path, expect_dir: bool) -> Result<ObjectIdentity, String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|err| format!("failed to inspect metadata for '{}': {err}", path.display()))?;
    reject_symlink_or_reparse(&meta, path)?;
    if expect_dir && !meta.is_dir() {
        return Err(format!("expected directory at '{}'", path.display()));
    }
    if !expect_dir && !meta.is_file() {
        return Err(format!("expected regular file at '{}'", path.display()));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(ObjectIdentity {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            readonly: meta.permissions().readonly(),
            is_dir: expect_dir,
        })
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let file_id = query_windows_file_id(path)?;
        Ok(ObjectIdentity {
            file_id,
            creation_time: meta.creation_time(),
            last_write_time: meta.last_write_time(),
            file_attributes: meta.file_attributes(),
            len: meta.len(),
            readonly: meta.permissions().readonly(),
            is_dir: expect_dir,
        })
    }
}

fn verify_open_handle_matches_pre(
    open_meta: &fs::Metadata,
    pre_id: &ObjectIdentity,
    path: &Path,
) -> Result<(), String> {
    reject_symlink_or_reparse(open_meta, path)?;
    if !open_meta.is_file() || pre_id.is_dir {
        return Err(format!(
            "open handle at '{}' is not a regular file",
            path.display()
        ));
    }
    if open_meta.len() != pre_id.len || open_meta.permissions().readonly() != pre_id.readonly {
        return Err(format!(
            "open handle metadata mismatch for '{}'",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if open_meta.dev() != pre_id.dev || open_meta.ino() != pre_id.ino {
            return Err(format!(
                "open handle device/inode mismatch for '{}'",
                path.display()
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if open_meta.creation_time() != pre_id.creation_time
            || open_meta.last_write_time() != pre_id.last_write_time
            || open_meta.file_attributes() != pre_id.file_attributes
        {
            return Err(format!(
                "open handle attributes/timestamps mismatch for '{}'",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn query_windows_file_id(path: &Path) -> Result<String, String> {
    let raw = path
        .to_str()
        .ok_or_else(|| format!("non-UTF-8 path '{}'", path.display()))?;
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(raw);
    let output = Command::new("fsutil")
        .args(["file", "queryfileid", stripped])
        .output()
        .map_err(|err| format!("failed to execute fsutil file queryfileid: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "fsutil file queryfileid failed for '{}' with status {}",
            path.display(),
            output.status
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut found = Vec::new();
    for raw_tok in text.split_whitespace() {
        let tok = raw_tok.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if let Some(hex) = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X"))
            && !hex.is_empty()
            && hex.bytes().all(|b| b.is_ascii_hexdigit())
        {
            found.push(format!("0x{}", hex.to_ascii_lowercase()));
        }
    }
    if found.len() != 1 {
        return Err(format!(
            "unexpected fsutil file queryfileid output for '{}'",
            path.display()
        ));
    }
    Ok(found.remove(0))
}
