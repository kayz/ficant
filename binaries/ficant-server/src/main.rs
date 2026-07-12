use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match ficant_server::entry_from_env().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
