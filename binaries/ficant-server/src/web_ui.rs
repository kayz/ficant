//! Static Web UI asset loading, compile-time embedded table integration, path sanitization,
//! unauthenticated `/healthz`, and single-port [`HttpEdgeLayer`] (`BP-4` `F-10`, `F-15`,
//! `AC-001`..`AC-004`).

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::future::Future;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::{HeaderValue, Method, Request, Response, StatusCode, header};
use http_body_util::Full;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tonic::body::Body;
use tower_layer::Layer;
use tower_service::Service;

use crate::store_adapter::SqliteBlotterRuntime;

include!(concat!(env!("OUT_DIR"), "/embedded_web_assets.rs"));

/// Maximum total bytes loaded across all web UI assets (64 MiB).
pub const MAX_WEB_ASSETS_TOTAL_BYTES: usize = 64 * 1024 * 1024;
/// Maximum number of files loaded in a web UI asset tree.
pub const MAX_WEB_ASSETS_FILE_COUNT: usize = 2048;
/// Pinned delivery `RulePack` static asset path served to the desk-web SPA.
pub const PINNED_RULEPACK_ASSET_PATH: &str = "/rulepacks/cgb-futures-v2.bin";
/// Exact pinned byte length of `/rulepacks/cgb-futures-v2.bin`.
pub const PINNED_RULEPACK_PAYLOAD_BYTES: usize = 159;
/// Exact pinned lowercase SHA-256 hex digest of `/rulepacks/cgb-futures-v2.bin`.
pub const PINNED_RULEPACK_PAYLOAD_SHA256: &str =
    "8d8a0b944557cf87ed3d1f598ab3e02fa7f0aec125c1c26c17280acbd24367c5";

#[cfg(windows)]
const WIN_FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

/// Returns the number of compile-time embedded static assets linked into this binary.
#[must_use]
pub const fn embedded_assets_count() -> usize {
    EMBEDDED_WEB_ASSETS.len()
}

/// Active static Web UI serving mode reported in `/healthz`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebUiMode {
    Disabled,
    Embedded,
    StaticDir,
}

impl WebUiMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Embedded => "embedded",
            Self::StaticDir => "static_dir",
        }
    }
}

/// Explicit Web UI configuration passed to [`crate::spawn_server_with_web_ui`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WebUiConfig {
    /// Web UI disabled (default for backward-compatible [`crate::spawn_server`]).
    #[default]
    Disabled,
    /// Serve compile-time embedded assets (fails closed if binary has zero embedded assets).
    Embedded,
    /// Load and verify assets once into immutable RAM from `PathBuf` at startup.
    StaticDir(PathBuf),
    /// Standard environment resolution priority:
    /// `serve_web_ui == Some(false)` -> `Disabled`;
    /// else `static_web_dir == Some(dir)` -> `StaticDir`;
    /// else non-empty embedded table -> `Embedded`;
    /// else `serve_web_ui == Some(true)` -> error, otherwise `Disabled`.
    Auto {
        serve_web_ui: Option<bool>,
        static_web_dir: Option<PathBuf>,
    },
}

/// Deterministic test hook stages fired during filesystem asset loading (`F-10` TOCTOU tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetLoadHookStage {
    BeforeOpen,
    AfterOpenBeforeVerify,
}

/// Immutable in-memory representation of a single verified static asset.
#[derive(Clone, Debug)]
pub struct WebAsset {
    pub content_type: &'static str,
    pub cache_control: &'static str,
    pub bytes: Bytes,
}

/// Immutable RAM catalog installed once at server startup (`F-10`).
#[derive(Clone, Debug)]
pub struct WebAssetCatalog {
    assets: BTreeMap<String, WebAsset>,
}

impl WebAssetCatalog {
    #[must_use]
    pub fn len(&self) -> usize {
        self.assets.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }

    #[must_use]
    pub fn get(&self, path: &str) -> Option<&WebAsset> {
        self.assets.get(path)
    }

