//! Pure synchronous L2 Desk Blotter application orchestration, standardized `PRE_TAX`
//! valuation, shared deterministic `RulePack` draft validation, and per-`(source_id, instrument)`
//! market quote management.

pub mod drafts;
pub mod model;
pub mod orchestrator;
pub mod quotes;
pub mod valuation;

pub use drafts::{
    ConfirmDraftEarmarkCommand, ConfirmDraftEarmarkOutput, DraftValidationTerms,
    SubmitDraftEarmarkCommand, SubmitDraftEarmarkOutput, ValidatedDraftPreview,
    apply_confirm_draft_earmark_state, apply_submit_draft_earmark_state,
    compute_draft_confirmation_token, validate_and_preview_draft, validate_source_text_anchors,
};
pub use model::{
    AccountChange, AccountDeskPolicy, AccountMetadataState, AnalysisUnits, BlotterEventEnvelope,
    CalendarResolutionMode, CoverageStatus, DeskBlotterWorkspaceState, DraftEarmarkRecord,
    DraftEarmarkStatus, DraftTargetState, EnrichedAccountBlotterSnapshot, FullOpeningPositionBase,
    InstrumentCatalogEntry, MarketQuoteTick, NlpEarmarkProposal, PollEventsBatch,
    PreviewCoveredHedgeInput, PreviewCoveredHedgeMeasures, PreviewCoveredHedgeOutput,
    PreviewHedgeScope, PullMarketQuotesResult, PullTradeFeedBatchResult, QuoteCursor,
    QuoteStoreState, RateCoverageEligibility, RiskAndCoverageSummary, RuleLineage,
    SnapshotFallbackRequired, SourceTextAnchor, StoredMarketQuoteTick, UncoveredReason,
    ValuedInstrumentPositionView,
};
pub use orchestrator::{
    BackfillTradeRevisionsOutput, BlotterEvaluationEngines, DeskBlotterOrchestrator,
    ImportFullOpeningBaseCommand, ImportFullOpeningBaseOutput, MissingTradeRevisionRequest,
    OrchestratedCancelCommittedOutput, OrchestratedConfirmHoldOutput,
    OrchestratedCreateCommittedOutput, OrchestratedCreateHoldOutput, OrchestratedExtendHoldOutput,
    OrchestratedIngestMarketQuotesOutput, OrchestratedIngestTradeFeedOutput,
    OrchestratedReleaseHoldOutput, OrchestratedResolvePendingMatchOutput,
    OrchestratedSetFeedControlOutput, OrchestratedSweepLifecycleOutput,
    OrchestratedSwitchQuoteSourceOutput, pending_missing_trade_revisions,
};
pub use quotes::{
    IngestMarketQuotesInput, QuoteIngestEffect, SwitchActiveQuoteSourceCommand,
    SwitchActiveQuoteSourceEffect, apply_ingest_market_quotes_state,
    apply_startup_quote_source_reconciliation, apply_switch_active_quote_source_state,
    is_quote_stale,
};
pub use valuation::{
    InstrumentPricingOutcome, StandardizedBondPricing, compute_trade_exact_dirty_price_input,
    decimal_value_to_fixed, evaluate_account_positions_and_risk, evaluate_instrument_pricing,
    evaluate_single_instrument_position, execute_preview_covered_hedge, position_dv01_from_d100,
    price_original_face_pre_tax_bond, price_standardized_pre_tax_bond,
    resolve_effective_calendar_requirement,
};
