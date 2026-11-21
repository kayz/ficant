use crate::blotter::error::{BlotterError, BlotterResult};
use crate::blotter::model::{
    AccountState, BucketBalanceView, EarmarkState, InstrumentPositionState, InstrumentPositionView,
    PendingMatchReason, PendingMatchState, SettlementBucket, TradeSide,
};
use crate::primitives::FixedDecimal;

/// Projects the full T+0 / T+1 bucket balance and economic net notional view for one instrument.
pub fn project_instrument_position(
    account: &AccountState,
    instrument_code: &str,
) -> BlotterResult<InstrumentPositionView> {
    let pos = account.positions.get(instrument_code).ok_or_else(|| {
        BlotterError::not_found(format!(
            "instrument {instrument_code} not found in account {}",
            account.account_id
        ))
    })?;

    project_from_position_state(account, pos)
}

#[allow(clippy::too_many_lines)]
pub(crate) fn project_from_position_state(
    account: &AccountState,
    pos: &InstrumentPositionState,
) -> BlotterResult<InstrumentPositionView> {
    let mut hold_buy_t0 = FixedDecimal::ZERO;
    let mut hold_buy_t1 = FixedDecimal::ZERO;
    let mut hold_sell_t0 = FixedDecimal::ZERO;
    let mut hold_sell_t1 = FixedDecimal::ZERO;
    let mut committed_buy_t0 = FixedDecimal::ZERO;
    let mut committed_buy_t1 = FixedDecimal::ZERO;
    let mut committed_sell_t0 = FixedDecimal::ZERO;
    let mut committed_sell_t1 = FixedDecimal::ZERO;

    for earmark in account.earmarks.values() {
        if earmark.instrument_code != pos.instrument_code {
            continue;
        }
        match earmark.state {
            EarmarkState::Hold => match (earmark.side, earmark.settlement_bucket) {
                (TradeSide::Buy, SettlementBucket::TPlus0) => {
                    hold_buy_t0 = checked_add(hold_buy_t0, earmark.notional)?;
                }
                (TradeSide::Buy, SettlementBucket::TPlus1) => {
                    hold_buy_t1 = checked_add(hold_buy_t1, earmark.notional)?;
                }
                (TradeSide::Sell, SettlementBucket::TPlus0) => {
                    hold_sell_t0 = checked_add(hold_sell_t0, earmark.notional)?;
                }
                (TradeSide::Sell, SettlementBucket::TPlus1) => {
                    hold_sell_t1 = checked_add(hold_sell_t1, earmark.notional)?;
                }
            },
            EarmarkState::Committed => match (earmark.side, earmark.settlement_bucket) {
                (TradeSide::Buy, SettlementBucket::TPlus0) => {
                    committed_buy_t0 =
                        checked_add(committed_buy_t0, earmark.unreconciled_notional)?;
                }
                (TradeSide::Buy, SettlementBucket::TPlus1) => {
                    committed_buy_t1 =
                        checked_add(committed_buy_t1, earmark.unreconciled_notional)?;
                }
                (TradeSide::Sell, SettlementBucket::TPlus0) => {
                    committed_sell_t0 =
                        checked_add(committed_sell_t0, earmark.unreconciled_notional)?;
                }
                (TradeSide::Sell, SettlementBucket::TPlus1) => {
                    committed_sell_t1 =
                        checked_add(committed_sell_t1, earmark.unreconciled_notional)?;
                }
            },
            EarmarkState::Expired
            | EarmarkState::Released
            | EarmarkState::Cancelled
            | EarmarkState::Reconciled => {}
        }
    }

    let mut pending_buy_t0 = FixedDecimal::ZERO;
    let mut pending_buy_t1 = FixedDecimal::ZERO;
    let mut pending_sell_t0 = FixedDecimal::ZERO;
    let mut pending_sell_t1 = FixedDecimal::ZERO;

    for pm in account.pending_matches.values() {
        if pm.instrument_code != pos.instrument_code
            || pm.state != PendingMatchState::Open
            || pm.reason == PendingMatchReason::RevisionHold
        {
            continue;
        }
        match (pm.side, pm.settlement_bucket) {
            (TradeSide::Buy, SettlementBucket::TPlus0) => {
                pending_buy_t0 = checked_add(pending_buy_t0, pm.unresolved_notional)?;
            }
            (TradeSide::Buy, SettlementBucket::TPlus1) => {
                pending_buy_t1 = checked_add(pending_buy_t1, pm.unresolved_notional)?;
            }
            (TradeSide::Sell, SettlementBucket::TPlus0) => {
                pending_sell_t0 = checked_add(pending_sell_t0, pm.unresolved_notional)?;
            }
            (TradeSide::Sell, SettlementBucket::TPlus1) => {
                pending_sell_t1 = checked_add(pending_sell_t1, pm.unresolved_notional)?;
            }
        }
    }

    let s0 = checked_add(
        checked_add(hold_sell_t0, pos.reconciled_sell_t0)?,
        checked_add(committed_sell_t0, pending_sell_t0)?,
    )?;
    let s1 = checked_add(
        checked_add(hold_sell_t1, pos.reconciled_sell_t1)?,
        checked_add(committed_sell_t1, pending_sell_t1)?,
    )?;

    let o_plus_d0 = checked_add(pos.opening_available_notional, pos.opening_net_t0_notional)?;
    let available_t0 = checked_sub(o_plus_d0, s0)?;

    let available_t1 = checked_sub(
        checked_sub(
            checked_add(
                checked_add(o_plus_d0, pos.opening_net_t1_notional)?,
                pos.reconciled_buy_t0,
            )?,
            s0,
        )?,
        s1,
    )?;

    let net_t0 = checked_sub(
        checked_add(committed_buy_t0, pos.reconciled_buy_t0)?,
        checked_add(committed_sell_t0, pos.reconciled_sell_t0)?,
    )?;
    let net_t1 = checked_sub(
        checked_add(committed_buy_t1, pos.reconciled_buy_t1)?,
        checked_add(committed_sell_t1, pos.reconciled_sell_t1)?,
    )?;
    let economic_net_notional =
        checked_add(pos.opening_economic_notional, checked_add(net_t0, net_t1)?)?;

    let sell_ambiguity_locked_t0 = committed_sell_t0.is_positive() && pending_sell_t0.is_positive();
    let sell_ambiguity_locked_t1 = committed_sell_t1.is_positive() && pending_sell_t1.is_positive();

    let bucket_t0 = BucketBalanceView {
        settlement_bucket: SettlementBucket::TPlus0,
        settlement_date: account.opening_base.trading_date,
        available_notional: available_t0,
        hold_buy_notional: hold_buy_t0,
        hold_sell_notional: hold_sell_t0,
        committed_buy_unreconciled_notional: committed_buy_t0,
        committed_sell_unreconciled_notional: committed_sell_t0,
        reconciled_buy_notional: pos.reconciled_buy_t0,
        reconciled_sell_notional: pos.reconciled_sell_t0,
        pending_match_buy_unresolved_notional: pending_buy_t0,
        pending_match_sell_unresolved_notional: pending_sell_t0,
        sell_ambiguity_locked: sell_ambiguity_locked_t0,
        negative_available_alert: available_t0 < FixedDecimal::ZERO,
    };

    let bucket_t1 = BucketBalanceView {
        settlement_bucket: SettlementBucket::TPlus1,
        settlement_date: account.opening_base.next_settlement_date,
        available_notional: available_t1,
        hold_buy_notional: hold_buy_t1,
        hold_sell_notional: hold_sell_t1,
        committed_buy_unreconciled_notional: committed_buy_t1,
        committed_sell_unreconciled_notional: committed_sell_t1,
        reconciled_buy_notional: pos.reconciled_buy_t1,
        reconciled_sell_notional: pos.reconciled_sell_t1,
        pending_match_buy_unresolved_notional: pending_buy_t1,
        pending_match_sell_unresolved_notional: pending_sell_t1,
        sell_ambiguity_locked: sell_ambiguity_locked_t1,
        negative_available_alert: available_t1 < FixedDecimal::ZERO,
    };

    Ok(InstrumentPositionView {
        account_id: account.account_id.clone(),
        instrument_code: pos.instrument_code.clone(),
        position_version: pos.position_version,
        economic_net_notional,
        available_t0_notional: available_t0,
        available_t1_notional: available_t1,
        bucket_t0,
        bucket_t1,
    })
}

