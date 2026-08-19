//! Phase 1 application use cases and owned ports.

mod error;
pub mod ports;
pub mod use_cases;

pub use error::{
    ApplicationError, ApplicationErrorCategory, ApplicationErrorDetail, map_domain_error,
    map_runtime_error,
};
pub use ports::{
    AccessScope, AeadCursorCodec, AuthorizedPrincipal, Cursor, CursorKey, CursorPage,
    IdempotencyKey, OperationFingerprint, PageRequest,
};
pub use use_cases::bond_analytics::{
    BOND_ANALYTICS_MEDIA_TYPE, BondAnalyticsReplay, CalculateBondAnalytics, PublishBondAnalytics,
    ReplayBondAnalytics, map_analytics_error,
};
pub use use_cases::carry_roll::{
    CARRY_ROLL_MEDIA_TYPE, CalculateCarryRoll, CarryRollReplay, PublishCarryRoll, ReplayCarryRoll,
};
pub use use_cases::data_health::{
    DataHealthQuery, DataHealthThresholdProfilePayload, GetDataHealthReport,
    PublishDataHealthThresholdProfile,
};
pub use use_cases::data_snapshot::{
    DataSnapshotPayloads, PublishDataSnapshot, PublishUniverseSnapshot, SnapshotUseCase,
    UniverseSnapshotIntent, verify_universe_snapshot_manifest,
};
pub use use_cases::data_sources::DataSourceUseCase;
pub use use_cases::factor_topology::FactorTopologyUseCase;
pub use use_cases::formal_outputs::FormalOutputUseCase;
pub use use_cases::futures_delivery::{
    CalculateFuturesDeliveryBasket, FUTURES_DELIVERY_MEDIA_TYPE, FuturesDeliveryCandidateBinding,
    FuturesDeliveryInputBindings, FuturesDeliveryReplay, MaterializeFuturesDeliveryInputs,
    MaterializeRegisteredFuturesDelivery, PublishFuturesDelivery,
    RegisteredFuturesDeliveryMaterialization, ReplayFuturesDelivery, ResolveFuturesContract,
    ResolveFuturesDeliveryRule,
};
pub use use_cases::futures_hedge::{
    CalculateFuturesHedge, FUTURES_HEDGE_MEDIA_TYPE, FuturesHedgeReplay, PublishFuturesHedge,
    ReplayFuturesHedge,
};
pub use use_cases::governed_inputs::{
    AuthorizedDataSource, FOUNDATION_CHANGE_READ_SCOPE, FoundationChangeUseCase,
    GovernedInputUseCase,
};
pub use use_cases::phase1_business_loop::{
    Phase1BusinessInput, Phase1BusinessLoop, Phase1BusinessResult, StagedArtifact, StagedSnapshot,
    replay_collected_journal,
};
pub use use_cases::portfolio_risk::{
    CalculateBondKeyRateDv01, CalculateBondKeyRateDv01Command, R4D_A_ALGORITHM_ID,
    R4D_A_ALGORITHM_VERSION, R4D_A_CONVENTION_PROFILE,
};
pub use use_cases::position_views::{
    CapitalUse, PositionSnapshotPayload, PositionView, PositionViews, PositionViewsUseCase,
    PublishPositionSnapshot,
};
pub use use_cases::rates_materialization::{
    BondRatesCommand, BondRatesMaterialization, CarryRatesCommand, CarryRatesMaterialization,
    CurveRatesCommand, CurveRatesMaterialization, DeliveryRatesCommand,
    DeliveryRatesMaterialization, HedgeRatesCommand, HedgeRatesMaterialization,
    ImmutableArtifactBinding, ImmutableCurveNodeBinding, ImmutableSnapshotBinding,
    MaterializeBondRatesInput, MaterializeCarryRatesInput, MaterializeCurveRatesInput,
    MaterializeDeliveryRatesInput, MaterializeHedgeRatesInput, RatesEvidenceBinding,
    RatesInputEvidence, RatesInputRole, RatesRequestEvidence, RatesUnitRequirement,
    rates_data_source_content_hash,
};
pub use use_cases::verified_reads::VerifiedSnapshotReader;
pub use use_cases::verified_reads::{
    VerifiedArtifactRead, VerifiedReadFacade, VerifiedSignalRead, VerifiedSnapshotRead,
};
