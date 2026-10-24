//! AC40 (per-bar P&L breakdown) and AC41 (participation cap, no price no fill) over verified Bar
//! snapshots.

mod bar_support;
mod replay_support;

use std::collections::BTreeMap;

use ficant_application::{BarReplayRequest, replay_bars};
use ficant_domain::market::{BarFrequency, BarSettlement, PriceSourceType};
use ficant_domain::primitives::{ContentHash, EffectivePeriod, Ulid, Version, VersionRef};
use ficant_domain::replay::{
    COST_NONE, COST_RATE_PLUS_FIXED, ExecutionRules, FillPriceBasis, NoFillReason, PointInTimeView,
    PolicyArtifact, PolicyArtifactInput, PolicyError, PolicyEvaluator, RemainderHandling,
    RemainderOutcome, ReplayAmount, ReplayFunding, SimulatedBook, SimulationResult, TargetExposure,
    YIELD_THRESHOLD_FAMILY, YieldThresholdPolicy,
};
use ficant_domain::subject::{FundingTier, SubjectVersion};
use rust_decimal::Decimal;

use bar_support::*;
use replay_support::*;

/// Two trading days of 260011.IB minute bars (T+1, visible two seconds after they end).
/// Entry yield 1.84, exit 1.82, target 10m face. Path:
/// - 07-20 09:31:02 sees 1.845 → buy 10m, executed in the 09:32 bar at VWAP 100.10;
/// - held overnight; 07-21 09:31:02 sees 1.815 → sell, executed in the 09:32 bar at VWAP 100.55
///   (not in the 09:31 bar, which started before the decision).
const TWO_DAYS: &str = r#"
{"id":"d1-0930","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:30:00","pub":"2026-07-20 09:31:02","o":"100.00","h":"100.00","l":"100.00","c":"100.00","vw":"100.00","vol":"20000000","ytm":"1.845"}
{"id":"d1-0931","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:31:00","pub":"2026-07-20 09:32:02","o":"100.05","h":"100.05","l":"100.05","c":"100.05","vw":"100.05","vol":"20000000","ytm":"1.838"}
{"id":"d1-0932","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:32:00","pub":"2026-07-20 09:33:02","o":"100.10","h":"100.20","l":"100.10","c":"100.20","vw":"100.10","vol":"30000000","ytm":"1.836"}
{"id":"d2-0930","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-21 09:30:00","pub":"2026-07-21 09:31:02","o":"100.40","h":"100.40","l":"100.40","c":"100.40","vw":"100.40","vol":"20000000","ytm":"1.815"}
{"id":"d2-0931","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-21 09:31:00","pub":"2026-07-21 09:32:02","o":"100.50","h":"100.50","l":"100.50","c":"100.50","vw":"100.50","vol":"20000000","ytm":"1.814"}
{"id":"d2-0932","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-21 09:32:00","pub":"2026-07-21 09:33:02","o":"100.50","h":"100.60","l":"100.50","c":"100.60","vw":"100.55","vol":"20000000","ytm":"1.810"}
"#;