    /// Loads and verifies the compile-time embedded asset slice into an immutable RAM catalog.
    pub fn from_embedded_slice(entries: &[(&str, &[u8])]) -> Result<Self, String> {
        if entries.is_empty() {
            return Err("no compile-time embedded web assets are available".to_owned());
        }
        if entries.len() > MAX_WEB_ASSETS_FILE_COUNT {
            return Err(format!(
                "embedded web asset file count exceeds limit of {MAX_WEB_ASSETS_FILE_COUNT}"
            ));
        }
        let mut raw_map = BTreeMap::new();
        let mut total_bytes = 0_usize;
        for &(path, bytes) in entries {
            total_bytes = total_bytes
                .checked_add(bytes.len())
                .ok_or_else(|| "embedded asset size overflow".to_owned())?;
            if total_bytes > MAX_WEB_ASSETS_TOTAL_BYTES {
                return Err(format!(
                    "embedded web asset total size exceeds limit of {MAX_WEB_ASSETS_TOTAL_BYTES} bytes"
                ));
            }
            if raw_map.insert(path.to_owned(), bytes.to_vec()).is_some() {
                return Err(format!("duplicate embedded web asset '{path}'"));
            }
        }
        validate_required_assets(&raw_map)?;
        Ok(Self::from_verified_map(raw_map))
    }

    fn from_verified_map(raw_map: BTreeMap<String, Vec<u8>>) -> Self {
        let mut assets = BTreeMap::new();
        for (path, bytes) in raw_map {
            let content_type = mime_type_for_path(&path);
            let cache_control = cache_control_for_path(&path);
            assets.insert(
                path,
                WebAsset {
                    content_type,
                    cache_control,
                    bytes: Bytes::from(bytes),
                },
            );
        }
        Self { assets }
    }
}

/// Parses `FICANT_SERVE_WEB_UI` and `FICANT_STATIC_WEB_DIR` from the process environment with
/// strict fail-closed validation (`interfaces.md`).
pub fn web_ui_config_from_env() -> Result<WebUiConfig, String> {
    let serve_raw = env::var("FICANT_SERVE_WEB_UI").ok();
    let static_raw = env::var("FICANT_STATIC_WEB_DIR").ok();
    parse_web_ui_env_values(serve_raw.as_deref(), static_raw.as_deref())
}

/// Pure parser for `FICANT_SERVE_WEB_UI` and `FICANT_STATIC_WEB_DIR` values (unit-testable).
pub fn parse_web_ui_env_values(
    serve_web_ui_raw: Option<&str>,
    static_web_dir_raw: Option<&str>,
) -> Result<WebUiConfig, String> {
    let serve_web_ui = parse_optional_bool_env("FICANT_SERVE_WEB_UI", serve_web_ui_raw)?;
    let static_web_dir = match static_web_dir_raw {
        None | Some("") => None,
        Some(raw) if raw.trim().is_empty() => {
            return Err("FICANT_STATIC_WEB_DIR must not be whitespace-only".to_owned());
        }
        Some(raw) => Some(PathBuf::from(raw)),
    };
    Ok(WebUiConfig::Auto {
        serve_web_ui,
        static_web_dir,
    })
}

/// Parses an optional boolean environment variable (`true`/`false` case-insensitive, `1`/`0`, or
/// empty/unset as `None`). Rejects any other value.
pub fn parse_optional_bool_env(name: &str, raw: Option<&str>) -> Result<Option<bool>, String> {
    match raw {
        None | Some("") => Ok(None),
        Some("1") => Ok(Some(true)),
        Some("0") => Ok(Some(false)),
        Some(v) if v.eq_ignore_ascii_case("true") => Ok(Some(true)),
        Some(v) if v.eq_ignore_ascii_case("false") => Ok(Some(false)),
        Some(other) => Err(format!("invalid boolean value '{other}' for {name}")),
    }
}

