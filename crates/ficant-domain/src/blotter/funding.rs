use std::collections::BTreeSet;

use crate::blotter::error::{BlotterError, BlotterResult};
use crate::blotter::model::{
    AccountState, EarmarkState, ExactDirtyPriceCashInput, FundingGapSummary, FundingPrecision,
    PendingMatchReason, PendingMatchState, QuoteBasis, QuoteTerms, SettlementBucket,
    SourceFeedState, TradeSide,
};
use crate::blotter::wide_math::{signed_fixed, wide_mul_div_u128_round_half_even};
use crate::primitives::{FIXED_DECIMAL_FACTOR, FixedDecimal};

/// Denotes the per-100 quote divisor (`100 * 10^12`) used for clean-price estimate fallback.
const PERCENT_PAR_DIVISOR_SCALED: u128 = 100 * (FIXED_DECIMAL_FACTOR as u128);

/// Resolved signed settlement cash and its funding precision classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedSettlementCash {
    pub signed_cash: Option<FixedDecimal>,
    pub precision: FundingPrecision,
}

/// Applies the trade-side sign convention: `BUY` is negative cash outflow, `SELL` is positive cash
/// inflow.
pub fn apply_cash_sign(side: TradeSide, magnitude_abs: u128) -> BlotterResult<FixedDecimal> {
    let negative = matches!(side, TradeSide::Buy);
    signed_fixed(magnitude_abs, negative)
}

/// Resolves settlement cash from trade terms according to the strict precedence rule:
/// 1. Explicit positive `quote_terms.settlement_amount` (`ExactSettlementAmount`);
/// 2. Trade settlement-date pre-tax dirty price `notional * dirty_price / face_value`
///    (`ExactDirtyPrice`);
/// 3. Clean-price estimate `notional * clean_price_per_100 / 100` (`CleanPriceEstimate`);
/// 4. Yield-only or missing price (`UnpricedYieldOnly`, cash `None`).
pub fn resolve_settlement_cash(
    side: TradeSide,
    notional: FixedDecimal,
    quote_terms: Option<&QuoteTerms>,
    exact_dirty_price_input: Option<ExactDirtyPriceCashInput>,
) -> BlotterResult<ResolvedSettlementCash> {
    if !notional.is_non_negative() {
        return Err(BlotterError::validation(
            "notional for settlement cash resolution must be non-negative",
        ));
    }
    if notional == FixedDecimal::ZERO {
        return Ok(ResolvedSettlementCash {
            signed_cash: Some(FixedDecimal::ZERO),
            precision: FundingPrecision::ExactSettlementAmount,
        });
    }

    if let Some(terms) = quote_terms
        && let Some(explicit_amount) = terms.settlement_amount
    {
        if !explicit_amount.is_positive() {
            return Err(BlotterError::validation(
                "explicit settlement_amount must be positive",
            ));
        }
        let signed = apply_cash_sign(side, explicit_amount.scaled().unsigned_abs())?;
        return Ok(ResolvedSettlementCash {
            signed_cash: Some(signed),
            precision: FundingPrecision::ExactSettlementAmount,
        });
    }

    if let Some(exact) = exact_dirty_price_input {
        exact.validate()?;
        let mag = wide_mul_div_u128_round_half_even(
            notional.scaled().unsigned_abs(),
            exact.dirty_price.scaled().unsigned_abs(),
            exact.face_value.scaled().unsigned_abs(),
        )?;
        let signed = apply_cash_sign(side, mag)?;
        return Ok(ResolvedSettlementCash {
            signed_cash: Some(signed),
            precision: FundingPrecision::ExactDirtyPrice,
        });
    }

    if let Some(terms) = quote_terms
        && let Some(QuoteBasis::CleanPricePer100(clean_price)) = terms.basis
    {
        if !clean_price.is_positive() {
            return Err(BlotterError::validation(
                "clean_price_per_100 must be positive",
            ));
        }
        let mag = wide_mul_div_u128_round_half_even(
            notional.scaled().unsigned_abs(),
            clean_price.scaled().unsigned_abs(),
            PERCENT_PAR_DIVISOR_SCALED,
        )?;
        let signed = apply_cash_sign(side, mag)?;
        return Ok(ResolvedSettlementCash {
            signed_cash: Some(signed),
            precision: FundingPrecision::CleanPriceEstimate,
        });
    }

    Ok(ResolvedSettlementCash {
        signed_cash: None,
        precision: FundingPrecision::UnpricedYieldOnly,
    })
}

