use ficant_bootstrap::{BootstrapError, ServiceRole, entry};

fn main() -> Result<(), BootstrapError> {
    entry(ServiceRole::Web)
}