/// The ADR-0024 single-day path (same rows as `bar_replay`): traded face per bar is 20m, 20m,
/// 30m, 0, 10m, 10m, 0, 10m, 10m.
const ONE_DAY: &str = r#"
{"id":"b0930","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:30:00","pub":"2026-07-20 09:31:02","o":"100.10","h":"100.10","l":"100.10","c":"100.10","vw":"100.10","vol":"20000000","ytm":"1.845"}
{"id":"b0931","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:31:00","pub":"2026-07-20 09:32:02","o":"100.12","h":"100.12","l":"100.12","c":"100.12","vw":"100.12","vol":"20000000","ytm":"1.838"}
{"id":"b0932","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:32:00","pub":"2026-07-20 09:33:02","o":"100.12","h":"100.14","l":"100.12","c":"100.14","vw":"100.13","vol":"30000000","ytm":"1.836"}
{"id":"b0933","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:33:00","pub":"2026-07-20 09:34:02","vol":"0"}
{"id":"b0934","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:34:00","pub":"2026-07-20 09:35:02","o":"100.30","h":"100.30","l":"100.30","c":"100.30","vol":"10000000","ytm":"1.815"}
{"id":"b0935","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:35:00","pub":"2026-07-20 09:36:02","o":"100.29","h":"100.29","l":"100.29","c":"100.29","vol":"10000000","ytm":"1.816"}
{"id":"b0936","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:36:00","pub":"2026-07-20 09:37:02","vol":"0"}
{"id":"b0937","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:37:00","pub":"2026-07-20 09:38:02","o":"100.31","h":"100.31","l":"100.31","c":"100.31","vw":"100.31","vol":"10000000","ytm":"1.814"}
{"id":"b0938","code":"260011.IB","freq":"1min","settle":"T+1","start":"2026-07-20 09:38:00","pub":"2026-07-20 09:39:02","o":"100.32","h":"100.32","l":"100.32","c":"100.32","vol":"10000000","ytm":"1.813"}
"#;

fn rules(cap: &str, remainder: RemainderHandling) -> ExecutionRules {
    ExecutionRules::new(amount(cap), remainder).unwrap()
}

fn policy(execution: ExecutionRules) -> PolicyArtifact {
    PolicyArtifact::new(policy_input(execution)).unwrap()
}

fn policy_input(execution: ExecutionRules) -> PolicyArtifactInput {
    PolicyArtifactInput {
        policy_artifact_id: Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FP0").unwrap(),
        owner: owner(),
        family: YIELD_THRESHOLD_FAMILY.to_owned(),
        family_version: 1,
        parameters: BTreeMap::from([
            ("entry_yield".to_owned(), amount("1.84")),
            ("exit_yield".to_owned(), amount("1.82")),
            ("target_face".to_owned(), amount("10000000")),
        ]),
        universe: vec![instrument()],
        frequency: BarFrequency::M1,
        settlement: BarSettlement::T1,
        valid: EffectivePeriod::new(
            market_time("2026-07-01T00:00:00Z"),
            market_time("2026-08-01T00:00:00Z"),
        )
        .unwrap(),
        execution,
    }
}

/// Accrued interest 1.50 on 07-20 and 0.02 on 07-21, with a 2.00 coupon paid on 07-21 (per 100).
fn coupon_carry() -> TableCarry {
    TableCarry::new(
        &[("2026-07-20", "1.50"), ("2026-07-21", "0.02")],
        &[("2026-07-21", "2.00")],
    )
}

#[derive(Clone)]
struct Inputs {
    policy: PolicyArtifact,
    subject: SubjectVersion,
    funding: ReplayFunding,
    carry: TableCarry,
    cost_model: String,
    cost_parameters: BTreeMap<String, ReplayAmount>,
}

/// 1 bp of traded face plus 50 per execution.
fn rate_plus_fixed() -> BTreeMap<String, ReplayAmount> {
    BTreeMap::from([
        ("face_rate_bp".to_owned(), amount("1")),
        ("fee_per_fill".to_owned(), amount("50")),
    ])
}

fn two_day_inputs() -> Inputs {
    Inputs {
        policy: policy(rules("1", RemainderHandling::Cancel)),
        subject: subject(1, FundingTier::DrAvailable),
        funding: funding("0.0365", 365),
        carry: coupon_carry(),
        cost_model: COST_RATE_PLUS_FIXED.to_owned(),
        cost_parameters: rate_plus_fixed(),
    }
}

fn one_day_inputs(execution: ExecutionRules) -> Inputs {
    Inputs {
        policy: policy(execution),
        subject: subject(1, FundingTier::DrAvailable),
        funding: funding("0.0365", 365),
        carry: TableCarry::new(&[("2026-07-20", "1.00")], &[]),
        cost_model: COST_NONE.to_owned(),
        cost_parameters: BTreeMap::new(),
    }
}