/// Projects the full 16-field [`FundingGapSummary`] for an account and its trade feed source.
///
/// Formulas:
/// - `P0 = opening_cash_t0 + recognized_flow0`
/// - `P1 = P0 + opening_net_cash_delta_t1 + recognized_flow1`
/// - `gap[b] = max(0, -P[b])`
///
/// `recognized_flow` sums remaining committed earmark cash, active reconciled official cash, and
/// approved baseline cash deltas. Open pending matches are reported separately in
/// `pending_match_cash_t0/t1` and always force `funding_is_complete_exact = false` even if buys
/// and sells net to zero.
#[allow(clippy::too_many_lines)]
pub fn compute_funding_summary(
    account: &AccountState,
    feed: &SourceFeedState,
) -> BlotterResult<FundingGapSummary> {
    let mut flow_t0 = FixedDecimal::ZERO;
    let mut flow_t1 = FixedDecimal::ZERO;
    let mut clean_est_count = 0_u32;
    let mut unpriced_count = 0_u32;

    for earmark in account.earmarks.values() {
        if earmark.state != EarmarkState::Committed || !earmark.unreconciled_notional.is_positive()
        {
            continue;
        }
        match earmark.funding_precision {
            FundingPrecision::CleanPriceEstimate => {
                clean_est_count = clean_est_count.saturating_add(1);
            }
            FundingPrecision::UnpricedYieldOnly => {
                unpriced_count = unpriced_count.saturating_add(1);
            }
            FundingPrecision::ExactSettlementAmount | FundingPrecision::ExactDirtyPrice => {}
        }
        if let Some(cash) = earmark.unreconciled_settlement_cash {
            add_bucket_cash(&mut flow_t0, &mut flow_t1, earmark.settlement_bucket, cash)?;
        } else if earmark.funding_precision != FundingPrecision::UnpricedYieldOnly {
            unpriced_count = unpriced_count.saturating_add(1);
        }
    }

    let mut clean_est_fills: BTreeSet<(&str, &str, u64)> = BTreeSet::new();
    let mut unpriced_fills: BTreeSet<(&str, &str, u64)> = BTreeSet::new();

    for alloc in account.allocations.values() {
        if alloc.reversed_by_revision.is_some() {
            continue;
        }
        if let Some(cash) = alloc.allocated_fill_cash {
            add_bucket_cash(&mut flow_t0, &mut flow_t1, alloc.settlement_bucket, cash)?;
        }
        if alloc.baseline_delta.is_none() {
            let fill_key = (
                alloc.source_id.as_str(),
                alloc.external_trade_id.as_str(),
                alloc.source_revision,
            );
            match alloc.fill_funding_precision {
                FundingPrecision::CleanPriceEstimate => {
                    clean_est_fills.insert(fill_key);
                }
                FundingPrecision::UnpricedYieldOnly => {
                    unpriced_fills.insert(fill_key);
                }
                FundingPrecision::ExactSettlementAmount | FundingPrecision::ExactDirtyPrice => {
                    if alloc.allocated_fill_cash.is_none() {
                        unpriced_fills.insert(fill_key);
                    }
                }
            }
        }
    }

    clean_est_count =
        clean_est_count.saturating_add(u32::try_from(clean_est_fills.len()).unwrap_or(u32::MAX));
    unpriced_count =
        unpriced_count.saturating_add(u32::try_from(unpriced_fills.len()).unwrap_or(u32::MAX));

    let mut pending_cash_t0 = FixedDecimal::ZERO;
    let mut pending_cash_t1 = FixedDecimal::ZERO;
    let mut pending_match_count = 0_u32;
    let mut pending_unknown_cash_count = 0_u32;

    for pm in account.pending_matches.values() {
        if pm.state != PendingMatchState::Open {
            continue;
        }
        pending_match_count = pending_match_count.saturating_add(1);
        if pm.reason == PendingMatchReason::RevisionHold {
            // Unapplied revision hold cannot determine net cash delta until predecessors arrive.
            pending_unknown_cash_count = pending_unknown_cash_count.saturating_add(1);
            continue;
        }
        if let Some(cash) = pm.unresolved_signed_cash {
            add_bucket_cash(
                &mut pending_cash_t0,
                &mut pending_cash_t1,
                pm.settlement_bucket,
                cash,
            )?;
        } else {
            pending_unknown_cash_count = pending_unknown_cash_count.saturating_add(1);
        }
    }

    let opening_cash_t0 = account.opening_base.opening_cash_t0;
    let opening_net_cash_delta_t1 = account.opening_base.opening_net_cash_delta_t1;

    let projected_cash_t0 = opening_cash_t0
        .checked_add(flow_t0)
        .map_err(BlotterError::from)?;
    let projected_cash_t1 = projected_cash_t0
        .checked_add(opening_net_cash_delta_t1)
        .and_then(|value| value.checked_add(flow_t1))
        .map_err(BlotterError::from)?;

    let funding_gap_t0 = funding_gap(projected_cash_t0)?;
    let funding_gap_t1 = funding_gap(projected_cash_t1)?;

    let feed_incomplete = feed.has_incomplete_batches_or_revision_holds();
    let funding_is_complete_exact = clean_est_count == 0
        && unpriced_count == 0
        && pending_match_count == 0
        && pending_unknown_cash_count == 0
        && !feed_incomplete;

    Ok(FundingGapSummary {
        opening_cash_t0,
        opening_net_cash_delta_t1,
        net_settlement_flow_t0: flow_t0,
        net_settlement_flow_t1: flow_t1,
        projected_cash_t0,
        projected_cash_t1,
        funding_gap_t0,
        funding_gap_t1,
        clean_price_estimated_funding_count: clean_est_count,
        unpriced_funding_earmark_count: unpriced_count,
        funding_is_complete_exact,
        pending_match_cash_t0: pending_cash_t0,
        pending_match_cash_t1: pending_cash_t1,
        pending_match_count,
        pending_unknown_cash_count,
        feed_incomplete,
    })
}

fn add_bucket_cash(
    t0: &mut FixedDecimal,
    t1: &mut FixedDecimal,
    bucket: SettlementBucket,
    delta: FixedDecimal,
) -> BlotterResult<()> {
    match bucket {
        SettlementBucket::TPlus0 => {
            *t0 = t0.checked_add(delta).map_err(BlotterError::from)?;
        }
        SettlementBucket::TPlus1 => {
            *t1 = t1.checked_add(delta).map_err(BlotterError::from)?;
        }
    }
    Ok(())
}

fn funding_gap(projected_cash: FixedDecimal) -> BlotterResult<FixedDecimal> {
    if projected_cash.is_non_negative() {
        Ok(FixedDecimal::ZERO)
    } else {
        FixedDecimal::ZERO
            .checked_sub(projected_cash)
            .map_err(BlotterError::from)
    }
}
