use chrono::{NaiveDate, TimeDelta};

use crate::analytics::{
    AnalyticsObjectRef, BondTerms, CalendarBinding, CalendarRequirement, FixedDecimal,
    MARKET_TIMEZONE,
};
use crate::primitives::{ContentHash, MarketTime, OwnerRef};
use crate::{DomainErrorCode, DomainResult};

pub const CURVE_RESULT_SCHEMA_ID: &str = "ficant.yield-curve-point.result.v1";
pub const CURVE_ALGORITHM_ID: &str = "ficant.cgb.ytm-curve.linear";
pub const CURVE_ALGORITHM_VERSION: u32 = 1;
pub const CURVE_CONVENTION_PROFILE: &str = "cfets-ytm-linear-v1";
pub const CARRY_ROLL_RESULT_SCHEMA_ID: &str = "ficant.carry-roll.result.v1";
pub const CARRY_ROLL_ARTIFACT_SCHEMA_ID: &str = "ficant.carry-roll.arrow.v1";
pub const CARRY_ROLL_ARTIFACT_CODEC_ID: &str = "ficant-carry-roll-arrow/1";
pub const CARRY_ROLL_ALGORITHM_ID: &str = "ficant.cgb.carry-roll.unfunded";
pub const CARRY_ROLL_ALGORITHM_VERSION: u32 = 1;
pub const CARRY_ROLL_CONVENTION_PROFILE: &str = "cfets-ytm-carry-roll-v1";