/// Resolves [`WebUiConfig`] into an active [`WebUiMode`] and optional immutable RAM
/// [`WebAssetCatalog`], failing closed on any missing or invalid assets.
pub fn resolve_web_ui(
    config: &WebUiConfig,
) -> Result<(WebUiMode, Option<Arc<WebAssetCatalog>>), String> {
    resolve_web_ui_with_embedded(config, EMBEDDED_WEB_ASSETS)
}

/// Pure resolver accepting an explicit embedded slice (enables deterministic unit testing of
/// embedded vs `static_dir` vs `disabled` priority).
pub fn resolve_web_ui_with_embedded(
    config: &WebUiConfig,
    embedded_entries: &[(&str, &[u8])],
) -> Result<(WebUiMode, Option<Arc<WebAssetCatalog>>), String> {
    match config {
        WebUiConfig::Disabled => Ok((WebUiMode::Disabled, None)),
        WebUiConfig::Embedded => {
            let catalog = WebAssetCatalog::from_embedded_slice(embedded_entries)?;
            Ok((WebUiMode::Embedded, Some(Arc::new(catalog))))
        }
        WebUiConfig::StaticDir(dir) => {
            let catalog = load_web_asset_catalog_from_dir(dir)?;
            Ok((WebUiMode::StaticDir, Some(Arc::new(catalog))))
        }
        WebUiConfig::Auto {
            serve_web_ui,
            static_web_dir,
        } => {
            if *serve_web_ui == Some(false) {
                return Ok((WebUiMode::Disabled, None));
            }
            if let Some(dir) = static_web_dir {
                let catalog = load_web_asset_catalog_from_dir(dir)?;
                return Ok((WebUiMode::StaticDir, Some(Arc::new(catalog))));
            }
            if !embedded_entries.is_empty() {
                let catalog = WebAssetCatalog::from_embedded_slice(embedded_entries)?;
                return Ok((WebUiMode::Embedded, Some(Arc::new(catalog))));
            }
            if *serve_web_ui == Some(true) {
                return Err(
                    "FICANT_SERVE_WEB_UI=true requested but neither FICANT_STATIC_WEB_DIR nor embedded web assets are available"
                        .to_owned(),
                );
            }
            Ok((WebUiMode::Disabled, None))
        }
    }
}

/// Loads and verifies a static Web UI directory once into an immutable RAM [`WebAssetCatalog`].
pub fn load_web_asset_catalog_from_dir(root: &Path) -> Result<WebAssetCatalog, String> {
    load_web_asset_catalog_from_dir_with_hook(root, |_, _| Ok(()))
}

/// Loads and verifies a static Web UI directory with a deterministic test hook invoked between
/// pre-open verification and `File::open`, and between file read and post-read verification.
pub fn load_web_asset_catalog_from_dir_with_hook<F>(
    root: &Path,
    mut hook: F,
) -> Result<WebAssetCatalog, String>
where
    F: FnMut(AssetLoadHookStage, &str) -> Result<(), String>,
{
    let abs_root = normalize_and_verify_ancestors(root)?;
    let mut preflight_files = 0_usize;
    let mut preflight_bytes = 0_u64;
    preflight_check_limits(&abs_root, &mut preflight_files, &mut preflight_bytes)?;
    let root_id = inspect_object_identity(&abs_root, true)?;

    let mut raw_assets = BTreeMap::new();
    let mut total_bytes: usize = 0;
    let mut dir_stack: Vec<(PathBuf, ObjectIdentity)> = vec![(abs_root.clone(), root_id)];

    walk_verified_dir(
        &abs_root,
        &abs_root,
        "",
        &mut dir_stack,
        &mut raw_assets,
        &mut total_bytes,
        &mut hook,
    )?;

    validate_required_assets(&raw_assets)?;
    Ok(WebAssetCatalog::from_verified_map(raw_assets))
}