/// Validates capacity for creating a new `HOLD`/`COMMITTED` or re-applying after releasing an
/// expired `HOLD`.
pub fn validate_new_earmark_capacity(
    view: &InstrumentPositionView,
    side: TradeSide,
    bucket: SettlementBucket,
    notional: FixedDecimal,
) -> BlotterResult<()> {
    if !notional.is_positive() {
        return Err(BlotterError::validation(
            "earmark notional must be positive",
        ));
    }
    match side {
        TradeSide::Buy => Ok(()),
        TradeSide::Sell => {
            if view.bucket_t0.sell_ambiguity_locked || view.bucket_t1.sell_ambiguity_locked {
                return Err(BlotterError::sell_ambiguity_locked(format!(
                    "sell ambiguity lock active on {}",
                    view.instrument_code
                )));
            }
            match bucket {
                SettlementBucket::TPlus0 => {
                    let min_avail = view.available_t0_notional.min(view.available_t1_notional);
                    if min_avail < notional {
                        return Err(BlotterError::insufficient_available(format!(
                            "insufficient T+0/T+1 available notional for SELL T+0 on {}",
                            view.instrument_code
                        )));
                    }
                    Ok(())
                }
                SettlementBucket::TPlus1 => {
                    if view.available_t1_notional < notional {
                        return Err(BlotterError::insufficient_available(format!(
                            "insufficient T+1 available notional for SELL T+1 on {}",
                            view.instrument_code
                        )));
                    }
                    Ok(())
                }
            }
        }
    }
}

