//! Zero-dependency strict CLI argument parser and offline maintenance dispatch for `ficant-server`
//! (`BP-4` `AC-005`, `AC-006`).
//!
//! Supported modes (mutually exclusive):
//! - Default (no flags): run single-port server (`entry_from_env`)
//! - `ficant-server --health-check`
//! - `ficant-server --sqlite-rollover --db-path <FILE> --archive-dir <DIR> [--archive-name <STEM>] [--archive-only] [--allow-unresolved]`
//! - `ficant-server --verify-sqlite-archive --manifest <FILE>`

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use ficant_blotter_store::{
    SqliteRolloverRequest, sqlite_rollover, validate_archive_stem, verify_sqlite_archive,
};

/// Parsed top-level command for `ficant-server`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    /// Start the single-port gRPC / gRPC-Web / static Web UI server from environment variables.
    Serve,
    /// Probe the configured listen address `GET /healthz` within the bounded 2-second timeout.
    HealthCheck,
    /// Execute offline `SQLite` rollover / archive-only snapshot directly without starting a server.
    SqliteRollover(SqliteRolloverRequest),
    /// Independently verify an archive database against its `ficant.blotter.archive-manifest.v1`.
    VerifySqliteArchive { manifest_path: PathBuf },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CliMode {
    HealthCheck,
    SqliteRollover,
    VerifySqliteArchive,
}

impl CliMode {
    const fn flag_name(self) -> &'static str {
        match self {
            Self::HealthCheck => "--health-check",
            Self::SqliteRollover => "--sqlite-rollover",
            Self::VerifySqliteArchive => "--verify-sqlite-archive",
        }
    }
}

/// Parses raw OS CLI arguments (excluding `argv[0]`), rejecting non-UTF-8 arguments fail-closed.
pub fn parse_cli_args_os<I>(args: I) -> Result<CliCommand, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut utf8_args = Vec::new();
    for raw in args {
        let s = raw
            .into_string()
            .map_err(|_| "CLI argument is not valid UTF-8".to_owned())?;
        utf8_args.push(s);
    }
    parse_cli_args(utf8_args)
}

#[derive(Default)]
struct RawCliFlags {
    mode: Option<CliMode>,
    db_path: Option<String>,
    archive_dir: Option<String>,
    archive_name: Option<String>,
    archive_only: bool,
    allow_unresolved: bool,
    manifest: Option<String>,
}

/// Strictly parses CLI arguments (excluding `argv[0]`) with zero external parser dependencies.
///
/// Accepts both `--flag <VAL>` and `--flag=<VAL>` for value-taking options, and rejects any
/// unknown flag, positional argument, duplicate flag, empty value, cross-mode option mixing,
/// invalid `--archive-name` stem, or `--archive-only` + `--allow-unresolved` conflict.
pub fn parse_cli_args<I, S>(args: I) -> Result<CliCommand, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let tokens: Vec<String> = args.into_iter().map(|s| s.as_ref().to_owned()).collect();
    let mut iter = tokens.into_iter().peekable();
    let mut flags = RawCliFlags::default();

    while let Some(token) = iter.next() {
        let (flag, inline_val) = if token.starts_with("--") {
            match token.split_once('=') {
                Some((k, v)) => (k, Some(v)),
                None => (token.as_str(), None),
            }
        } else {
            (token.as_str(), None)
        };

        match flag {
            "--health-check" => {
                reject_inline_value(flag, inline_val)?;
                set_mode(&mut flags.mode, CliMode::HealthCheck)?;
            }
            "--sqlite-rollover" => {
                reject_inline_value(flag, inline_val)?;
                set_mode(&mut flags.mode, CliMode::SqliteRollover)?;
            }
            "--verify-sqlite-archive" => {
                reject_inline_value(flag, inline_val)?;
                set_mode(&mut flags.mode, CliMode::VerifySqliteArchive)?;
            }
            "--archive-only" => {
                reject_inline_value(flag, inline_val)?;
                if flags.archive_only {
                    return Err("duplicate flag '--archive-only'".to_owned());
                }
                flags.archive_only = true;
            }
            "--allow-unresolved" => {
                reject_inline_value(flag, inline_val)?;
                if flags.allow_unresolved {
                    return Err("duplicate flag '--allow-unresolved'".to_owned());
                }
                flags.allow_unresolved = true;
            }
            "--db-path" => {
                take_value("--db-path", inline_val, &mut iter, &mut flags.db_path)?;
            }
            "--archive-dir" => {
                take_value(
                    "--archive-dir",
                    inline_val,
                    &mut iter,
                    &mut flags.archive_dir,
                )?;
            }
            "--archive-name" => {
                take_value(
                    "--archive-name",
                    inline_val,
                    &mut iter,
                    &mut flags.archive_name,
                )?;
            }
            "--manifest" => {
                take_value("--manifest", inline_val, &mut iter, &mut flags.manifest)?;
            }
            _ => {
                return Err(format!("unsupported argument: '{token}'"));
            }
        }
    }

    finalize_cli_command(flags)
}