const NEGATIVE_ONE_SCALED: i128 = -1_000_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum YieldCurveInterpolation {
    LinearYield = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct YieldCurveNode {
    maturity_date: NaiveDate,
    yield_to_maturity: FixedDecimal,
}

impl YieldCurveNode {
    pub fn new(maturity_date: NaiveDate, yield_to_maturity: FixedDecimal) -> DomainResult<Self> {
        if yield_to_maturity.scaled() <= NEGATIVE_ONE_SCALED {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            maturity_date,
            yield_to_maturity,
        })
    }

    #[must_use]
    pub const fn maturity_date(self) -> NaiveDate {
        self.maturity_date
    }

    #[must_use]
    pub const fn yield_to_maturity(self) -> FixedDecimal {
        self.yield_to_maturity
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YieldCurveBinding {
    curve_snapshot: AnalyticsObjectRef,
    valuation_date: NaiveDate,
    interpolation: YieldCurveInterpolation,
    nodes: Vec<YieldCurveNode>,
}

impl YieldCurveBinding {
    pub fn new(
        curve_snapshot: AnalyticsObjectRef,
        valuation_date: NaiveDate,
        interpolation: YieldCurveInterpolation,
        nodes: Vec<YieldCurveNode>,
    ) -> DomainResult<Self> {
        if nodes.len() < 2
            || nodes
                .iter()
                .any(|node| node.maturity_date() <= valuation_date)
            || nodes
                .windows(2)
                .any(|pair| pair[0].maturity_date() >= pair[1].maturity_date())
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            curve_snapshot,
            valuation_date,
            interpolation,
            nodes,
        })
    }

    #[must_use]
    pub fn curve_snapshot(&self) -> &AnalyticsObjectRef {
        &self.curve_snapshot
    }

    #[must_use]
    pub const fn valuation_date(&self) -> NaiveDate {
        self.valuation_date
    }

    #[must_use]
    pub const fn interpolation(&self) -> YieldCurveInterpolation {
        self.interpolation
    }

    #[must_use]
    pub fn nodes(&self) -> &[YieldCurveNode] {
        &self.nodes
    }

    #[must_use]
    pub fn covers(&self, query_date: NaiveDate) -> bool {
        self.nodes.first().is_some_and(|first| {
            self.nodes.last().is_some_and(|last| {
                query_date >= first.maturity_date() && query_date <= last.maturity_date()
            })
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YieldCurveQuery {
    curve: YieldCurveBinding,
    query_date: NaiveDate,
}

impl YieldCurveQuery {
    pub fn new(curve: YieldCurveBinding, query_date: NaiveDate) -> DomainResult<Self> {
        if query_date <= curve.valuation_date() || !curve.covers(query_date) {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(Self { curve, query_date })
    }

    #[must_use]
    pub fn curve(&self) -> &YieldCurveBinding {
        &self.curve
    }

    #[must_use]
    pub const fn query_date(&self) -> NaiveDate {
        self.query_date
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YieldCurvePoint {
    query: YieldCurveQuery,
    yield_to_maturity: FixedDecimal,
}

impl YieldCurvePoint {
    pub fn new(query: YieldCurveQuery, yield_to_maturity: FixedDecimal) -> DomainResult<Self> {
        if yield_to_maturity.scaled() <= NEGATIVE_ONE_SCALED {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            query,
            yield_to_maturity,
        })
    }

    pub fn validate_against(&self, query: &YieldCurveQuery) -> DomainResult<()> {
        if &self.query != query {
            return Err(DomainErrorCode::BrokenLineage);
        }
        Ok(())
    }

    #[must_use]
    pub fn query(&self) -> &YieldCurveQuery {
        &self.query
    }

    #[must_use]
    pub const fn yield_to_maturity(&self) -> FixedDecimal {
        self.yield_to_maturity
    }

    #[must_use]
    pub fn schema_id(&self) -> &'static str {
        CURVE_RESULT_SCHEMA_ID
    }

    #[must_use]
    pub fn algorithm_id(&self) -> &'static str {
        CURVE_ALGORITHM_ID
    }

    #[must_use]
    pub const fn algorithm_version(&self) -> u32 {
        CURVE_ALGORITHM_VERSION
    }

    #[must_use]
    pub fn convention_profile(&self) -> &'static str {
        CURVE_CONVENTION_PROFILE
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarryRollInput {
    owner: OwnerRef,
    bond: AnalyticsObjectRef,
    rule_pack: AnalyticsObjectRef,
    snapshot: AnalyticsObjectRef,
    valuation_at: MarketTime,
    initial_settlement: NaiveDate,
    horizon_settlement: NaiveDate,
    calendar_requirement: CalendarRequirement,
    calendar: CalendarBinding,
    terms: BondTerms,
    curve: YieldCurveBinding,
}

impl CarryRollInput {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner: OwnerRef,
        bond: AnalyticsObjectRef,
        rule_pack: AnalyticsObjectRef,
        snapshot: AnalyticsObjectRef,
        valuation_at: MarketTime,
        initial_settlement: NaiveDate,
        horizon_settlement: NaiveDate,
        calendar_requirement: CalendarRequirement,
        calendar: CalendarBinding,
        terms: BondTerms,
        curve: YieldCurveBinding,
    ) -> DomainResult<Self> {
        if valuation_at.market_timezone() != MARKET_TIMEZONE
            || curve.valuation_date() != valuation_at.local_trading_date()
            || initial_settlement < terms.first_issue_date()
            || horizon_settlement <= initial_settlement
            || horizon_settlement >= terms.maturity_date()
        {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        if calendar_requirement == CalendarRequirement::ExactMarket
            && (initial_settlement < calendar.coverage_start()
                || horizon_settlement > calendar.coverage_end())
        {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        let input = Self {
            owner,
            bond,
            rule_pack,
            snapshot,
            valuation_at,
            initial_settlement,
            horizon_settlement,
            calendar_requirement,
            calendar,
            terms,
            curve,
        };
        if !input.curve.covers(input.initial_curve_query_date()?)
            || !input.curve.covers(input.rolled_curve_query_date()?)
        {
            return Err(DomainErrorCode::InvalidEffectiveTime);
        }
        Ok(input)
    }

    fn curve_query_date(&self, settlement: NaiveDate) -> DomainResult<NaiveDate> {
        let residual_days = self
            .terms
            .maturity_date()
            .signed_duration_since(settlement)
            .num_days();
        self.curve
            .valuation_date()
            .checked_add_signed(TimeDelta::days(residual_days))
            .ok_or(DomainErrorCode::InvalidEffectiveTime)
    }

    pub fn initial_curve_query_date(&self) -> DomainResult<NaiveDate> {
        self.curve_query_date(self.initial_settlement)
    }

    pub fn rolled_curve_query_date(&self) -> DomainResult<NaiveDate> {
        self.curve_query_date(self.horizon_settlement)
    }

    /// Returns a deterministic identity over every semantic input field.
    #[must_use]
    pub fn fingerprint(&self) -> ContentHash {
        let mut bytes = Vec::with_capacity(1024);
        field(&mut bytes, b"ficant.carry-roll.input.v1");
        object_ref(
            &mut bytes,
            self.owner.tenant_id().as_str().as_bytes(),
            0,
            &[],
        );
        object_ref(
            &mut bytes,
            self.owner.owner_id().as_str().as_bytes(),
            0,
            &[],
        );
        analytics_ref(&mut bytes, &self.bond);
        analytics_ref(&mut bytes, &self.rule_pack);
        analytics_ref(&mut bytes, &self.snapshot);
        field(
            &mut bytes,
            &self.valuation_at.instant().timestamp().to_be_bytes(),
        );
        field(
            &mut bytes,
            &self
                .valuation_at
                .instant()
                .timestamp_subsec_nanos()
                .to_be_bytes(),
        );
        field(&mut bytes, self.valuation_at.market_timezone().as_bytes());
        date_field(&mut bytes, self.valuation_at.local_trading_date());
        date_field(&mut bytes, self.initial_settlement);
        date_field(&mut bytes, self.horizon_settlement);
        field(&mut bytes, &[self.calendar_requirement as u8]);
        field(&mut bytes, self.calendar.id().as_bytes());
        field(&mut bytes, &self.calendar.version().get().to_be_bytes());
        field(&mut bytes, self.calendar.content_hash().as_bytes());
        date_field(&mut bytes, self.calendar.coverage_start());
        date_field(&mut bytes, self.calendar.coverage_end());
        dates_field(&mut bytes, self.calendar.non_business_days());
        dates_field(&mut bytes, self.calendar.work_weekends());
        date_field(&mut bytes, self.terms.first_issue_date());
        date_field(&mut bytes, self.terms.maturity_date());
        field(&mut bytes, &(self.terms.frequency() as u32).to_be_bytes());
        field(&mut bytes, &(self.terms.day_count() as u32).to_be_bytes());
        field(
            &mut bytes,
            &(self.terms.business_day() as u32).to_be_bytes(),
        );
        field(&mut bytes, &self.terms.coupon_rate().scaled().to_be_bytes());
        field(&mut bytes, &self.terms.face_amount().scaled().to_be_bytes());
        analytics_ref(&mut bytes, self.curve.curve_snapshot());
        date_field(&mut bytes, self.curve.valuation_date());
        field(
            &mut bytes,
            &(self.curve.interpolation() as u32).to_be_bytes(),
        );
        field(
            &mut bytes,
            &u64::try_from(self.curve.nodes().len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        for node in self.curve.nodes() {
            date_field(&mut bytes, node.maturity_date());
            field(&mut bytes, &node.yield_to_maturity().scaled().to_be_bytes());
        }
        ContentHash::digest(&bytes)
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }

    #[must_use]
    pub fn bond(&self) -> &AnalyticsObjectRef {
        &self.bond
    }

    #[must_use]
    pub fn rule_pack(&self) -> &AnalyticsObjectRef {
        &self.rule_pack
    }

    #[must_use]
    pub fn snapshot(&self) -> &AnalyticsObjectRef {
        &self.snapshot
    }

    #[must_use]
    pub fn valuation_at(&self) -> &MarketTime {
        &self.valuation_at
    }

    #[must_use]
    pub const fn initial_settlement(&self) -> NaiveDate {
        self.initial_settlement
    }

    #[must_use]
    pub const fn horizon_settlement(&self) -> NaiveDate {
        self.horizon_settlement
    }

    #[must_use]
    pub const fn calendar_requirement(&self) -> CalendarRequirement {
        self.calendar_requirement
    }

    #[must_use]
    pub fn calendar(&self) -> &CalendarBinding {
        &self.calendar
    }

    #[must_use]
    pub fn terms(&self) -> &BondTerms {
        &self.terms
    }

    #[must_use]
    pub fn curve(&self) -> &YieldCurveBinding {
        &self.curve
    }
}

fn field(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    target.extend_from_slice(value);
}

fn object_ref(target: &mut Vec<u8>, id: &[u8], version: u64, hash: &[u8]) {
    field(target, id);
    field(target, &version.to_be_bytes());
    field(target, hash);
}

fn analytics_ref(target: &mut Vec<u8>, reference: &AnalyticsObjectRef) {
    object_ref(
        target,
        reference.version_ref().id().as_str().as_bytes(),
        reference.version_ref().version().get(),
        reference.content_hash().as_bytes(),
    );
}

fn date_field(target: &mut Vec<u8>, value: NaiveDate) {
    field(target, value.to_string().as_bytes());
}

fn dates_field(target: &mut Vec<u8>, values: &[NaiveDate]) {
    field(
        target,
        &u64::try_from(values.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for value in values {
        date_field(target, *value);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarryRollMeasures {
    initial_yield: FixedDecimal,
    rolled_yield: FixedDecimal,
    initial_dirty_price: FixedDecimal,
    horizon_dirty_at_initial_yield: FixedDecimal,
    horizon_dirty_at_rolled_yield: FixedDecimal,
    paid_cashflows: FixedDecimal,
    carry: FixedDecimal,
    roll_down: FixedDecimal,
    total_return: FixedDecimal,
}

impl CarryRollMeasures {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        initial_yield: FixedDecimal,
        rolled_yield: FixedDecimal,
        initial_dirty_price: FixedDecimal,
        horizon_dirty_at_initial_yield: FixedDecimal,
        horizon_dirty_at_rolled_yield: FixedDecimal,
        paid_cashflows: FixedDecimal,
        carry: FixedDecimal,
        roll_down: FixedDecimal,
        total_return: FixedDecimal,
    ) -> DomainResult<Self> {
        if initial_yield.scaled() <= NEGATIVE_ONE_SCALED
            || rolled_yield.scaled() <= NEGATIVE_ONE_SCALED
            || !initial_dirty_price.is_positive()
            || !horizon_dirty_at_initial_yield.is_positive()
            || !horizon_dirty_at_rolled_yield.is_positive()
            || !paid_cashflows.is_non_negative()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        let expected_carry = horizon_dirty_at_initial_yield
            .checked_add(paid_cashflows)?
            .checked_sub(initial_dirty_price)?;
        let expected_roll =
            horizon_dirty_at_rolled_yield.checked_sub(horizon_dirty_at_initial_yield)?;
        if carry != expected_carry
            || roll_down != expected_roll
            || total_return != carry.checked_add(roll_down)?
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            initial_yield,
            rolled_yield,
            initial_dirty_price,
            horizon_dirty_at_initial_yield,
            horizon_dirty_at_rolled_yield,
            paid_cashflows,
            carry,
            roll_down,
            total_return,
        })
    }

    #[must_use]
    pub const fn initial_yield(self) -> FixedDecimal {
        self.initial_yield
    }
    #[must_use]
    pub const fn rolled_yield(self) -> FixedDecimal {
        self.rolled_yield
    }
    #[must_use]
    pub const fn initial_dirty_price(self) -> FixedDecimal {
        self.initial_dirty_price
    }
    #[must_use]
    pub const fn horizon_dirty_at_initial_yield(self) -> FixedDecimal {
        self.horizon_dirty_at_initial_yield
    }
    #[must_use]
    pub const fn horizon_dirty_at_rolled_yield(self) -> FixedDecimal {
        self.horizon_dirty_at_rolled_yield
    }
    #[must_use]
    pub const fn paid_cashflows(self) -> FixedDecimal {
        self.paid_cashflows
    }
    #[must_use]
    pub const fn carry(self) -> FixedDecimal {
        self.carry
    }
    #[must_use]
    pub const fn roll_down(self) -> FixedDecimal {
        self.roll_down
    }
    #[must_use]
    pub const fn total_return(self) -> FixedDecimal {
        self.total_return
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarryRollResult {
    input: CarryRollInput,
    measures: CarryRollMeasures,
}

impl CarryRollResult {
    #[must_use]
    pub fn new(input: CarryRollInput, measures: CarryRollMeasures) -> Self {
        Self { input, measures }
    }

    pub fn validate_against(&self, input: &CarryRollInput) -> DomainResult<()> {
        if &self.input != input {
            return Err(DomainErrorCode::BrokenLineage);
        }
        Ok(())
    }

    #[must_use]
    pub fn input(&self) -> &CarryRollInput {
        &self.input
    }

    #[must_use]
    pub const fn measures(&self) -> CarryRollMeasures {
        self.measures
    }

    #[must_use]
    pub fn schema_id(&self) -> &'static str {
        CARRY_ROLL_RESULT_SCHEMA_ID
    }

    #[must_use]
    pub fn algorithm_id(&self) -> &'static str {
        CARRY_ROLL_ALGORITHM_ID
    }

    #[must_use]
    pub const fn algorithm_version(&self) -> u32 {
        CARRY_ROLL_ALGORITHM_VERSION
    }

    #[must_use]
    pub fn convention_profile(&self) -> &'static str {
        CARRY_ROLL_CONVENTION_PROFILE
    }
}
