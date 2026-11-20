use crate::analytics::{AnalyticsObjectRef, MARKET_TIMEZONE};
use crate::futures_delivery::CgbFuturesProduct;
use crate::primitives::{ContentHash, FixedDecimal, MarketTime, OwnerRef};
use crate::{DomainErrorCode, DomainResult};

pub const FUTURES_HEDGE_RESULT_SCHEMA_ID: &str = "ficant.cgb-futures-hedge.result.v1";
pub const FUTURES_HEDGE_ARTIFACT_SCHEMA_ID: &str = "ficant.cgb-futures-hedge.arrow.v1";
pub const FUTURES_HEDGE_ARTIFACT_CODEC_ID: &str = "ficant-cgb-futures-hedge-arrow/1";
pub const FUTURES_HEDGE_ALGORITHM_ID: &str = "ficant.cffex.cgb-futures-dv01-hedge";
pub const FUTURES_HEDGE_ALGORITHM_VERSION: u32 = 1;
pub const FUTURES_HEDGE_CONVENTION_PROFILE: &str = "cffex-cgb-futures-dv01-hedge-v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesHedgeInput {
    owner: OwnerRef,
    target_risk_artifact: AnalyticsObjectRef,
    delivery_artifact: AnalyticsObjectRef,
    ctd_analytics_artifact: AnalyticsObjectRef,
    futures_contract: AnalyticsObjectRef,
    ctd_bond: AnalyticsObjectRef,
    rule_pack: AnalyticsObjectRef,
    snapshot: AnalyticsObjectRef,
    valuation_at: Option<MarketTime>,
    product: CgbFuturesProduct,
    contract_notional: FixedDecimal,
    face_quote_basis: FixedDecimal,
    target_dv01: FixedDecimal,
    ctd_dv01_per_quote_unit: FixedDecimal,
    conversion_factor: FixedDecimal,
}