fn finalize_cli_command(flags: RawCliFlags) -> Result<CliCommand, String> {
    let has_rollover_opt = flags.db_path.is_some()
        || flags.archive_dir.is_some()
        || flags.archive_name.is_some()
        || flags.archive_only
        || flags.allow_unresolved;
    let has_verify_opt = flags.manifest.is_some();

    match flags.mode {
        None => {
            if has_rollover_opt {
                return Err("rollover options require '--sqlite-rollover'".to_owned());
            }
            if has_verify_opt {
                return Err("'--manifest' requires '--verify-sqlite-archive'".to_owned());
            }
            Ok(CliCommand::Serve)
        }
        Some(CliMode::HealthCheck) => {
            if has_rollover_opt || has_verify_opt {
                return Err("'--health-check' cannot be combined with other options".to_owned());
            }
            Ok(CliCommand::HealthCheck)
        }
        Some(CliMode::SqliteRollover) => {
            if has_verify_opt {
                return Err("'--manifest' cannot be combined with '--sqlite-rollover'".to_owned());
            }
            if flags.archive_only && flags.allow_unresolved {
                return Err(
                    "'--archive-only' and '--allow-unresolved' cannot be combined".to_owned(),
                );
            }
            let Some(db_path) = flags.db_path else {
                return Err("'--sqlite-rollover' requires '--db-path <FILE>'".to_owned());
            };
            let Some(archive_dir) = flags.archive_dir else {
                return Err("'--sqlite-rollover' requires '--archive-dir <DIR>'".to_owned());
            };
            if let Some(ref stem) = flags.archive_name {
                validate_archive_stem(stem).map_err(|err| err.to_string())?;
            }
            Ok(CliCommand::SqliteRollover(SqliteRolloverRequest {
                db_path: PathBuf::from(db_path),
                archive_dir: PathBuf::from(archive_dir),
                archive_name: flags.archive_name,
                archive_only: flags.archive_only,
                allow_unresolved: flags.allow_unresolved,
            }))
        }
        Some(CliMode::VerifySqliteArchive) => {
            if has_rollover_opt {
                return Err(
                    "rollover options cannot be combined with '--verify-sqlite-archive'".to_owned(),
                );
            }
            let Some(manifest) = flags.manifest else {
                return Err("'--verify-sqlite-archive' requires '--manifest <FILE>'".to_owned());
            };
            Ok(CliCommand::VerifySqliteArchive {
                manifest_path: PathBuf::from(manifest),
            })
        }
    }
}

/// Runs `ficant_blotter_store::sqlite_rollover` directly (without starting a Tokio server or
/// opening `SqliteBlotterStore`) and returns a deterministic JSON report string.
pub fn run_sqlite_rollover_json(request: &SqliteRolloverRequest) -> Result<String, String> {
    let report = sqlite_rollover(request).map_err(|err| err.to_string())?;
    serde_json::to_string_pretty(&report)
        .map_err(|err| format!("failed to serialize rollover report: {err}"))
}