fn request<'a>(data: &'a SnapshotBars, inputs: &'a Inputs) -> BarReplayRequest<'a> {
    BarReplayRequest {
        policy: &inputs.policy,
        evaluator: &YieldThresholdPolicy,
        data,
        subject: &inputs.subject,
        subject_state: VersionRef::new(
            Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FV1").unwrap(),
            Version::new(1).unwrap(),
        ),
        funding: inputs.funding.clone(),
        carry: &inputs.carry,
        cost_model: inputs.cost_model.clone(),
        cost_parameters: inputs.cost_parameters.clone(),
        seed: 7,
        code: ContentHash::digest(b"code"),
        runtime: ContentHash::digest(b"runtime"),
    }
}

fn run(
    data: &SnapshotBars,
    inputs: &Inputs,
) -> Result<SimulationResult, ficant_application::ApplicationError> {
    replay_bars(&request(data, inputs))
}

/// Every row's components add up to its equity change, and the rows add up to final equity.
fn assert_rows_reconcile(result: &SimulationResult) {
    let mut equity = ReplayAmount::ZERO;
    for row in result.pnl() {
        assert_eq!(row.components_total(), row.total, "{row:#?}");
        assert_eq!(equity.plus(row.total), row.equity, "{row:#?}");
        equity = row.equity;
    }
}

#[tokio::test]
async fn each_bar_breaks_down_into_price_coupon_funding_and_cost() {
    // AC40. Hand check (face 10m; prices and accrued interest per 100 face):
    //
    // 07-20 09:32  buy 10m at VWAP 100.10, accrued 1.50, close 100.20
    //   cash   = -10m × (100.10 + 1.50)/100 - cost            = -10,160,000 - 1,050
    //   price  = 10m × (100.20 - 100.10)/100                   = 10,000
    //   cost   = -(10m × 1bp + 50)                             = -1,050
    //   equity = -10,161,050 + 10m × (100.20 + 1.50)/100       = 8,950
    // 07-21 09:30  overnight, close 100.40
    //   funding = -10,161,050 × 3.65% × 1 day / 365            = -1,016.105
    //   coupon  = 10m × (0.02 - 1.50 + 2.00)/100               = 52,000 (200,000 paid in cash)
    //   price   = 10m × (100.40 - 100.20)/100                  = 20,000
    //   total   = 70,983.895; equity 79,933.895
    // 07-21 09:31  price 10m × 0.10/100 = 10,000; equity 89,933.895
    // 07-21 09:32  sell 10m at VWAP 100.55, close 100.60
    //   price  = 10m × (100.60 - 100.50)/100 - 10m × (100.60 - 100.55)/100 = 5,000
    //   cost   = -1,050; total 3,950; flat with cash = equity = 93,883.895
    let data = SnapshotBars(two_day_snapshot(TWO_DAYS, "01ARZ3NDEKTSV4RRFFQ69G5FS3").await);
    let result = run(&data, &two_day_inputs()).unwrap();
    assert_rows_reconcile(&result);

    let rows = result.pnl();
    assert_eq!(rows.len(), 6, "one row per bar");
    for row in &rows[..2] {
        assert_eq!(row.total, ReplayAmount::ZERO);
        assert!(row.instruments.is_empty());
    }

    let buy = &rows[2];
    assert_eq!(buy.trading_date, date("2026-07-20"));
    assert_eq!(buy.price(), amount("10000"));
    assert_eq!(buy.coupon(), ReplayAmount::ZERO);
    assert_eq!(buy.funding, ReplayAmount::ZERO);
    assert_eq!(buy.transaction_cost(), amount("-1050"));
    assert_eq!(buy.total, amount("8950"));
    assert_eq!(buy.cash, amount("-10161050"));
    let held = &buy.instruments[&instrument()];
    assert_eq!(held.position_start, ReplayAmount::ZERO);
    assert_eq!(held.position_end, amount("10000000"));
    assert_eq!(held.mark, amount("100.20"));
    assert_eq!(held.accrued_interest, amount("1.50"));

    let overnight = &rows[3];
    assert_eq!(overnight.trading_date, date("2026-07-21"));
    assert_eq!(overnight.funding, amount("-1016.105"));
    assert_eq!(overnight.coupon(), amount("52000"));
    assert_eq!(overnight.price(), amount("20000"));
    assert_eq!(overnight.transaction_cost(), ReplayAmount::ZERO);
    assert_eq!(overnight.total, amount("70983.895"));
    assert_eq!(overnight.cash, amount("-9962066.105"));
    assert_eq!(overnight.equity, amount("79933.895"));

    assert_eq!(rows[4].price(), amount("10000"));
    assert_eq!(
        rows[4].funding,
        ReplayAmount::ZERO,
        "funding is charged once a day"
    );
    assert_eq!(rows[4].coupon(), ReplayAmount::ZERO);

    let sell = &rows[5];
    assert_eq!(sell.price(), amount("5000"));
    assert_eq!(sell.transaction_cost(), amount("-1050"));
    assert_eq!(sell.total, amount("3950"));
    assert_eq!(sell.equity, amount("93883.895"));
    assert_eq!(
        sell.instruments[&instrument()].position_end,
        ReplayAmount::ZERO
    );

    assert_eq!(result.book().positions().count(), 0);
    assert_eq!(result.book().cash(), amount("93883.895"));
    let sum = rows
        .iter()
        .fold(ReplayAmount::ZERO, |total, row| total.plus(row.total));
    assert_eq!(sum, amount("93883.895"));
}