/// Validates an in-term `HOLD -> COMMITTED` conversion on the simulated post-conversion position.
pub fn validate_in_term_confirm_capacity(
    pre_view: &InstrumentPositionView,
    post_view: &InstrumentPositionView,
    side: TradeSide,
    bucket: SettlementBucket,
) -> BlotterResult<()> {
    match side {
        TradeSide::Buy => Ok(()),
        TradeSide::Sell => {
            if pre_view.bucket_t0.sell_ambiguity_locked || pre_view.bucket_t1.sell_ambiguity_locked
            {
                return Err(BlotterError::sell_ambiguity_locked(format!(
                    "sell ambiguity lock active on {}",
                    pre_view.instrument_code
                )));
            }
            match bucket {
                SettlementBucket::TPlus0 => {
                    let min_avail = post_view
                        .available_t0_notional
                        .min(post_view.available_t1_notional);
                    if min_avail < FixedDecimal::ZERO {
                        return Err(BlotterError::insufficient_available(format!(
                            "post-conversion min(A0, A1) < 0 for SELL T+0 on {}",
                            post_view.instrument_code
                        )));
                    }
                    Ok(())
                }
                SettlementBucket::TPlus1 => {
                    if post_view.available_t1_notional < FixedDecimal::ZERO {
                        return Err(BlotterError::insufficient_available(format!(
                            "post-conversion A1 < 0 for SELL T+1 on {}",
                            post_view.instrument_code
                        )));
                    }
                    Ok(())
                }
            }
        }
    }
}

pub(crate) fn add_reconciled_notional(
    pos: &mut InstrumentPositionState,
    side: TradeSide,
    bucket: SettlementBucket,
    delta: FixedDecimal,
) -> BlotterResult<()> {
    match (side, bucket) {
        (TradeSide::Buy, SettlementBucket::TPlus0) => {
            pos.reconciled_buy_t0 = checked_add(pos.reconciled_buy_t0, delta)?;
        }
        (TradeSide::Buy, SettlementBucket::TPlus1) => {
            pos.reconciled_buy_t1 = checked_add(pos.reconciled_buy_t1, delta)?;
        }
        (TradeSide::Sell, SettlementBucket::TPlus0) => {
            pos.reconciled_sell_t0 = checked_add(pos.reconciled_sell_t0, delta)?;
        }
        (TradeSide::Sell, SettlementBucket::TPlus1) => {
            pos.reconciled_sell_t1 = checked_add(pos.reconciled_sell_t1, delta)?;
        }
    }
    Ok(())
}

pub(crate) fn checked_add(a: FixedDecimal, b: FixedDecimal) -> BlotterResult<FixedDecimal> {
    a.checked_add(b).map_err(BlotterError::from)
}

pub(crate) fn checked_sub(a: FixedDecimal, b: FixedDecimal) -> BlotterResult<FixedDecimal> {
    a.checked_sub(b).map_err(BlotterError::from)
}