/// Runs `ficant_blotter_store::verify_sqlite_archive` directly and returns a deterministic JSON
/// report string.
pub fn run_verify_sqlite_archive_json(manifest_path: &Path) -> Result<String, String> {
    let report = verify_sqlite_archive(manifest_path).map_err(|err| err.to_string())?;
    serde_json::to_string_pretty(&report)
        .map_err(|err| format!("failed to serialize archive verification report: {err}"))
}

fn set_mode(slot: &mut Option<CliMode>, target: CliMode) -> Result<(), String> {
    match *slot {
        None => {
            *slot = Some(target);
            Ok(())
        }
        Some(existing) if existing == target => {
            Err(format!("duplicate flag '{}'", target.flag_name()))
        }
        Some(existing) => Err(format!(
            "conflicting mode flags: '{}' and '{}' are mutually exclusive",
            existing.flag_name(),
            target.flag_name()
        )),
    }
}

fn reject_inline_value(flag: &str, inline_val: Option<&str>) -> Result<(), String> {
    if inline_val.is_some() {
        return Err(format!("flag '{flag}' does not accept a value"));
    }
    Ok(())
}

fn is_known_flag_token(tok: &str) -> bool {
    let name = tok.split_once('=').map_or(tok, |(k, _)| k);
    matches!(
        name,
        "--health-check"
            | "--sqlite-rollover"
            | "--verify-sqlite-archive"
            | "--db-path"
            | "--archive-dir"
            | "--archive-name"
            | "--archive-only"
            | "--allow-unresolved"
            | "--manifest"
    )
}

