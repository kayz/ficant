use chrono::NaiveDate;
use ficant_domain::analytics::FixedDecimal;
use ficant_domain::market::{BondTaxAttributes, RulePackContent};
use ficant_domain::primitives::{ContentHash, UnitRef};
use ficant_domain::subject::TaxTreatment;

use super::ApplicationResult;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrossCouponTaxBasis {
    LegacyRetainedRate,
    VatIncluded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaxRoundingMode {
    Exact,
    TiesToEven,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CouponTaxClaimScope {
    LegacySyntheticRetainedRate,
    CouponOutputVatBeforeInputCredit,
}

/// Provider-neutral coupon-tax treatment selected from one parsed L3 rule pack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CouponTaxTreatment {
    value_added_tax_rate: FixedDecimal,
    income_tax_rate: FixedDecimal,
    unit: UnitRef,
    gross_coupon_basis: GrossCouponTaxBasis,
    rounding: TaxRoundingMode,
    claim_scope: CouponTaxClaimScope,
    authority_semantic_hash: Option<ContentHash>,
}

impl CouponTaxTreatment {
    /// Compatibility constructor for the explicitly selected v1 parser.
    #[must_use]
    pub const fn new(coupon_tax_rate: FixedDecimal, unit: UnitRef) -> Self {
        Self::legacy_retained_rate(coupon_tax_rate, unit)
    }

    /// Constructs the explicit v1 synthetic retained-rate compatibility treatment.
    #[must_use]
    pub const fn legacy_retained_rate(coupon_tax_rate: FixedDecimal, unit: UnitRef) -> Self {
        Self {
            value_added_tax_rate: coupon_tax_rate,
            income_tax_rate: FixedDecimal::ZERO,
            unit,
            gross_coupon_basis: GrossCouponTaxBasis::LegacyRetainedRate,
            rounding: TaxRoundingMode::Exact,
            claim_scope: CouponTaxClaimScope::LegacySyntheticRetainedRate,
            authority_semantic_hash: None,
        }
    }

    /// Constructs an explicit VAT-included treatment.
    ///
    /// # Errors
    ///
    /// Fails closed unless both rates are in `[0, 1]` and an authority semantic proof is bound.
    pub fn vat_included(
        value_added_tax_rate: FixedDecimal,
        income_tax_rate: FixedDecimal,
        unit: UnitRef,
        authority_semantic_hash: ContentHash,
    ) -> ApplicationResult<Self> {
        if !value_added_tax_rate.is_non_negative()
            || value_added_tax_rate > FixedDecimal::ONE
            || !income_tax_rate.is_non_negative()
            || income_tax_rate > FixedDecimal::ONE
        {
            return Err(crate::map_domain_error(
                ficant_domain::DomainErrorCode::InvalidValue,
            ));
        }
        Ok(Self {
            value_added_tax_rate,
            income_tax_rate,
            unit,
            gross_coupon_basis: GrossCouponTaxBasis::VatIncluded,
            rounding: TaxRoundingMode::TiesToEven,
            claim_scope: CouponTaxClaimScope::CouponOutputVatBeforeInputCredit,
            authority_semantic_hash: Some(authority_semantic_hash),
        })
    }

    #[must_use]
    pub const fn value_added_tax_rate(&self) -> FixedDecimal {
        self.value_added_tax_rate
    }

    /// Compatibility accessor for the v1 synthetic retained-rate mechanism.
    #[must_use]
    pub const fn coupon_tax_rate(&self) -> FixedDecimal {
        self.value_added_tax_rate
    }

    #[must_use]
    pub const fn income_tax_rate(&self) -> FixedDecimal {
        self.income_tax_rate
    }

    #[must_use]
    pub fn unit(&self) -> &UnitRef {
        &self.unit
    }

    #[must_use]
    pub const fn gross_coupon_basis(&self) -> GrossCouponTaxBasis {
        self.gross_coupon_basis
    }

    #[must_use]
    pub const fn rounding(&self) -> TaxRoundingMode {
        self.rounding
    }

    #[must_use]
    pub const fn claim_scope(&self) -> CouponTaxClaimScope {
        self.claim_scope
    }

    #[must_use]
    pub const fn authority_semantic_hash(&self) -> Option<&ContentHash> {
        self.authority_semantic_hash.as_ref()
    }

    /// Applies the selected treatment to one gross coupon amount.
    ///
    /// # Errors
    ///
    /// Returns validation failure on invalid rates, overflow, zero division, or inexact legacy
    /// multiplication.
    pub fn adjust_coupon(&self, gross: FixedDecimal) -> ApplicationResult<FixedDecimal> {
        if !gross.is_non_negative() {
            return Err(crate::map_domain_error(
                ficant_domain::DomainErrorCode::InvalidValue,
            ));
        }
        match (self.gross_coupon_basis, self.rounding) {
            (GrossCouponTaxBasis::LegacyRetainedRate, TaxRoundingMode::Exact) => {
                let retained = FixedDecimal::ONE
                    .checked_sub(self.value_added_tax_rate)
                    .map_err(crate::map_domain_error)?;
                gross.checked_mul(retained).map_err(crate::map_domain_error)
            }
            (GrossCouponTaxBasis::VatIncluded, TaxRoundingMode::TiesToEven) => {
                let denominator = FixedDecimal::ONE
                    .checked_add(self.value_added_tax_rate)
                    .map_err(crate::map_domain_error)?;
                gross
                    .checked_div_round_ties_even(denominator)
                    .map_err(crate::map_domain_error)
            }
            _ => Err(crate::map_domain_error(
                ficant_domain::DomainErrorCode::InvalidValue,
            )),
        }
    }

    /// Stable bytes committed into the request parameter digest.
    #[must_use]
    pub fn fingerprint_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.push(match self.gross_coupon_basis {
            GrossCouponTaxBasis::LegacyRetainedRate => 1,
            GrossCouponTaxBasis::VatIncluded => 2,
        });
        bytes.push(match self.rounding {
            TaxRoundingMode::Exact => 1,
            TaxRoundingMode::TiesToEven => 2,
        });
        bytes.push(match self.claim_scope {
            CouponTaxClaimScope::LegacySyntheticRetainedRate => 1,
            CouponTaxClaimScope::CouponOutputVatBeforeInputCredit => 2,
        });
        bytes.extend_from_slice(&self.value_added_tax_rate.scaled().to_be_bytes());
        bytes.extend_from_slice(&self.income_tax_rate.scaled().to_be_bytes());
        bytes.extend_from_slice(self.unit.unit_id().as_str().as_bytes());
        bytes.extend_from_slice(&self.unit.version().get().to_be_bytes());
        if let Some(hash) = &self.authority_semantic_hash {
            bytes.extend_from_slice(hash.as_bytes());
        }
        bytes
    }
}

