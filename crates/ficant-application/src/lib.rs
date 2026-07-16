//! Phase 1 application use cases and owned ports.

mod error;
pub mod ports;
pub mod use_cases;

pub use error::{ApplicationError, ApplicationErrorCategory, map_domain_error, map_runtime_error};
pub use ports::{
    AccessScope, AeadCursorCodec, Cursor, CursorKey, CursorPage, IdempotencyKey,
    OperationFingerprint, PageRequest,
};
pub use use_cases::bond_analytics::{
    BOND_ANALYTICS_MEDIA_TYPE, BondAnalyticsReplay, CalculateBondAnalytics, PublishBondAnalytics,
    ReplayBondAnalytics, map_analytics_error,
};
pub use use_cases::phase1_business_loop::{
    Phase1BusinessInput, Phase1BusinessLoop, Phase1BusinessResult, StagedArtifact, StagedSnapshot,
    replay_collected_journal,
};
pub use use_cases::verified_reads::{
    VerifiedArtifactRead, VerifiedReadFacade, VerifiedSignalRead, VerifiedSnapshotRead,
};