#[tokio::test]
async fn funding_comes_from_the_subjects_tier_and_carry_data_must_exist() {
    let data = SnapshotBars(two_day_snapshot(TWO_DAYS, "01ARZ3NDEKTSV4RRFFQ69G5FS3").await);

    // Funding resolved for another tier than the Subject's is refused.
    let mut other_tier = two_day_inputs();
    other_tier.subject = subject(1, FundingTier::ROnly);
    assert!(run(&data, &other_tier).is_err());

    // A different rate for the same tier changes only the funding component.
    let mut higher = two_day_inputs();
    higher.funding = funding("0.073", 365);
    let higher = run(&data, &higher).unwrap();
    assert_rows_reconcile(&higher);
    assert_eq!(higher.pnl()[3].funding, amount("-2032.21"));
    assert_eq!(higher.pnl()[3].coupon(), amount("52000"));

    // Holding a bond overnight without its accrued interest for the new date fails closed.
    let mut missing = two_day_inputs();
    missing.carry = TableCarry::new(&[("2026-07-20", "1.50")], &[]);
    assert!(run(&data, &missing).is_err());
}

#[tokio::test]
async fn cost_models_are_built_in_and_strictly_parameterised() {
    let data = SnapshotBars(two_day_snapshot(TWO_DAYS, "01ARZ3NDEKTSV4RRFFQ69G5FS3").await);
    let with = |model: &str, parameters: &[(&str, &str)]| {
        let mut inputs = two_day_inputs();
        inputs.cost_model = model.to_owned();
        inputs.cost_parameters = parameters
            .iter()
            .map(|(name, value)| ((*name).to_owned(), amount(value)))
            .collect();
        inputs
    };
    assert!(run(&data, &with("ficant.cost.unknown", &[])).is_err());
    assert!(run(&data, &with(COST_RATE_PLUS_FIXED, &[("face_rate_bp", "1")])).is_err());
    assert!(
        run(
            &data,
            &with(
                COST_RATE_PLUS_FIXED,
                &[
                    ("face_rate_bp", "1"),
                    ("fee_per_fill", "50"),
                    ("extra", "0")
                ]
            )
        )
        .is_err()
    );
    assert!(
        run(
            &data,
            &with(
                COST_RATE_PLUS_FIXED,
                &[("face_rate_bp", "-1"), ("fee_per_fill", "50")]
            )
        )
        .is_err()
    );

    let free = run(&data, &with(COST_NONE, &[])).unwrap();
    assert_rows_reconcile(&free);
    assert!(
        free.pnl()
            .iter()
            .all(|row| row.transaction_cost() == ReplayAmount::ZERO)
    );
    // Without costs: price 45,000 + coupon 52,000 + funding on 10,160,000 borrowed (-1,016).
    assert_eq!(free.pnl()[3].funding, amount("-1016"));
    assert_eq!(free.book().cash(), amount("95984"));
}

