use std::error::Error;
use std::sync::Arc;

use ficant_bootstrap::{ServiceRole, entry};
use ficant_worker::{ProductionWorkerBackend, WorkerConfig, run_worker};
use tokio::sync::watch;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().nth(1).as_deref() == Some("--print-native-source-digest") {
        println!(
            "{}",
            ficant_native_nodes::native_node_source_digest_attestation()
        );
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--health-check") {
        entry(ServiceRole::Worker)?;
        return Ok(());
    }

    let config = WorkerConfig::from_env()?;
    let backend = Arc::new(ProductionWorkerBackend::connect(&config).await?);
    let (drain_tx, drain_rx) = watch::channel(false);
    install_drain_signal(drain_tx);

    std::thread::spawn(|| {
        if let Err(error) = entry(ServiceRole::Worker) {
            eprintln!("ficant-worker health listener stopped: {error}");
        }
    });

    run_worker(backend.as_ref(), &config, drain_rx).await?;
    Ok(())
}

fn install_drain_signal(drain_tx: watch::Sender<bool>) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};

            let terminate = signal(SignalKind::terminate());
            match terminate {
                Ok(mut terminate) => {
                    tokio::select! {
                        result = tokio::signal::ctrl_c() => {
                            if result.is_err() {
                                return;
                            }
                        }
                        _ = terminate.recv() => {}
                    }
                }
                Err(_) => {
                    if tokio::signal::ctrl_c().await.is_err() {
                        return;
                    }
                }
            }
        }
        #[cfg(not(unix))]
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        let _ = drain_tx.send(true);
    });
}
