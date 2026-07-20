//! Generated Phase 1 transport contracts from the root `interface/` source.
//!
//! Do not hand-write transport DTOs here. Regenerate the included files with
//! the version-and-revision-pinned plugins in `interface/buf.gen.yaml`.

pub mod ficant {
    pub mod app {
        pub mod v1 {
            include!("generated/ficant.app.v1.rs");
            include!("generated/ficant.app.v1.tonic.rs");
        }
    }

    pub mod core {
        pub mod v1 {
            include!("generated/ficant.core.v1.rs");
        }
    }

    pub mod market {
        pub mod v1 {
            include!("generated/ficant.market.v1.rs");
            include!("generated/ficant.market.v1.tonic.rs");
        }
    }

    pub mod rates {
        pub mod v1 {
            include!("generated/ficant.rates.v1.rs");
            include!("generated/ficant.rates.v1.tonic.rs");
        }
    }

    pub mod research {
        pub mod v1 {
            include!("generated/ficant.research.v1.rs");
            include!("generated/ficant.research.v1.tonic.rs");
        }
    }
}