/// Every execution stays within traded face × cap, and every attempt records the snapshot's price
/// source type.
fn assert_within_cap(result: &SimulationResult, data: &SnapshotBars, cap: &str) {
    for fill in result.fills() {
        assert_eq!(fill.price_source, PriceSourceType::RealTrade);
        let Some(record) = fill.execution_bar.as_deref() else {
            assert_eq!(fill.no_fill, Some(NoFillReason::NoLaterBar));
            continue;
        };
        let bar = data
            .0
            .bars()
            .iter()
            .find(|bar| bar.source_record_id() == record)
            .unwrap();
        let volume = Decimal::from_i128_with_scale(
            bar.volume().coefficient().parse().unwrap(),
            bar.volume().scale(),
        );
        let limit = amount(
            &(volume * cap.parse::<Decimal>().unwrap())
                .normalize()
                .to_string(),
        );
        assert_eq!(fill.limit, Some(limit), "{fill:#?}");
        assert!(fill.quantity.abs() <= limit, "{fill:#?}");
        if bar.prices().is_none() {
            assert_eq!(fill.no_fill, Some(NoFillReason::NoPriceInBar));
            assert!(fill.quantity.is_zero());
            assert_eq!(fill.price, None);
        }
    }
}

#[tokio::test]
async fn a_capped_remainder_is_cancelled_when_the_policy_says_so() {
    // AC41, CANCEL with a 25% cap:
    // - buy 10m in b0932 (30m traded → limit 7.5m): 7.5m at VWAP 100.13, 2.5m cancelled;
    // - 09:35:02 sell 7.5m in b0936: no trades → nothing, cancelled (NO_PRICE_IN_BAR);
    // - 09:37:02 sell 7.5m in b0938 (10m → 2.5m): 2.5m at close 100.32, 5m cancelled;
    // - 09:39:02 sell the remaining 5m: no later bar.
    let data = SnapshotBars(snapshot(ONE_DAY, "01ARZ3NDEKTSV4RRFFQ69G5FS4").await);
    let result = run(
        &data,
        &one_day_inputs(rules("0.25", RemainderHandling::Cancel)),
    )
    .unwrap();
    assert_rows_reconcile(&result);
    assert_within_cap(&result, &data, "0.25");

    let fills = result.fills();
    assert_eq!(fills.len(), 4, "{fills:#?}");
    let buy = &fills[0];
    assert_eq!(buy.execution_bar.as_deref(), Some("b0932"));
    assert_eq!(buy.requested, amount("10000000"));
    assert_eq!(buy.quantity, amount("7500000"));
    assert_eq!(buy.price, Some(amount("100.13")));
    assert_eq!(buy.basis, Some(FillPriceBasis::Vwap));
    let remainder = buy.remainder.unwrap();
    assert_eq!(remainder.quantity, amount("2500000"));
    assert_eq!(remainder.reason, NoFillReason::ParticipationCap);
    assert_eq!(remainder.outcome, RemainderOutcome::Cancelled);

    let blocked = &fills[1];
    assert_eq!(blocked.execution_bar.as_deref(), Some("b0936"));
    assert_eq!(blocked.requested, amount("-7500000"));
    assert_eq!(blocked.no_fill, Some(NoFillReason::NoPriceInBar));
    assert_eq!(
        blocked.remainder.unwrap().outcome,
        RemainderOutcome::Cancelled
    );

    let partial = &fills[2];
    assert_eq!(partial.decision_time, utc("2026-07-20T01:37:02Z"));
    assert_eq!(partial.execution_bar.as_deref(), Some("b0938"));
    assert_eq!(partial.quantity, amount("-2500000"));
    assert_eq!(partial.price, Some(amount("100.32")));
    assert_eq!(partial.basis, Some(FillPriceBasis::Close));
    assert_eq!(partial.remainder.unwrap().quantity, amount("-5000000"));

    let last = &fills[3];
    assert_eq!(last.decision_time, utc("2026-07-20T01:39:02Z"));
    assert_eq!(last.requested, amount("-5000000"));
    assert_eq!(last.no_fill, Some(NoFillReason::NoLaterBar));

    assert_eq!(result.book().position(&instrument()), amount("5000000"));
}

