use std::collections::BTreeSet;

use chrono::{Datelike, Months, NaiveDate};

use crate::analytics::{AnalyticsObjectRef, BondTerms, MARKET_TIMEZONE};
use crate::primitives::{ContentHash, FixedDecimal, MarketTime, OwnerRef};
use crate::{DomainErrorCode, DomainResult};

pub const FUTURES_DELIVERY_RESULT_SCHEMA_ID: &str = "ficant.cgb-futures-delivery.result.v1";
pub const FUTURES_DELIVERY_ARTIFACT_SCHEMA_ID: &str = "ficant.cgb-futures-delivery.arrow.v1";
pub const FUTURES_DELIVERY_ARTIFACT_CODEC_ID: &str = "ficant-cgb-futures-delivery-arrow/1";
pub const FUTURES_DELIVERY_ALGORITHM_ID: &str = "ficant.cffex.cgb-futures-delivery";
pub const FUTURES_DELIVERY_ALGORITHM_VERSION: u32 = 1;
pub const FUTURES_DELIVERY_CONVENTION_PROFILE: &str = "cffex-cgb-futures-delivery-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CgbFuturesProduct {
    TwoYear = 1,
    FiveYear = 2,
    TenYear = 3,
    ThirtyYear = 4,
}

impl CgbFuturesProduct {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::TwoYear => "TS",
            Self::FiveYear => "TF",
            Self::TenYear => "T",
            Self::ThirtyYear => "TL",
        }
    }
}

/// Provider-neutral delivery-rule shape injected from an exact `RulePack`.
///
/// It deliberately contains no market or product branch. The L3 parser selects this shape using
/// the public product identity before L2/L0 calculation begins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesDeliveryRule {
    original_term_max_months: u32,
    residual_min_months: u32,
    residual_max_months: Option<u32>,
    delivery_months: Vec<u32>,
    nominal_coupon: FixedDecimal,
    face_quote_basis: FixedDecimal,
    accrued_interest_day_count: u32,
    conversion_factor_rounding_places: u32,
    accrued_interest_rounding_places: u32,
    annual_day_basis: u32,
    contract_size_in_quote_units: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesDeliveryRuleInput {
    pub original_term_max_months: u32,
    pub residual_min_months: u32,
    pub residual_max_months: Option<u32>,
    pub delivery_months: Vec<u32>,
    pub nominal_coupon: FixedDecimal,
    pub face_quote_basis: FixedDecimal,
    pub accrued_interest_day_count: u32,
    pub conversion_factor_rounding_places: u32,
    pub accrued_interest_rounding_places: u32,
    pub annual_day_basis: u32,
}