/// Compatibility name retained only for explicit v1 AC10 tests.
pub type CouponTaxRate = CouponTaxTreatment;

/// L3 adapter boundary for one typed coupon-tax rule payload schema.
///
/// The application validates the exact definition binding and this adapter's declared envelope
/// before asking the adapter to select the Bond interval, attributes, and Subject profile pair.
pub trait TaxRulePackParser: Send + Sync {
    #[must_use]
    fn market(&self) -> &'static str;

    #[must_use]
    fn rule_type(&self) -> &'static str;

    #[must_use]
    fn type_url(&self) -> &'static str;

    /// Optional exact source identity for production authority-bound packs.
    #[must_use]
    fn expected_source(&self) -> Option<&'static str> {
        None
    }

    /// Exact operational window for one authority-bound pack, expressed as RFC 3339 instants.
    #[must_use]
    fn expected_effective_window(&self) -> Option<(&'static str, &'static str)> {
        None
    }

    /// Exact immutable Unit definition required by an authority-bound parser.
    #[must_use]
    fn expected_rate_unit(
        &self,
    ) -> Option<(&'static str, u64, &'static str, &'static str, u32, u32)> {
        None
    }

    /// Parses one exact Bond and Subject tax treatment.
    ///
    /// # Errors
    ///
    /// Returns a non-retryable validation failure when a required interval, attribute, profile,
    /// or rate item is missing or invalid.
    fn parse(
        &self,
        content: &RulePackContent,
        first_issue_date: NaiveDate,
        tax_attributes: BondTaxAttributes,
        tax_treatment: &TaxTreatment,
    ) -> ApplicationResult<CouponTaxTreatment>;
}
