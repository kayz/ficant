use std::collections::BTreeSet;

use chrono::{Datelike, Months, NaiveDate};

use crate::analytics::{AnalyticsObjectRef, BondTerms, FixedDecimal, MARKET_TIMEZONE};
use crate::primitives::{MarketTime, OwnerRef};
use crate::{DomainErrorCode, DomainResult};

pub const FUTURES_DELIVERY_RESULT_SCHEMA_ID: &str = "ficant.cgb-futures-delivery.result.v1";
pub const FUTURES_DELIVERY_ARTIFACT_SCHEMA_ID: &str = "ficant.cgb-futures-delivery.arrow.v1";
pub const FUTURES_DELIVERY_ARTIFACT_CODEC_ID: &str = "ficant-cgb-futures-delivery-arrow/1";
pub const FUTURES_DELIVERY_ALGORITHM_ID: &str = "ficant.cffex.cgb-futures-delivery";
pub const FUTURES_DELIVERY_ALGORITHM_VERSION: u32 = 1;
pub const FUTURES_DELIVERY_CONVENTION_PROFILE: &str = "cffex-cgb-futures-delivery-v1";

const FACE_PER_HUNDRED_SCALED: i128 = 100_000_000_000_000;

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

    const fn original_term_months(self) -> u32 {
        match self {
            Self::TwoYear => 60,
            Self::FiveYear => 84,
            Self::TenYear => 120,
            Self::ThirtyYear => 360,
        }
    }

    const fn residual_term_bounds(self) -> (u32, Option<u32>) {
        match self {
            Self::TwoYear => (18, Some(27)),
            Self::FiveYear => (48, Some(63)),
            Self::TenYear => (78, None),
            Self::ThirtyYear => (300, None),
        }
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
        terms: BondTerms,
        spot_clean_price: FixedDecimal,
        futures_clean_price: FixedDecimal,
        financing_rate: FixedDecimal,
    ) -> DomainResult<Self> {
        if valuation_at.market_timezone() != MARKET_TIMEZONE
            || valuation_at.local_trading_date() > purchase_date
            || purchase_date < terms.issue_date()
            || purchase_date >= delivery_date
            || delivery_date >= terms.maturity_date()
            || delivery_month_first.day() != 1
            || !matches!(delivery_month_first.month(), 3 | 6 | 9 | 12)
            || delivery_date.year() != delivery_month_first.year()
            || delivery_date.month() != delivery_month_first.month()
            || !terms.coupon_rate().is_positive()
            || terms.face_amount().scaled() != FACE_PER_HUNDRED_SCALED
            || !spot_clean_price.is_positive()
            || !futures_clean_price.is_positive()
            || !financing_rate.is_non_negative()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        if !is_deliverable(product, &terms, delivery_month_first)? {
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
}

pub fn is_deliverable(
    product: CgbFuturesProduct,
    terms: &BondTerms,
    delivery_month_first: NaiveDate,
) -> DomainResult<bool> {
    let original_limit = terms
        .issue_date()
        .checked_add_months(Months::new(product.original_term_months()))
        .ok_or(DomainErrorCode::InvalidValue)?;
    let (minimum_months, maximum_months) = product.residual_term_bounds();
    let minimum_maturity = delivery_month_first
        .checked_add_months(Months::new(minimum_months))
        .ok_or(DomainErrorCode::InvalidValue)?;
    let maximum_maturity = match maximum_months {
        Some(months) => Some(
            delivery_month_first
                .checked_add_months(Months::new(months))
                .ok_or(DomainErrorCode::InvalidValue)?,
        ),
        None => None,
    };
    Ok(terms.maturity_date() <= original_limit
        && terms.maturity_date() >= minimum_maturity
        && maximum_maturity.is_none_or(|maximum| terms.maturity_date() <= maximum))
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