fn take_value(
    flag: &str,
    inline_val: Option<&str>,
    iter: &mut std::iter::Peekable<std::vec::IntoIter<String>>,
    slot: &mut Option<String>,
) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("duplicate flag '{flag}'"));
    }
    let val = if let Some(v) = inline_val {
        v.to_owned()
    } else {
        match iter.peek() {
            None => return Err(format!("missing value for '{flag}'")),
            Some(next) if is_known_flag_token(next) => {
                return Err(format!("missing value for '{flag}'"));
            }
            Some(_) => iter.next().unwrap_or_default(),
        }
    };
    if val.is_empty() || val.trim().is_empty() {
        return Err(format!("empty value for '{flag}'"));
    }
    *slot = Some(val);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_four_valid_cli_modes_and_equals_syntax() {
        assert_eq!(parse_cli_args([] as [&str; 0]).unwrap(), CliCommand::Serve);
        assert_eq!(
            parse_cli_args(["--health-check"]).unwrap(),
            CliCommand::HealthCheck
        );

        let r1 = parse_cli_args([
            "--sqlite-rollover",
            "--db-path",
            "data/blotter.sqlite",
            "--archive-dir",
            "archives",
        ])
        .unwrap();
        assert_eq!(
            r1,
            CliCommand::SqliteRollover(SqliteRolloverRequest {
                db_path: PathBuf::from("data/blotter.sqlite"),
                archive_dir: PathBuf::from("archives"),
                archive_name: None,
                archive_only: false,
                allow_unresolved: false,
            })
        );

        let r2 = parse_cli_args([
            "--archive-dir=archives/day 1",
            "--db-path=data/blotter.sqlite",
            "--archive-name=snap_e1",
            "--archive-only",
            "--sqlite-rollover",
        ])
        .unwrap();
        assert_eq!(
            r2,
            CliCommand::SqliteRollover(SqliteRolloverRequest {
                db_path: PathBuf::from("data/blotter.sqlite"),
                archive_dir: PathBuf::from("archives/day 1"),
                archive_name: Some("snap_e1".to_owned()),
                archive_only: true,
                allow_unresolved: false,
            })
        );

        let r3 = parse_cli_args([
            "--sqlite-rollover",
            "--db-path",
            "a.sqlite",
            "--archive-dir",
            "arch",
            "--allow-unresolved",
        ])
        .unwrap();
        assert_eq!(
            r3,
            CliCommand::SqliteRollover(SqliteRolloverRequest {
                db_path: PathBuf::from("a.sqlite"),
                archive_dir: PathBuf::from("arch"),
                archive_name: None,
                archive_only: false,
                allow_unresolved: true,
            })
        );

        let v1 = parse_cli_args([
            "--verify-sqlite-archive",
            "--manifest",
            "arch/s.manifest.json",
        ])
        .unwrap();
        assert_eq!(
            v1,
            CliCommand::VerifySqliteArchive {
                manifest_path: PathBuf::from("arch/s.manifest.json"),
            }
        );

        let v2 =
            parse_cli_args(["--manifest=arch/s.manifest.json", "--verify-sqlite-archive"]).unwrap();
        assert_eq!(v2, v1);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn rejects_invalid_conflicting_duplicate_and_unknown_arguments() {
        let invalid_cases: &[&[&str]] = &[
            // Mutual exclusion across modes
            &[
                "--health-check",
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
            ],
            &[
                "--health-check",
                "--verify-sqlite-archive",
                "--manifest",
                "m",
            ],
            &[
                "--sqlite-rollover",
                "--verify-sqlite-archive",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--manifest",
                "m",
            ],
            // Duplicate mode flags
            &["--health-check", "--health-check"],
            &[
                "--sqlite-rollover",
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
            ],
            &[
                "--verify-sqlite-archive",
                "--verify-sqlite-archive",
                "--manifest",
                "m",
            ],
            // Conflicting rollover flags
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--archive-only",
                "--allow-unresolved",
            ],
            // Duplicate option flags
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--db-path",
                "b",
                "--archive-dir",
                "c",
            ],
            &[
                "--sqlite-rollover",
                "--db-path=a",
                "--db-path",
                "b",
                "--archive-dir",
                "c",
            ],
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--archive-dir=c",
            ],
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--archive-name",
                "s1",
                "--archive-name",
                "s2",
            ],
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--archive-only",
                "--archive-only",
            ],
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--allow-unresolved",
                "--allow-unresolved",
            ],
            &["--verify-sqlite-archive", "--manifest", "a", "--manifest=b"],
            // Missing required flags or missing/empty values
            &["--sqlite-rollover"],
            &["--sqlite-rollover", "--db-path", "a"],
            &["--sqlite-rollover", "--archive-dir", "b"],
            &["--sqlite-rollover", "--db-path"],
            &["--sqlite-rollover", "--db-path", "", "--archive-dir", "b"],
            &["--sqlite-rollover", "--db-path=", "--archive-dir", "b"],
            &[
                "--sqlite-rollover",
                "--db-path",
                "   ",
                "--archive-dir",
                "b",
            ],
            &["--sqlite-rollover", "--db-path", "--archive-dir", "b"],
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--archive-name",
                "",
            ],
            &["--verify-sqlite-archive"],
            &["--verify-sqlite-archive", "--manifest"],
            &["--verify-sqlite-archive", "--manifest="],
            // Boolean flags with inline values
            &["--health-check=true"],
            &[
                "--sqlite-rollover=1",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
            ],
            &["--verify-sqlite-archive=1", "--manifest", "m"],
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--archive-only=true",
            ],
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--allow-unresolved=1",
            ],
            // Cross-mode flag mixing or options without mode
            &["--health-check", "--db-path", "a"],
            &["--health-check", "--manifest", "m"],
            &[
                "--verify-sqlite-archive",
                "--manifest",
                "m",
                "--db-path",
                "a",
            ],
            &[
                "--verify-sqlite-archive",
                "--manifest",
                "m",
                "--archive-only",
            ],
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--manifest",
                "m",
            ],
            &["--db-path", "a", "--archive-dir", "b"],
            &["--manifest", "m"],
            &["--archive-only"],
            // Unknown flags & positional args
            &["--unknown"],
            &["--help"],
            &["-h"],
            &["positional"],
            &["--"],
            &["--health-check", "extra"],
            // Invalid stems
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--archive-name",
                "bad.sqlite",
            ],
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--archive-name",
                "CON",
            ],
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--archive-name",
                "-leading",
            ],
            &[
                "--sqlite-rollover",
                "--db-path",
                "a",
                "--archive-dir",
                "b",
                "--archive-name",
                "../escape",
            ],
        ];

        for case in invalid_cases {
            assert!(
                parse_cli_args(*case).is_err(),
                "expected invalid CLI args {case:?} to be rejected"
            );
        }
    }
}
