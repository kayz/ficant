//! Pure synchronous application ports and pricing/risk/position calculators.
//!
//! Deliberately carries zero database, object-storage, or async I/O coupling so
//! the core pricing and rule-pack engine runs in-memory anywhere.

#![allow(
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::module_name_repetitions
)]

pub mod error;
pub mod ports;
pub mod use_cases;

pub use error::{
    ApplicationError, ApplicationErrorCategory, ApplicationErrorDetail, map_domain_error,
};
pub use use_cases::{
    CalculateBondAnalytics, CalculateCarryRoll, CalculateFuturesDeliveryBasket,
    CalculateFuturesHedge, CapitalUse, InterpolateYieldCurve, PositionView, PositionViews,
    SubjectCandidateMeasures, TaxAdjustedBondAnalytics, calculate_capital_use,
    parse_verified_delivery_rule, parse_verified_delivery_rule_for_per_100_hedge,
    parse_verified_funding_rule, parse_verified_regulatory_rule, parse_verified_tax_rule,
    project_position_views, subject_delivery_irr,
};