impl FuturesHedgeInput {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        owner: OwnerRef,
        target_risk_artifact: AnalyticsObjectRef,
        delivery_artifact: AnalyticsObjectRef,
        ctd_analytics_artifact: AnalyticsObjectRef,
        futures_contract: AnalyticsObjectRef,
        ctd_bond: AnalyticsObjectRef,
        rule_pack: AnalyticsObjectRef,
        snapshot: AnalyticsObjectRef,
        valuation_at: Option<MarketTime>,
        product: CgbFuturesProduct,
        contract_notional: FixedDecimal,
        face_quote_basis: FixedDecimal,
        target_dv01: FixedDecimal,
        ctd_dv01_per_quote_unit: FixedDecimal,
        conversion_factor: FixedDecimal,
    ) -> DomainResult<Self> {
        if valuation_at
            .as_ref()
            .is_some_and(|time| time.market_timezone() != MARKET_TIMEZONE)
            || !contract_notional.is_positive()
            || !face_quote_basis.is_positive()
            || target_dv01 == FixedDecimal::ZERO
            || !ctd_dv01_per_quote_unit.is_positive()
            || !conversion_factor.is_positive()
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            owner,
            target_risk_artifact,
            delivery_artifact,
            ctd_analytics_artifact,
            futures_contract,
            ctd_bond,
            rule_pack,
            snapshot,
            valuation_at,
            product,
            contract_notional,
            face_quote_basis,
            target_dv01,
            ctd_dv01_per_quote_unit,
            conversion_factor,
        })
    }

    #[must_use]
    pub fn owner(&self) -> &OwnerRef {
        &self.owner
    }
    #[must_use]
    pub fn target_risk_artifact(&self) -> &AnalyticsObjectRef {
        &self.target_risk_artifact
    }
    #[must_use]
    pub fn delivery_artifact(&self) -> &AnalyticsObjectRef {
        &self.delivery_artifact
    }
    #[must_use]
    pub fn ctd_analytics_artifact(&self) -> &AnalyticsObjectRef {
        &self.ctd_analytics_artifact
    }
    #[must_use]
    pub fn futures_contract(&self) -> &AnalyticsObjectRef {
        &self.futures_contract
    }
    #[must_use]
    pub fn ctd_bond(&self) -> &AnalyticsObjectRef {
        &self.ctd_bond
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
    pub fn valuation_at(&self) -> Option<&MarketTime> {
        self.valuation_at.as_ref()
    }
    #[must_use]
    pub const fn product(&self) -> CgbFuturesProduct {
        self.product
    }
    #[must_use]
    pub const fn contract_notional(&self) -> FixedDecimal {
        self.contract_notional
    }
    #[must_use]
    pub const fn face_quote_basis(&self) -> FixedDecimal {
        self.face_quote_basis
    }
    #[must_use]
    pub const fn target_dv01(&self) -> FixedDecimal {
        self.target_dv01
    }
    #[must_use]
    pub const fn ctd_dv01_per_quote_unit(&self) -> FixedDecimal {
        self.ctd_dv01_per_quote_unit
    }
    #[must_use]
    pub const fn conversion_factor(&self) -> FixedDecimal {
        self.conversion_factor
    }

    #[must_use]
    pub fn fingerprint(&self) -> ContentHash {
        let mut bytes = Vec::new();
        field(&mut bytes, FUTURES_HEDGE_ALGORITHM_ID.as_bytes());
        field(&mut bytes, &FUTURES_HEDGE_ALGORITHM_VERSION.to_be_bytes());
        field(&mut bytes, self.owner.tenant_id().as_str().as_bytes());
        field(&mut bytes, self.owner.owner_id().as_str().as_bytes());
        for reference in [
            &self.target_risk_artifact,
            &self.delivery_artifact,
            &self.ctd_analytics_artifact,
            &self.futures_contract,
            &self.ctd_bond,
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
        match &self.valuation_at {
            Some(valuation_at) => {
                field(&mut bytes, &[1]);
                field(
                    &mut bytes,
                    &valuation_at.instant().timestamp_micros().to_be_bytes(),
                );
                field(&mut bytes, valuation_at.market_timezone().as_bytes());
                field(
                    &mut bytes,
                    valuation_at.local_trading_date().to_string().as_bytes(),
                );
            }
            None => field(&mut bytes, &[0]),
        }
        field(&mut bytes, &(self.product as u32).to_be_bytes());
        for value in [
            self.contract_notional,
            self.face_quote_basis,
            self.target_dv01,
            self.ctd_dv01_per_quote_unit,
            self.conversion_factor,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FuturesHedgeMeasures {
    futures_contract_dv01: FixedDecimal,
    raw_contracts: FixedDecimal,
    recommended_contracts: i64,
    residual_dv01: FixedDecimal,
    hedge_effectiveness: FixedDecimal,
}

impl FuturesHedgeMeasures {
    pub fn new(
        futures_contract_dv01: FixedDecimal,
        raw_contracts: FixedDecimal,
        recommended_contracts: i64,
        residual_dv01: FixedDecimal,
        hedge_effectiveness: FixedDecimal,
    ) -> DomainResult<Self> {
        if !futures_contract_dv01.is_positive()
            || raw_contracts == FixedDecimal::ZERO
            || !hedge_effectiveness.is_non_negative()
            || hedge_effectiveness > FixedDecimal::ONE
        {
            return Err(DomainErrorCode::InvalidValue);
        }
        Ok(Self {
            futures_contract_dv01,
            raw_contracts,
            recommended_contracts,
            residual_dv01,
            hedge_effectiveness,
        })
    }

    #[must_use]
    pub const fn futures_contract_dv01(self) -> FixedDecimal {
        self.futures_contract_dv01
    }
    #[must_use]
    pub const fn raw_contracts(self) -> FixedDecimal {
        self.raw_contracts
    }
    #[must_use]
    pub const fn recommended_contracts(self) -> i64 {
        self.recommended_contracts
    }
    #[must_use]
    pub const fn residual_dv01(self) -> FixedDecimal {
        self.residual_dv01
    }
    #[must_use]
    pub const fn hedge_effectiveness(self) -> FixedDecimal {
        self.hedge_effectiveness
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesHedgeResult {
    input: FuturesHedgeInput,
    measures: FuturesHedgeMeasures,
}

impl FuturesHedgeResult {
    #[must_use]
    pub fn new(input: FuturesHedgeInput, measures: FuturesHedgeMeasures) -> Self {
        Self { input, measures }
    }
    pub fn validate_against(&self, input: &FuturesHedgeInput) -> DomainResult<()> {
        if &self.input != input {
            return Err(DomainErrorCode::BrokenLineage);
        }
        Ok(())
    }
    #[must_use]
    pub fn input(&self) -> &FuturesHedgeInput {
        &self.input
    }
    #[must_use]
    pub const fn measures(&self) -> FuturesHedgeMeasures {
        self.measures
    }
    #[must_use]
    pub fn schema_id(&self) -> &'static str {
        FUTURES_HEDGE_RESULT_SCHEMA_ID
    }
    #[must_use]
    pub fn algorithm_id(&self) -> &'static str {
        FUTURES_HEDGE_ALGORITHM_ID
    }
    #[must_use]
    pub const fn algorithm_version(&self) -> u32 {
        FUTURES_HEDGE_ALGORITHM_VERSION
    }
    #[must_use]
    pub fn convention_profile(&self) -> &'static str {
        FUTURES_HEDGE_CONVENTION_PROFILE
    }
}
