use std::env;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    if env::args().any(|arg| arg == "--health-check") {
        return match ficant_server::health_check_from_env() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }

    match ficant_server::entry_from_env().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
