mod analytics;
pub mod blotter;
mod curves;
mod funding_rule_parser;
mod futures_delivery;
mod futures_hedge;
mod regulatory_rule_parser;
mod rule_pack_parser;
mod tax_rule_parser;

use crate::ApplicationError;

pub type ApplicationResult<T> = Result<T, ApplicationError>;

pub use analytics::BondAnalyticsEngine;
pub use blotter::{
    BlotterClock, BlotterCommitPlan, BlotterEventJournalPort, BlotterPersistencePort,
    BlotterSnapshotReader, CredentialVerifier, DeskBlotterPolicyProvider, IdempotencyScopeKey,
    MAX_SPI_PAGE_SIZE, MIN_SPI_PAGE_SIZE, ManualBlotterClock, MarketQuoteStreamPort,
    NlpIntentParserPort, PollEventsOutcome, PositionBaseProviderPort, PrincipalRole,
    StoredIdempotencyReceipt, TradeFeedReconcilerPort, VerifiedPrincipal, validate_spi_page_size,
};
pub use curves::{CarryRollEngine, YieldCurveEngine};
pub use funding_rule_parser::{FundingRate, FundingRulePackParser};
pub use futures_delivery::FuturesDeliveryEngine;
pub use futures_hedge::FuturesHedgeEngine;
pub use regulatory_rule_parser::RegulatoryRulePackParser;
pub use rule_pack_parser::FuturesDeliveryRuleParser;
pub use tax_rule_parser::{
    CouponTaxClaimScope, CouponTaxRate, CouponTaxTreatment, GrossCouponTaxBasis, TaxRoundingMode,
    TaxRulePackParser,
};