#[tokio::test]
async fn a_capped_remainder_is_carried_to_the_next_bars_when_the_policy_says_so() {
    // AC41, CARRY with a 25% cap:
    // - buy 10m: b0932 7.5m, b0933 no trades (carried), b0934 2.5m at close 100.30;
    // - 09:35:02 sell 10m: b0936 no trades, b0937 2.5m at VWAP 100.31, b0938 2.5m at close
    //   100.32, then no later bar for the last 5m;
    // - 09:39:02 the policy asks to sell those 5m again: no later bar.
    let data = SnapshotBars(snapshot(ONE_DAY, "01ARZ3NDEKTSV4RRFFQ69G5FS4").await);
    let result = run(
        &data,
        &one_day_inputs(rules("0.25", RemainderHandling::Carry)),
    )
    .unwrap();
    assert_rows_reconcile(&result);
    assert_within_cap(&result, &data, "0.25");

    let fills = result.fills();
    let summary: Vec<(Option<&str>, String, String)> = fills
        .iter()
        .map(|fill| {
            (
                fill.execution_bar.as_deref(),
                fill.requested.canonical(),
                fill.quantity.canonical(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (Some("b0932"), "10000000".to_owned(), "7500000".to_owned()),
            (Some("b0933"), "2500000".to_owned(), "0".to_owned()),
            (Some("b0934"), "2500000".to_owned(), "2500000".to_owned()),
            (Some("b0936"), "-10000000".to_owned(), "0".to_owned()),
            (Some("b0937"), "-10000000".to_owned(), "-2500000".to_owned()),
            (Some("b0938"), "-7500000".to_owned(), "-2500000".to_owned()),
            (None, "-5000000".to_owned(), "0".to_owned()),
            (None, "-5000000".to_owned(), "0".to_owned()),
        ],
        "{fills:#?}"
    );
    assert_eq!(fills[1].no_fill, Some(NoFillReason::NoPriceInBar));
    assert_eq!(
        fills[1].remainder.unwrap().outcome,
        RemainderOutcome::Carried
    );
    assert_eq!(fills[2].basis, Some(FillPriceBasis::Close));
    assert_eq!(fills[4].basis, Some(FillPriceBasis::Vwap));
    // While the carried trades were still executing, the policy counted on them and did not add
    // new ones: the only extra trade comes after the snapshot ran out of bars.
    assert_eq!(fills[6].decision_time, utc("2026-07-20T01:35:02Z"));
    assert_eq!(fills[7].decision_time, utc("2026-07-20T01:39:02Z"));
    assert_eq!(result.book().position(&instrument()), amount("5000000"));
}

#[tokio::test]
async fn funding_carry_cost_and_execution_rules_are_part_of_the_result_id() {
    let data = SnapshotBars(two_day_snapshot(TWO_DAYS, "01ARZ3NDEKTSV4RRFFQ69G5FS3").await);
    let base = run(&data, &two_day_inputs()).unwrap();
    assert_eq!(base, run(&data, &two_day_inputs()).unwrap());

    let change = |edit: fn(&mut Inputs)| {
        let mut inputs = two_day_inputs();
        edit(&mut inputs);
        run(&data, &inputs).unwrap()
    };
    let variants = [
        (
            "funding rate",
            change(|inputs| inputs.funding = funding("0.0366", 365)),
        ),
        (
            "day basis",
            change(|inputs| inputs.funding = funding("0.0365", 360)),
        ),
        (
            "funding rule pack",
            change(|inputs| {
                inputs.funding = ReplayFunding::new(
                    VersionRef::new(
                        Ulid::new("01ARZ3NDEKTSV4RRFFQ69G5FR0").unwrap(),
                        Version::new(2).unwrap(),
                    ),
                    FundingTier::DrAvailable,
                    amount("0.0365"),
                    365,
                )
                .unwrap();
            }),
        ),
        (
            "carry",
            change(|inputs| {
                inputs.carry = TableCarry::new(
                    &[("2026-07-20", "1.50"), ("2026-07-21", "0.02")],
                    &[("2026-07-21", "2.01")],
                );
            }),
        ),
        (
            "cost model",
            change(|inputs| {
                inputs.cost_model = COST_NONE.to_owned();
                inputs.cost_parameters = BTreeMap::new();
            }),
        ),
        (
            "execution rules",
            change(|inputs| inputs.policy = policy(rules("0.9", RemainderHandling::Cancel))),
        ),
    ];
    let mut ids = vec![base.result_id().clone()];
    for (name, result) in &variants {
        assert_ne!(
            result.result_id(),
            base.result_id(),
            "{name} did not change the ID"
        );
        ids.push(result.result_id().clone());
    }
    let unique: std::collections::BTreeSet<_> = ids.iter().map(ContentHash::as_bytes).collect();
    assert_eq!(unique.len(), ids.len());
}

#[test]
fn participation_cap_must_be_a_share_of_the_bar() {
    assert!(ExecutionRules::new(amount("0"), RemainderHandling::Cancel).is_err());
    assert!(ExecutionRules::new(amount("-0.1"), RemainderHandling::Carry).is_err());
    assert!(ExecutionRules::new(amount("1.01"), RemainderHandling::Cancel).is_err());
    assert!(ExecutionRules::new(amount("1"), RemainderHandling::Carry).is_ok());
}

#[tokio::test]
async fn idle_cash_earns_no_funding() {
    // A test-only policy that is short 10m on the first day and flat on the second, so the book
    // holds positive cash overnight. Funding is charged on borrowed cash only (ADR-0025), so the
    // overnight row carries no funding at all.
    struct ShortFirstDay;
    impl PolicyEvaluator for ShortFirstDay {
        #[allow(clippy::unnecessary_literal_bound)]
        fn family(&self) -> &str {
            "test.policy.short-first-day"
        }
        fn family_version(&self) -> u32 {
            1
        }
        fn decide(
            &self,
            policy: &PolicyArtifact,
            view: &PointInTimeView<'_>,
            _: &SimulatedBook,
        ) -> Result<TargetExposure, PolicyError> {
            let mut target = TargetExposure::default();
            let first_day = view
                .bars()
                .iter()
                .all(|bar| bar.local_trading_date() == date("2026-07-20"));
            let face = if first_day { "-10000000" } else { "0" };
            target.set(policy.universe()[0].clone(), amount(face));
            Ok(target)
        }
    }
    let data = SnapshotBars(two_day_snapshot(TWO_DAYS, "01ARZ3NDEKTSV4RRFFQ69G5FS3").await);
    let mut inputs = two_day_inputs();
    inputs.policy = PolicyArtifact::new(PolicyArtifactInput {
        family: "test.policy.short-first-day".to_owned(),
        parameters: BTreeMap::new(),
        ..policy_input(rules("1", RemainderHandling::Cancel))
    })
    .unwrap();
    let result = replay_bars(&BarReplayRequest {
        evaluator: &ShortFirstDay,
        ..request(&data, &inputs)
    })
    .unwrap();
    assert_rows_reconcile(&result);
    let overnight = &result.pnl()[3];
    assert_eq!(overnight.trading_date, date("2026-07-21"));
    assert!(
        result.pnl()[2].cash > ReplayAmount::ZERO,
        "cash is idle overnight"
    );
    assert_eq!(overnight.funding, ReplayAmount::ZERO);
    // The short pays the coupon instead of earning it.
    assert_eq!(overnight.coupon(), amount("-52000"));
}