fn preflight_check_limits(
    dir: &Path,
    file_count: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), String> {
    for entry_res in
        fs::read_dir(dir).map_err(|err| format!("failed to read web asset directory: {err}"))?
    {
        let entry = entry_res.map_err(|err| format!("failed to read web asset entry: {err}"))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|err| format!("failed to inspect web asset entry metadata: {err}"))?;
        reject_symlink_or_reparse(&meta)?;
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
            return Err("unsupported non-regular filesystem object in web asset tree".to_owned());
        }
    }
    Ok(())
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

#[allow(clippy::too_many_lines)]
fn walk_verified_dir<F>(
    abs_root: &Path,
    current_dir: &Path,
    rel_prefix: &str,
    dir_stack: &mut Vec<(PathBuf, ObjectIdentity)>,
    assets: &mut BTreeMap<String, Vec<u8>>,
    total_bytes: &mut usize,
    hook: &mut F,
) -> Result<(), String>
where
    F: FnMut(AssetLoadHookStage, &str) -> Result<(), String>,
{
    let mut entries = Vec::new();
    for entry_res in fs::read_dir(current_dir)
        .map_err(|err| format!("failed to read web asset directory: {err}"))?
    {
        let entry = entry_res.map_err(|err| format!("failed to read web asset entry: {err}"))?;
        entries.push(entry.path());
    }
    entries.sort();

    for entry_path in entries {
        let name = entry_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "non-UTF-8 web asset filename".to_owned())?;
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
            || name.bytes().any(|b| b < 0x20 || b == 0x7F)
        {
            return Err("invalid web asset entry name".to_owned());
        }

        let meta = fs::symlink_metadata(&entry_path)
            .map_err(|err| format!("failed to inspect web asset entry metadata: {err}"))?;
        reject_symlink_or_reparse(&meta)?;

        let rel_url = if rel_prefix.is_empty() {
            format!("/{name}")
        } else {
            format!("{rel_prefix}/{name}")
        };

        if meta.is_dir() {
            let sub_id = inspect_object_identity(&entry_path, true)?;
            dir_stack.push((entry_path.clone(), sub_id));
            walk_verified_dir(
                abs_root,
                &entry_path,
                &rel_url,
                dir_stack,
                assets,
                total_bytes,
                hook,
            )?;
            dir_stack.pop();
        } else if meta.is_file() {
            if assets.len() >= MAX_WEB_ASSETS_FILE_COUNT {
                return Err(format!(
                    "web asset file count exceeds limit of {MAX_WEB_ASSETS_FILE_COUNT}"
                ));
            }
            verify_parent_chain(abs_root, dir_stack)?;
            let pre_id = inspect_object_identity(&entry_path, false)?;
            let remaining = MAX_WEB_ASSETS_TOTAL_BYTES.saturating_sub(*total_bytes);
            if pre_id.len > u64::try_from(remaining).unwrap_or(u64::MAX) {
                return Err(format!(
                    "web asset total size exceeds limit of {MAX_WEB_ASSETS_TOTAL_BYTES} bytes"
                ));
            }

            hook(AssetLoadHookStage::BeforeOpen, &rel_url)?;

            let file = File::open(&entry_path)
                .map_err(|err| format!("failed to open web asset '{rel_url}': {err}"))?;
            let open_meta = file.metadata().map_err(|err| {
                format!("failed to inspect open handle metadata for '{rel_url}': {err}")
            })?;
            verify_open_handle_matches_pre(&open_meta, &pre_id, &rel_url)?;

            let mut bytes = Vec::with_capacity(usize::try_from(open_meta.len()).unwrap_or(0));
            let take_limit = u64::try_from(remaining)
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            file.take(take_limit)
                .read_to_end(&mut bytes)
                .map_err(|err| format!("failed to read web asset '{rel_url}': {err}"))?;
            if bytes.len() > remaining {
                return Err(format!(
                    "web asset total size exceeds limit of {MAX_WEB_ASSETS_TOTAL_BYTES} bytes"
                ));
            }
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != open_meta.len() {
                return Err(format!("web asset '{rel_url}' changed size during read"));
            }

            hook(AssetLoadHookStage::AfterOpenBeforeVerify, &rel_url)?;

            verify_parent_chain(abs_root, dir_stack)?;
            let post_id = inspect_object_identity(&entry_path, false)?;
            if post_id != pre_id {
                return Err(format!(
                    "web asset '{rel_url}' metadata or identity changed during load"
                ));
            }

            *total_bytes = total_bytes.saturating_add(bytes.len());
            if assets.insert(rel_url.clone(), bytes).is_some() {
                return Err(format!("duplicate web asset '{rel_url}'"));
            }
        } else {
            return Err("unsupported non-regular filesystem object in web asset tree".to_owned());
        }
    }
    Ok(())
}

