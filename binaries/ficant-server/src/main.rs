use std::env;
use std::process::ExitCode;

use ficant_server::{
    CliCommand, entry_from_env, health_check_from_env, parse_cli_args_os, run_sqlite_rollover_json,
    run_verify_sqlite_archive_json,
};

fn main() -> ExitCode {
    let command = match parse_cli_args_os(env::args_os().skip(1)) {
        Ok(cmd) => cmd,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    match command {
        CliCommand::HealthCheck => match health_check_from_env() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        CliCommand::SqliteRollover(request) => match run_sqlite_rollover_json(&request) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        CliCommand::VerifySqliteArchive { manifest_path } => {
            match run_verify_sqlite_archive_json(&manifest_path) {
                Ok(json) => {
                    println!("{json}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        CliCommand::Serve => {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(error) => {
                    eprintln!("failed to initialize async runtime: {error}");
                    return ExitCode::FAILURE;
                }
            };
            match runtime.block_on(entry_from_env()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}