impl FuturesDeliveryRule {
    /// Creates one complete set of already-parsed delivery rules.
    ///
    /// # Errors
    ///
    /// Returns validation failure for missing, unordered, incompatible, or non-positive rules.
    pub fn new(input: FuturesDeliveryRuleInput) -> DomainResult<Self> {
        let FuturesDeliveryRuleInput {
            original_term_max_months,
            residual_min_months,
            residual_max_months,
            delivery_months,
            nominal_coupon,
            face_quote_basis,
            accrued_interest_day_count,
            conversion_factor_rounding_places,
            accrued_interest_rounding_places,
            annual_day_basis,
        } = input;
        let delivery_months_valid = !delivery_months.is_empty()
            && delivery_months.iter().all(|month| (1..=12).contains(month))
            && delivery_months.windows(2).all(|pair| pair[0] < pair[1]);
        if original_term_max_months == 0
            || residual_min_months == 0
            || residual_max_months.is_some_and(|value| value < residual_min_months)
            || !delivery_months_valid
            || !nominal_coupon.is_positive()
            || !face_quote_basis.is_positive()
            || accrued_interest_day_count == 0
            || conversion_factor_rounding_places > 12
            || accrued_interest_rounding_places > 12
            || annual_day_basis == 0
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            original_term_max_months,
            residual_min_months,
            residual_max_months,
            delivery_months,
            nominal_coupon,
            face_quote_basis,
            accrued_interest_day_count,
            conversion_factor_rounding_places,
            accrued_interest_rounding_places,
            annual_day_basis,
            contract_size_in_quote_units: None,
        })
    }

    /// Adds the product-specific L3 contract size used only by portfolio risk.
    pub fn with_contract_size_in_quote_units(mut self, value: u32) -> DomainResult<Self> {
        if value == 0 {
            return Err(DomainErrorCode::InvalidValue);
        }
        self.contract_size_in_quote_units = Some(value);
        Ok(self)
    }

    #[must_use]
    pub const fn original_term_max_months(&self) -> u32 {
        self.original_term_max_months
    }

    #[must_use]
    pub const fn residual_min_months(&self) -> u32 {
        self.residual_min_months
    }

    #[must_use]
    pub const fn residual_max_months(&self) -> Option<u32> {
        self.residual_max_months
    }

    #[must_use]
    pub fn delivery_months(&self) -> &[u32] {
        &self.delivery_months
    }

    #[must_use]
    pub const fn nominal_coupon(&self) -> FixedDecimal {
        self.nominal_coupon
    }

    #[must_use]
    pub const fn face_quote_basis(&self) -> FixedDecimal {
        self.face_quote_basis
    }

    #[must_use]
    pub const fn accrued_interest_day_count(&self) -> u32 {
        self.accrued_interest_day_count
    }

    #[must_use]
    pub const fn conversion_factor_rounding_places(&self) -> u32 {
        self.conversion_factor_rounding_places
    }

    #[must_use]
    pub const fn accrued_interest_rounding_places(&self) -> u32 {
        self.accrued_interest_rounding_places
    }

    #[must_use]
    pub const fn annual_day_basis(&self) -> u32 {
        self.annual_day_basis
    }

    #[must_use]
    pub const fn contract_size_in_quote_units(&self) -> Option<u32> {
        self.contract_size_in_quote_units
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesDeliverableInput {
    owner: OwnerRef,
    futures_contract: AnalyticsObjectRef,
    bond: AnalyticsObjectRef,
    rule_pack: AnalyticsObjectRef,
    snapshot: AnalyticsObjectRef,
    valuation_at: MarketTime,
    purchase_date: NaiveDate,
    delivery_month_first: NaiveDate,
    delivery_date: NaiveDate,
    product: CgbFuturesProduct,
    rule: FuturesDeliveryRule,
    terms: BondTerms,
    spot_clean_price: FixedDecimal,
    futures_clean_price: FixedDecimal,
    financing_rate: FixedDecimal,
}

impl FuturesDeliverableInput {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner: OwnerRef,
        futures_contract: AnalyticsObjectRef,
        bond: AnalyticsObjectRef,
        rule_pack: AnalyticsObjectRef,
        snapshot: AnalyticsObjectRef,
        valuation_at: MarketTime,
        purchase_date: NaiveDate,
        delivery_month_first: NaiveDate,
        delivery_date: NaiveDate,
        product: CgbFuturesProduct,
        rule: FuturesDeliveryRule,
        terms: BondTerms,
        spot_clean_price: FixedDecimal,
        futures_clean_price: FixedDecimal,
        financing_rate: FixedDecimal,
    ) -> DomainResult<Self> {
        if valuation_at.market_timezone() != MARKET_TIMEZONE
            || valuation_at.local_trading_date() > purchase_date
            || purchase_date < terms.first_issue_date()
            || purchase_date >= delivery_date
            || delivery_date >= terms.maturity_date()
            || delivery_month_first.day() != 1
            || rule
                .delivery_months()
                .binary_search(&delivery_month_first.month())
                .is_err()
            || delivery_date.year() != delivery_month_first.year()
            || delivery_date.month() != delivery_month_first.month()
            || !terms.coupon_rate().is_positive()
            || terms.face_amount() != rule.face_quote_basis()
            || !spot_clean_price.is_positive()
            || !futures_clean_price.is_positive()
            || !financing_rate.is_non_negative()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        if !is_deliverable(&rule, &terms, delivery_month_first)? {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            owner,
            futures_contract,
            bond,
            rule_pack,
            snapshot,
            valuation_at,
            purchase_date,
            delivery_month_first,
            delivery_date,
            product,
            rule,
            terms,
            spot_clean_price,
            futures_clean_price,
            financing_rate,
        })
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }
    #[must_use]
    pub fn futures_contract(&self) -> &AnalyticsObjectRef {
        &self.futures_contract
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
    pub const fn purchase_date(&self) -> NaiveDate {
        self.purchase_date
    }
    #[must_use]
    pub const fn delivery_month_first(&self) -> NaiveDate {
        self.delivery_month_first
    }
    #[must_use]
    pub const fn delivery_date(&self) -> NaiveDate {
        self.delivery_date
    }
    #[must_use]
    pub const fn product(&self) -> CgbFuturesProduct {
        self.product
    }
    #[must_use]
    pub fn rule(&self) -> &FuturesDeliveryRule {
        &self.rule
    }
    #[must_use]
    pub fn terms(&self) -> &BondTerms {
        &self.terms
    }
    #[must_use]
    pub const fn spot_clean_price(&self) -> FixedDecimal {
        self.spot_clean_price
    }
    #[must_use]
    pub const fn futures_clean_price(&self) -> FixedDecimal {
        self.futures_clean_price
    }
    #[must_use]
    pub const fn financing_rate(&self) -> FixedDecimal {
        self.financing_rate
    }

    #[must_use]
    pub fn fingerprint(&self) -> ContentHash {
        let mut bytes = Vec::new();
        field(&mut bytes, FUTURES_DELIVERY_ALGORITHM_ID.as_bytes());
        field(
            &mut bytes,
            &FUTURES_DELIVERY_ALGORITHM_VERSION.to_be_bytes(),
        );
        field(&mut bytes, self.owner.tenant_id().as_str().as_bytes());
        field(&mut bytes, self.owner.owner_id().as_str().as_bytes());
        for reference in [
            &self.futures_contract,
            &self.bond,
            &self.rule_pack,
            &self.snapshot,
        ] {
            field(&mut bytes, reference.version_ref().id().as_str().as_bytes());
            field(
                &mut bytes,
                &reference.version_ref().version().get().to_be_bytes(),
            );
            field(&mut bytes, reference.content_hash().as_bytes());
        }
        field(
            &mut bytes,
            &self.valuation_at.instant().timestamp_micros().to_be_bytes(),
        );
        field(&mut bytes, self.valuation_at.market_timezone().as_bytes());
        for value in [
            self.valuation_at.local_trading_date(),
            self.purchase_date,
            self.delivery_month_first,
            self.delivery_date,
            self.terms.first_issue_date(),
            self.terms.maturity_date(),
        ] {
            field(&mut bytes, value.to_string().as_bytes());
        }
        field(&mut bytes, &(self.product as u32).to_be_bytes());
        field(
            &mut bytes,
            &self.rule.original_term_max_months().to_be_bytes(),
        );
        field(&mut bytes, &self.rule.residual_min_months().to_be_bytes());
        match self.rule.residual_max_months() {
            Some(value) => {
                field(&mut bytes, &[1]);
                field(&mut bytes, &value.to_be_bytes());
            }
            None => field(&mut bytes, &[0]),
        }
        field(
            &mut bytes,
            &u64::try_from(self.rule.delivery_months().len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        for month in self.rule.delivery_months() {
            field(&mut bytes, &month.to_be_bytes());
        }
        for value in [self.rule.nominal_coupon(), self.rule.face_quote_basis()] {
            field(&mut bytes, &value.scaled().to_be_bytes());
        }
        for value in [
            self.rule.accrued_interest_day_count(),
            self.rule.conversion_factor_rounding_places(),
            self.rule.accrued_interest_rounding_places(),
            self.rule.annual_day_basis(),
        ] {
            field(&mut bytes, &value.to_be_bytes());
        }
        field(&mut bytes, &(self.terms.frequency() as u32).to_be_bytes());
        for value in [
            self.terms.coupon_rate(),
            self.terms.face_amount(),
            self.spot_clean_price,
            self.futures_clean_price,
            self.financing_rate,
        ] {
            field(&mut bytes, &value.scaled().to_be_bytes());
        }
        ContentHash::digest(&bytes)
    }
}

fn field(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    target.extend_from_slice(value);
}

pub fn is_deliverable(
    rule: &FuturesDeliveryRule,
    terms: &BondTerms,
    delivery_month_first: NaiveDate,
) -> DomainResult<bool> {
    is_deliverable_by_dates(
        rule,
        terms.first_issue_date(),
        terms.maturity_date(),
        delivery_month_first,
    )
}

/// Evaluates delivery eligibility from provider-neutral registered dates.
///
/// The caller remains responsible for resolving and verifying the source of these dates. This
/// helper applies only the already-parsed rule and contains no market, product, or provider branch.
///
/// # Errors
///
/// Returns validation failure when a rule-bound date cannot be represented.
pub fn is_deliverable_by_dates(
    rule: &FuturesDeliveryRule,
    first_issue_date: NaiveDate,
    maturity_date: NaiveDate,
    delivery_month_first: NaiveDate,
) -> DomainResult<bool> {
    let original_limit = first_issue_date
        .checked_add_months(Months::new(rule.original_term_max_months()))
        .ok_or(DomainErrorCode::InvalidValue)?;
    let minimum_maturity = delivery_month_first
        .checked_add_months(Months::new(rule.residual_min_months()))
        .ok_or(DomainErrorCode::InvalidValue)?;
    let maximum_maturity = match rule.residual_max_months() {
        Some(months) => Some(
            delivery_month_first
                .checked_add_months(Months::new(months))
                .ok_or(DomainErrorCode::InvalidValue)?,
        ),
        None => None,
    };
    Ok(maturity_date <= original_limit
        && maturity_date >= minimum_maturity
        && maximum_maturity.is_none_or(|maximum| maturity_date <= maximum))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuturesDeliveryMeasures {
    months_to_next_coupon: u32,
    remaining_coupon_count: u32,
    conversion_factor: FixedDecimal,
    purchase_accrued_interest: FixedDecimal,
    delivery_accrued_interest: FixedDecimal,
    interim_coupons: FixedDecimal,
    invoice_price: FixedDecimal,
    purchase_dirty_price: FixedDecimal,
    gross_basis: FixedDecimal,
    financing_cost: FixedDecimal,
    holding_carry: FixedDecimal,
    net_basis: FixedDecimal,
    implied_repo_rate: FixedDecimal,
    delivery_profit: FixedDecimal,
}

impl FuturesDeliveryMeasures {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        months_to_next_coupon: u32,
        remaining_coupon_count: u32,
        conversion_factor: FixedDecimal,
        purchase_accrued_interest: FixedDecimal,
        delivery_accrued_interest: FixedDecimal,
        interim_coupons: FixedDecimal,
        invoice_price: FixedDecimal,
        purchase_dirty_price: FixedDecimal,
        gross_basis: FixedDecimal,
        financing_cost: FixedDecimal,
        holding_carry: FixedDecimal,
        net_basis: FixedDecimal,
        implied_repo_rate: FixedDecimal,
        delivery_profit: FixedDecimal,
    ) -> DomainResult<Self> {
        if remaining_coupon_count == 0
            || !conversion_factor.is_positive()
            || !purchase_accrued_interest.is_non_negative()
            || !delivery_accrued_interest.is_non_negative()
            || !interim_coupons.is_non_negative()
            || !invoice_price.is_positive()
            || !purchase_dirty_price.is_positive()
            || !financing_cost.is_non_negative()
            || net_basis != gross_basis.checked_sub(holding_carry)?
            || delivery_profit != FixedDecimal::ZERO.checked_sub(net_basis)?
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            months_to_next_coupon,
            remaining_coupon_count,
            conversion_factor,
            purchase_accrued_interest,
            delivery_accrued_interest,
            interim_coupons,
            invoice_price,
            purchase_dirty_price,
            gross_basis,
            financing_cost,
            holding_carry,
            net_basis,
            implied_repo_rate,
            delivery_profit,
        })
    }

    #[must_use]
    pub const fn months_to_next_coupon(self) -> u32 {
        self.months_to_next_coupon
    }
    #[must_use]
    pub const fn remaining_coupon_count(self) -> u32 {
        self.remaining_coupon_count
    }
    #[must_use]
    pub const fn conversion_factor(self) -> FixedDecimal {
        self.conversion_factor
    }
    #[must_use]
    pub const fn purchase_accrued_interest(self) -> FixedDecimal {
        self.purchase_accrued_interest
    }
    #[must_use]
    pub const fn delivery_accrued_interest(self) -> FixedDecimal {
        self.delivery_accrued_interest
    }
    #[must_use]
    pub const fn interim_coupons(self) -> FixedDecimal {
        self.interim_coupons
    }
    #[must_use]
    pub const fn invoice_price(self) -> FixedDecimal {
        self.invoice_price
    }
    #[must_use]
    pub const fn purchase_dirty_price(self) -> FixedDecimal {
        self.purchase_dirty_price
    }
    #[must_use]
    pub const fn gross_basis(self) -> FixedDecimal {
        self.gross_basis
    }
    #[must_use]
    pub const fn financing_cost(self) -> FixedDecimal {
        self.financing_cost
    }
    #[must_use]
    pub const fn holding_carry(self) -> FixedDecimal {
        self.holding_carry
    }
    #[must_use]
    pub const fn net_basis(self) -> FixedDecimal {
        self.net_basis
    }
    #[must_use]
    pub const fn implied_repo_rate(self) -> FixedDecimal {
        self.implied_repo_rate
    }
    #[must_use]
    pub const fn delivery_profit(self) -> FixedDecimal {
        self.delivery_profit
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesDeliveryResult {
    input: FuturesDeliverableInput,
    measures: FuturesDeliveryMeasures,
}

impl FuturesDeliveryResult {
    #[must_use]
    pub fn new(input: FuturesDeliverableInput, measures: FuturesDeliveryMeasures) -> Self {
        Self { input, measures }
    }
    pub fn validate_against(&self, input: &FuturesDeliverableInput) -> DomainResult<()> {
        if &self.input != input {
            return Err(DomainErrorCode::BrokenLineage);
        }
        Ok(())
    }
    #[must_use]
    pub fn input(&self) -> &FuturesDeliverableInput {
        &self.input
    }
    #[must_use]
    pub const fn measures(&self) -> FuturesDeliveryMeasures {
        self.measures
    }
    #[must_use]
    pub fn schema_id(&self) -> &'static str {
        FUTURES_DELIVERY_RESULT_SCHEMA_ID
    }
    #[must_use]
    pub fn algorithm_id(&self) -> &'static str {
        FUTURES_DELIVERY_ALGORITHM_ID
    }
    #[must_use]
    pub const fn algorithm_version(&self) -> u32 {
        FUTURES_DELIVERY_ALGORITHM_VERSION
    }
    #[must_use]
    pub fn convention_profile(&self) -> &'static str {
        FUTURES_DELIVERY_CONVENTION_PROFILE
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesDeliveryBasketResult {
    candidates: Vec<FuturesDeliveryResult>,
    ctd_index: usize,
}

impl FuturesDeliveryBasketResult {
    pub fn new(candidates: Vec<FuturesDeliveryResult>, ctd_index: usize) -> DomainResult<Self> {
        if candidates.is_empty() || ctd_index >= candidates.len() {
            return Err(DomainErrorCode::InvalidValue);
        }
        let mut bonds = BTreeSet::new();
        if candidates
            .iter()
            .any(|candidate| !bonds.insert(candidate.input().bond().version_ref().id()))
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            candidates,
            ctd_index,
        })
    }
    #[must_use]
    pub fn candidates(&self) -> &[FuturesDeliveryResult] {
        &self.candidates
    }
    #[must_use]
    pub const fn ctd_index(&self) -> usize {
        self.ctd_index
    }
    #[must_use]
    pub fn ctd(&self) -> &FuturesDeliveryResult {
        &self.candidates[self.ctd_index]
    }
}
