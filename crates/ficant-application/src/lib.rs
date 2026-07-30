//! Phase 1 application use cases and owned ports.

mod error;
pub mod ports;
pub mod use_cases;

pub use error::{
    ApplicationError, ApplicationErrorCategory, ApplicationErrorDetail, map_domain_error,
    map_runtime_error,
};
pub use ports::{
    AccessScope, AeadCursorCodec, Cursor, CursorKey, CursorPage, IdempotencyKey,
    OperationFingerprint, PageRequest,
};
pub use use_cases::bond_analytics::{
    BOND_ANALYTICS_MEDIA_TYPE, BondAnalyticsReplay, CalculateBondAnalytics, PublishBondAnalytics,
    ReplayBondAnalytics, map_analytics_error,
};
pub use use_cases::carry_roll::{
    CARRY_ROLL_MEDIA_TYPE, CalculateCarryRoll, CarryRollReplay, PublishCarryRoll, ReplayCarryRoll,
};
pub use use_cases::data_snapshot::{DataSnapshotPayloads, PublishDataSnapshot};
pub use use_cases::futures_delivery::{
    CalculateFuturesDeliveryBasket, FUTURES_DELIVERY_MEDIA_TYPE, FuturesDeliveryCandidateBinding,
    FuturesDeliveryInputBindings, FuturesDeliveryReplay, MaterializeFuturesDeliveryInputs,
    PublishFuturesDelivery, ReplayFuturesDelivery, ResolveFuturesContract,
    ResolveFuturesDeliveryRule,
};
pub use use_cases::futures_hedge::{
    CalculateFuturesHedge, FUTURES_HEDGE_MEDIA_TYPE, FuturesHedgeReplay, PublishFuturesHedge,
    ReplayFuturesHedge,
};
pub use use_cases::phase1_business_loop::{
    Phase1BusinessInput, Phase1BusinessLoop, Phase1BusinessResult, StagedArtifact, StagedSnapshot,
    replay_collected_journal,
};
pub use use_cases::verified_reads::VerifiedSnapshotReader;
pub use use_cases::verified_reads::{
    VerifiedArtifactRead, VerifiedReadFacade, VerifiedSignalRead, VerifiedSnapshotRead,
};