fn validate_required_assets(assets: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let Some(index_bytes) = assets.get("/index.html") else {
        return Err("web assets missing required '/index.html'".to_owned());
    };
    if index_bytes.is_empty() {
        return Err("web asset '/index.html' must be non-empty".to_owned());
    }
    let Some(rulepack_bytes) = assets.get(PINNED_RULEPACK_ASSET_PATH) else {
        return Err(format!(
            "web assets missing required '{PINNED_RULEPACK_ASSET_PATH}'"
        ));
    };
    if rulepack_bytes.len() != PINNED_RULEPACK_PAYLOAD_BYTES {
        return Err(format!(
            "RulePack '{PINNED_RULEPACK_ASSET_PATH}' byte length {} != expected {PINNED_RULEPACK_PAYLOAD_BYTES}",
            rulepack_bytes.len()
        ));
    }
    let digest = Sha256::digest(rulepack_bytes);
    let hex_digest = encode_hex(&digest);
    if hex_digest != PINNED_RULEPACK_PAYLOAD_SHA256 {
        return Err(format!(
            "RulePack '{PINNED_RULEPACK_ASSET_PATH}' SHA-256 mismatch"
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
        return Err("web asset directory path is not valid UTF-8".to_owned());
    };
    if raw.trim().is_empty() || raw.contains('\0') {
        return Err("web asset directory path is empty or contains NUL".to_owned());
    }
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|err| format!("failed to read current working directory: {err}"))?
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
                    return Err("web asset path escapes filesystem root".to_owned());
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
        return Err("normalized web asset path is empty".to_owned());
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
        .map_err(|err| format!("failed to inspect web asset ancestor: {err}"))?;
    reject_symlink_or_reparse(&meta)
}

fn reject_symlink_or_reparse(meta: &fs::Metadata) -> Result<(), String> {
    if meta.file_type().is_symlink() {
        return Err("symlink rejected in web asset tree".to_owned());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if (meta.file_attributes() & WIN_FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            return Err("Windows reparse point / junction rejected in web asset tree".to_owned());
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
            return Err("web asset parent directory changed identity during load".to_owned());
        }
    }
    Ok(())
}

fn inspect_object_identity(path: &Path, expect_dir: bool) -> Result<ObjectIdentity, String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|err| format!("failed to inspect web asset metadata: {err}"))?;
    reject_symlink_or_reparse(&meta)?;
    if expect_dir && !meta.is_dir() {
        return Err("expected directory in web asset tree".to_owned());
    }
    if !expect_dir && !meta.is_file() {
        return Err("expected regular file in web asset tree".to_owned());
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
    rel_url: &str,
) -> Result<(), String> {
    reject_symlink_or_reparse(open_meta)?;
    if !open_meta.is_file() || pre_id.is_dir {
        return Err(format!("open handle for '{rel_url}' is not a regular file"));
    }
    if open_meta.len() != pre_id.len || open_meta.permissions().readonly() != pre_id.readonly {
        return Err(format!("open handle metadata mismatch for '{rel_url}'"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if open_meta.dev() != pre_id.dev || open_meta.ino() != pre_id.ino {
            return Err(format!("open handle identity mismatch for '{rel_url}'"));
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
                "open handle attributes/timestamps mismatch for '{rel_url}'"
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn query_windows_file_id(path: &Path) -> Result<String, String> {
    use std::process::Command;

    let raw = path
        .to_str()
        .ok_or_else(|| "non-UTF-8 path during file identity check".to_owned())?;
    let stripped = raw.strip_prefix(r"\\?\").unwrap_or(raw);
    let output = Command::new("fsutil")
        .args(["file", "queryfileid", stripped])
        .output()
        .map_err(|err| format!("failed to run fsutil file queryfileid: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "fsutil file queryfileid exited with status {}",
            output.status
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    ficant_blotter_store::parse_single_hex_file_id(&text)
        .map_err(|err| format!("failed to parse fsutil file id: {err}"))
}

// ============================================================================
// Static Path Sanitization, MIME & Cache-Control
// ============================================================================

/// Sanitizes an incoming HTTP request path for static Web UI lookup (`F-15`, `AC-002`).
///
/// Rejects (`Err(())` -> HTTP `400 Bad Request`):
/// - paths not starting with `/`
/// - raw ASCII control bytes (`< 0x20` or `0x7F`), raw `\`, or double slashes `//`
/// - malformed percent escapes (`%`, `%2`, `%zz`)
/// - percent-encoded `/` (`%2f`/`%2F`), `\` (`%5c`/`%5C`), or control bytes (`%00`..`%1f`, `%7f`)
/// - invalid UTF-8 after percent-decoding
/// - `.` or `..` path segments (raw or percent-decoded)
pub fn sanitize_static_request_path(raw_path: &str) -> Result<String, &'static str> {
    let raw_bytes = raw_path.as_bytes();
    if raw_bytes.first() != Some(&b'/') {
        return Err("path must start with '/'");
    }
    if raw_path.contains("//") || raw_path.contains('\\') {
        return Err("path contains disallowed separator");
    }
    if raw_bytes.iter().any(|&b| b < 0x20 || b == 0x7F) {
        return Err("path contains control byte");
    }

    let mut decoded = Vec::with_capacity(raw_bytes.len());
    let mut i = 0;
    while i < raw_bytes.len() {
        if raw_bytes[i] == b'%' {
            if i + 2 >= raw_bytes.len() {
                return Err("truncated percent-escape");
            }
            let hi = hex_nibble(raw_bytes[i + 1]).ok_or("malformed percent-escape")?;
            let lo = hex_nibble(raw_bytes[i + 2]).ok_or("malformed percent-escape")?;
            let b = (hi << 4) | lo;
            if b == b'/' || b == b'\\' || b < 0x20 || b == 0x7F {
                return Err("disallowed percent-encoded byte");
            }
            decoded.push(b);
            i += 3;
        } else {
            decoded.push(raw_bytes[i]);
            i += 1;
        }
    }

    let decoded_str = String::from_utf8(decoded).map_err(|_| "invalid UTF-8 in path")?;
    if decoded_str == "/" {
        return Ok("/".to_owned());
    }

    let trimmed = decoded_str.strip_suffix('/').unwrap_or(&decoded_str);
    for seg in trimmed.split('/').skip(1) {
        if seg.is_empty() || seg == "." || seg == ".." {
            return Err("disallowed dot or empty path segment");
        }
    }

    Ok(decoded_str)
}

const fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn mime_type_for_path(path: &str) -> &'static str {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn cache_control_for_path(path: &str) -> &'static str {
    if path == "/index.html" {
        "no-cache"
    } else if path.starts_with("/assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

fn is_missing_resource_path(clean_path: &str) -> bool {
    if clean_path == "/assets"
        || clean_path.starts_with("/assets/")
        || clean_path == "/rulepacks"
        || clean_path.starts_with("/rulepacks/")
    {
        return true;
    }
    let last_seg = clean_path.rsplit('/').next().unwrap_or("");
    last_seg.contains('.')
}

// ============================================================================
// Single-Port HttpEdgeLayer (/healthz + gRPC/gRPC-Web pass-through + Static UI)
// ============================================================================

/// Shared read-only edge state bound to a running `ficant-server` instance.
#[derive(Clone)]
pub struct HttpEdgeState {
    pub storage_mode_str: &'static str,
    pub runtime: Option<Arc<SqliteBlotterRuntime>>,
    pub web_ui_mode: WebUiMode,
    pub catalog: Option<Arc<WebAssetCatalog>>,
    pub embedded_assets_count: usize,
}

#[derive(Serialize)]
struct HealthzPayload {
    status: &'static str,
    service: &'static str,
    storage_mode: &'static str,
    blotter_healthy: bool,
    ledger_epoch: Option<u64>,
    ledger_instance_id: Option<String>,
    web_ui_mode: &'static str,
    embedded_assets_count: usize,
}

impl HttpEdgeState {
    fn build_healthz_response(&self, is_head: bool) -> Response<Body> {
        let (blotter_healthy, ledger_epoch, ledger_instance_id) = match &self.runtime {
            None => (true, None, None),
            Some(rt) => {
                let halted = rt.store().is_halted();
                match rt.store().ledger_identity() {
                    Ok(id) => (!halted, Some(id.ledger_epoch), Some(id.ledger_instance_id)),
                    Err(_) => (false, None, None),
                }
            }
        };

        let status_code = if blotter_healthy {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };
        let payload = HealthzPayload {
            status: if blotter_healthy { "ok" } else { "degraded" },
            service: "ficant-server",
            storage_mode: self.storage_mode_str,
            blotter_healthy,
            ledger_epoch,
            ledger_instance_id,
            web_ui_mode: self.web_ui_mode.as_str(),
            embedded_assets_count: self.embedded_assets_count,
        };
        let body_bytes =
            serde_json::to_vec(&payload).unwrap_or_else(|_| b"{\"status\":\"degraded\"}".to_vec());
        let len_header = HeaderValue::from_str(&body_bytes.len().to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0"));

        let body = if is_head {
            Body::empty()
        } else {
            Body::new(Full::new(Bytes::from(body_bytes)))
        };

        let mut resp = Response::new(body);
        *resp.status_mut() = status_code;
        let headers = resp.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        headers.insert(header::CONTENT_LENGTH, len_header);
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        resp
    }

    fn build_static_response(&self, method: &Method, raw_path: &str) -> Response<Body> {
        let is_head = match *method {
            Method::GET => false,
            Method::HEAD => true,
            _ => return method_not_allowed_response(),
        };

        let Ok(clean_path) = sanitize_static_request_path(raw_path) else {
            return plain_error_response(StatusCode::BAD_REQUEST, "bad request", is_head);
        };

        if self.web_ui_mode == WebUiMode::Disabled {
            return plain_error_response(StatusCode::NOT_FOUND, "not found", is_head);
        }

        let Some(catalog) = self.catalog.as_ref() else {
            return plain_error_response(StatusCode::NOT_FOUND, "not found", is_head);
        };

        let asset = if clean_path == "/" {
            catalog.get("/index.html")
        } else if let Some(exact) = catalog.get(&clean_path) {
            Some(exact)
        } else if is_missing_resource_path(&clean_path) {
            None
        } else {
            catalog.get("/index.html")
        };

        let Some(asset) = asset else {
            return plain_error_response(StatusCode::NOT_FOUND, "not found", is_head);
        };

        let len_header = HeaderValue::from_str(&asset.bytes.len().to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0"));
        let body = if is_head {
            Body::empty()
        } else {
            Body::new(Full::new(asset.bytes.clone()))
        };

        let mut resp = Response::new(body);
        *resp.status_mut() = StatusCode::OK;
        let headers = resp.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(asset.content_type),
        );
        headers.insert(header::CONTENT_LENGTH, len_header);
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(asset.cache_control),
        );
        headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        resp
    }
}

fn method_not_allowed_response() -> Response<Body> {
    let mut resp =
        plain_error_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed", false);
    resp.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("GET, HEAD"));
    resp
}

fn plain_error_response(status: StatusCode, msg: &'static str, is_head: bool) -> Response<Body> {
    let len_header = HeaderValue::from_str(&msg.len().to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("0"));
    let body = if is_head {
        Body::empty()
    } else {
        Body::new(Full::new(Bytes::from_static(msg.as_bytes())))
    };
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(header::CONTENT_LENGTH, len_header);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    resp
}

fn is_grpc_passthrough<B>(req: &Request<B>) -> bool {
    if req.uri().path().starts_with("/ficant.") {
        return true;
    }
    if let Some(ct) = req.headers().get(header::CONTENT_TYPE)
        && let Ok(ct_str) = ct.to_str()
    {
        let media = ct_str.split(';').next().unwrap_or("").trim();
        if media.eq_ignore_ascii_case("application/grpc")
            || media
                .get(..17)
                .is_some_and(|p| p.eq_ignore_ascii_case("application/grpc+"))
            || media
                .get(..17)
                .is_some_and(|p| p.eq_ignore_ascii_case("application/grpc-"))
        {
            return true;
        }
    }
    false
}

/// Outer Tower layer mounted outside [`tonic_web::GrpcWebLayer`] on the single server port.
#[derive(Clone)]
pub struct HttpEdgeLayer {
    state: Arc<HttpEdgeState>,
}

impl HttpEdgeLayer {
    #[must_use]
    pub fn new(state: HttpEdgeState) -> Self {
        Self {
            state: Arc::new(state),
        }
    }
}

impl<S> Layer<S> for HttpEdgeLayer {
    type Service = HttpEdgeService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        HttpEdgeService {
            inner,
            state: self.state.clone(),
        }
    }
}

/// Tower service implementing `/healthz`, unbuffered gRPC/gRPC-Web pass-through, and RAM-backed
/// static Web UI serving with zero `Access-Control-Allow-*` headers (`F-15`).
#[derive(Clone)]
pub struct HttpEdgeService<S> {
    inner: S,
    state: Arc<HttpEdgeState>,
}

/// Response future for [`HttpEdgeService`], zero-`unsafe` and zero-buffering on inner pass-through.
pub enum HttpEdgeFuture<F, E> {
    Immediate(Option<Result<Response<Body>, E>>),
    Inner(Pin<Box<F>>),
}

impl<F, E> Unpin for HttpEdgeFuture<F, E> where E: Unpin {}

impl<F, E> Future for HttpEdgeFuture<F, E>
where
    F: Future<Output = Result<Response<Body>, E>>,
    E: Unpin,
{
    type Output = Result<Response<Body>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            Self::Immediate(resp) => Poll::Ready(
                resp.take()
                    .expect("HttpEdgeFuture::Immediate polled after completion"),
            ),
            Self::Inner(fut) => fut.as_mut().poll(cx),
        }
    }
}

impl<S, ReqBody> Service<Request<ReqBody>> for HttpEdgeService<S>
where
    S: Service<Request<ReqBody>, Response = Response<Body>>,
    S::Error: Unpin,
    ReqBody: http_body::Body<Data = Bytes> + Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = HttpEdgeFuture<S::Future, S::Error>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<ReqBody>) -> Self::Future {
        let path = req.uri().path();
        if path == "/healthz" {
            let resp = match *req.method() {
                Method::GET => self.state.build_healthz_response(false),
                Method::HEAD => self.state.build_healthz_response(true),
                _ => method_not_allowed_response(),
            };
            return HttpEdgeFuture::Immediate(Some(Ok(resp)));
        }

        if is_grpc_passthrough(&req) {
            return HttpEdgeFuture::Inner(Box::pin(self.inner.call(req)));
        }

        let resp = self.state.build_static_response(req.method(), path);
        HttpEdgeFuture::Immediate(Some(Ok(resp)))
    }
}
