use ficant_domain::VersionedDefinition;
use ficant_domain::primitives::{ContentHash, DecimalValue, Ulid, UnitRef};

use super::definitions::{DefinitionRepository, DefinitionValue};
use super::facts::MarketFact;
use super::fingerprint::{FingerprintBuilder, fact_bytes};
use super::{AccessScope, ApplicationResult, OperationFingerprint};
use crate::map_domain_error;
use ficant_domain::DomainErrorCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketFactKind {
    Cashflow,
    Quote,
    Trade,
    Valuation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketFactFieldRole {
    Currency,
    Price,
    Notional,
}

impl MarketFactFieldRole {
    fn expected_dimension(self) -> &'static str {
        match self {
            Self::Currency => "currency",
            Self::Price => "price",
            Self::Notional => "notional",
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Currency => 1,
            Self::Price => 2,
            Self::Notional => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedUnitBinding {
    role: MarketFactFieldRole,
    ordinal: u32,
    unit: UnitRef,
    dimension: String,
    scale: u32,
    precision: u32,
}

impl ResolvedUnitBinding {
    #[must_use]
    pub fn role(&self) -> MarketFactFieldRole {
        self.role
    }

    #[must_use]
    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }

    #[must_use]
    pub fn unit(&self) -> &UnitRef {
        &self.unit
    }

    #[must_use]
    pub fn dimension(&self) -> &str {
        &self.dimension
    }

    #[must_use]
    pub fn scale(&self) -> u32 {
        self.scale
    }

    #[must_use]
    pub fn precision(&self) -> u32 {
        self.precision
    }
}

/// Opaque evidence that every decimal field was resolved against an exact Unit definition.
///
/// ```compile_fail
/// use ficant_application::ports::ResolvedMarketFactProof;
/// let _ = ResolvedMarketFactProof { bindings: Vec::new() };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedMarketFactProof {
    fact_digest: ContentHash,
    scope_fingerprint: OperationFingerprint,
    tenant_id: Ulid,
    kind: MarketFactKind,
    fact_id: Ulid,
    bindings: Vec<ResolvedUnitBinding>,
    binding_hash: OperationFingerprint,
}

impl ResolvedMarketFactProof {
    #[must_use]
    pub fn fact_digest(&self) -> &ContentHash {
        &self.fact_digest
    }

    #[must_use]
    pub fn scope_fingerprint(&self) -> &OperationFingerprint {
        &self.scope_fingerprint
    }

    #[must_use]
    pub fn tenant_id(&self) -> &Ulid {
        &self.tenant_id
    }

    #[must_use]
    pub fn kind(&self) -> MarketFactKind {
        self.kind
    }

    #[must_use]
    pub fn fact_id(&self) -> &Ulid {
        &self.fact_id
    }

    #[must_use]
    pub fn bindings(&self) -> &[ResolvedUnitBinding] {
        &self.bindings
    }

    #[must_use]
    pub fn binding_hash(&self) -> &OperationFingerprint {
        &self.binding_hash
    }

    fn validate_for(&self, fact: &MarketFact) -> ApplicationResult<()> {
        let fields = fact_fields(fact)?;
        if self.fact_digest != ContentHash::digest(&fact_bytes(fact))
            || self.tenant_id != *fact.owner().tenant_id()
            || self.kind != fact_kind(fact)
            || self.fact_id != *fact.id()
            || fields.len() != self.bindings.len()
        {
            return Err(invalid_unit());
        }
        for (field, binding) in fields.iter().zip(&self.bindings) {
            validate_binding(field, binding)?;
        }
        if self.binding_hash
            != proof_binding_hash(
                fact,
                &self.scope_fingerprint,
                &self.tenant_id,
                self.kind,
                &self.bindings,
            )
        {
            return Err(invalid_unit());
        }
        Ok(())
    }
}

/// A market fact that cannot be created without a complete resolved-unit proof.
///
/// ```compile_fail
/// use ficant_application::ports::ValidatedMarketFact;
/// let _ = ValidatedMarketFact { fact: panic!(), proof: panic!() };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedMarketFact {
    fact: MarketFact,
    proof: ResolvedMarketFactProof,
}

impl ValidatedMarketFact {
    #[must_use]
    pub fn fact(&self) -> &MarketFact {
        &self.fact
    }

    #[must_use]
    pub fn proof(&self) -> &ResolvedMarketFactProof {
        &self.proof
    }

    pub(crate) fn validate(&self) -> ApplicationResult<()> {
        self.proof.validate_for(&self.fact)
    }

    pub(crate) fn authorize_scope(&self, scope: &AccessScope) -> ApplicationResult<()> {
        self.validate()?;
        scope.authorize(self.fact.owner())?;
        if self.proof.tenant_id != *scope.tenant_id()
            || &self.proof.scope_fingerprint != scope.fingerprint()
        {
            return Err(invalid_unit());
        }
        Ok(())
    }

    pub(crate) fn into_parts(self) -> (MarketFact, ResolvedMarketFactProof) {
        (self.fact, self.proof)
    }
}

pub struct MarketFactUnitResolver<'a> {
    definitions: &'a dyn DefinitionRepository,
}

impl<'a> MarketFactUnitResolver<'a> {
    #[must_use]
    pub fn new(definitions: &'a dyn DefinitionRepository) -> Self {
        Self { definitions }
    }

    /// Resolves every decimal field against an exact tenant-visible Unit definition.
    ///
    /// # Errors
    ///
    /// Returns non-retryable validation failure for missing or semantically invalid Units.
    pub async fn resolve(
        &self,
        scope: &AccessScope,
        fact: MarketFact,
    ) -> ApplicationResult<ValidatedMarketFact> {
        scope.authorize(fact.owner())?;
        let fields = fact_fields(&fact)?;
        let mut bindings = Vec::with_capacity(fields.len());
        for field in &fields {
            let reference = field.decimal.unit();
            let value = self
                .definitions
                .get_version(scope, reference.unit_id().clone(), reference.version())
                .await?
                .ok_or_else(invalid_unit)?;
            let DefinitionValue::Unit(unit) = value else {
                return Err(invalid_unit());
            };
            if unit.identity() != reference.unit_id().as_str()
                || unit.version() != reference.version().get()
                || unit.owner().tenant_id() != scope.tenant_id()
                || unit.owner().tenant_id() != fact.owner().tenant_id()
                || unit.dimension() != field.role.expected_dimension()
                || field.decimal.scale() > unit.scale()
                || effective_precision(&field.decimal) > unit.precision()
            {
                return Err(invalid_unit());
            }
            bindings.push(ResolvedUnitBinding {
                role: field.role,
                ordinal: field.ordinal,
                unit: reference.clone(),
                dimension: unit.dimension().to_owned(),
                scale: unit.scale(),
                precision: unit.precision(),
            });
        }
        let kind = fact_kind(&fact);
        let tenant_id = scope.tenant_id().clone();
        let scope_fingerprint = scope.fingerprint().clone();
        let proof = ResolvedMarketFactProof {
            fact_digest: ContentHash::digest(&fact_bytes(&fact)),
            binding_hash: proof_binding_hash(
                &fact,
                &scope_fingerprint,
                &tenant_id,
                kind,
                &bindings,
            ),
            scope_fingerprint,
            tenant_id,
            kind,
            fact_id: fact.id().clone(),
            bindings,
        };
        let validated = ValidatedMarketFact { fact, proof };
        validated.validate()?;
        Ok(validated)
    }
}

#[derive(Clone)]
struct FactField {
    role: MarketFactFieldRole,
    ordinal: u32,
    decimal: DecimalValue,
}

fn fact_fields(fact: &MarketFact) -> ApplicationResult<Vec<FactField>> {
    let fields = match fact {
        MarketFact::Cashflow(value) => {
            vec![field(MarketFactFieldRole::Currency, 0, value.amount())]
        }
        MarketFact::Quote(value) => {
            let mut fields = Vec::with_capacity(2);
            if let Some(bid) = value.bid() {
                fields.push(field(MarketFactFieldRole::Price, 0, bid));
            }
            if let Some(ask) = value.ask() {
                fields.push(field(MarketFactFieldRole::Price, 1, ask));
            }
            fields
        }
        MarketFact::Trade(value) => vec![
            field(MarketFactFieldRole::Price, 0, value.price()),
            field(MarketFactFieldRole::Notional, 0, value.quantity()),
        ],
        MarketFact::Valuation(value) => value
            .values()
            .iter()
            .enumerate()
            .map(|(ordinal, value)| {
                let ordinal = u32::try_from(ordinal).map_err(|_| invalid_unit())?;
                Ok(field(MarketFactFieldRole::Price, ordinal, value))
            })
            .collect::<ApplicationResult<Vec<_>>>()?,
    };
    if fields.is_empty() {
        return Err(invalid_unit());
    }
    Ok(fields)
}

fn field(role: MarketFactFieldRole, ordinal: u32, decimal: &DecimalValue) -> FactField {
    FactField {
        role,
        ordinal,
        decimal: decimal.clone(),
    }
}

fn validate_binding(field: &FactField, binding: &ResolvedUnitBinding) -> ApplicationResult<()> {
    if binding.role != field.role
        || binding.ordinal != field.ordinal
        || binding.unit != *field.decimal.unit()
        || binding.dimension != field.role.expected_dimension()
        || field.decimal.scale() > binding.scale
        || effective_precision(&field.decimal) > binding.precision
    {
        return Err(invalid_unit());
    }
    Ok(())
}

fn effective_precision(decimal: &DecimalValue) -> u32 {
    u32::try_from(decimal.coefficient().trim_start_matches('-').len()).unwrap_or(u32::MAX)
}

fn fact_kind(fact: &MarketFact) -> MarketFactKind {
    match fact {
        MarketFact::Cashflow(_) => MarketFactKind::Cashflow,
        MarketFact::Quote(_) => MarketFactKind::Quote,
        MarketFact::Trade(_) => MarketFactKind::Trade,
        MarketFact::Valuation(_) => MarketFactKind::Valuation,
    }
}

fn proof_binding_hash(
    fact: &MarketFact,
    scope_fingerprint: &OperationFingerprint,
    tenant_id: &Ulid,
    kind: MarketFactKind,
    bindings: &[ResolvedUnitBinding],
) -> OperationFingerprint {
    let mut canonical = FingerprintBuilder::new("resolved-market-fact-proof/v1");
    canonical.field(2, &fact_bytes(fact));
    canonical.field(3, scope_fingerprint.content_hash().as_bytes());
    canonical.field(4, tenant_id.as_str().as_bytes());
    canonical.field(5, &[fact_kind_code(kind)]);
    canonical.field(6, fact.id().as_str().as_bytes());
    for binding in bindings {
        canonical.field(10, &[binding.role.code()]);
        canonical.u64(11, u64::from(binding.ordinal));
        canonical.field(12, binding.unit.unit_id().as_str().as_bytes());
        canonical.u64(13, binding.unit.version().get());
        canonical.field(14, binding.dimension.as_bytes());
        canonical.u64(15, u64::from(binding.scale));
        canonical.u64(16, u64::from(binding.precision));
    }
    canonical.finish()
}

fn fact_kind_code(kind: MarketFactKind) -> u8 {
    match kind {
        MarketFactKind::Cashflow => 1,
        MarketFactKind::Quote => 2,
        MarketFactKind::Trade => 3,
        MarketFactKind::Valuation => 4,
    }
}

fn invalid_unit() -> crate::ApplicationError {
    map_domain_error(DomainErrorCode::InvalidUnit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ficant_domain::market::{FactSource, Quote, QuoteInput};
    use ficant_domain::primitives::{MarketTime, OwnerRef, Version, VersionRef};

    #[test]
    fn missing_duplicate_and_fact_swapped_internal_proofs_fail_closed() {
        let original = MarketFact::Quote(quote('Q'));
        let binding = ResolvedUnitBinding {
            role: MarketFactFieldRole::Price,
            ordinal: 0,
            unit: UnitRef::new(id('P'), version(1)),
            dimension: "price".to_owned(),
            scale: 2,
            precision: 18,
        };

        let missing = validated_with_bindings(original.clone(), Vec::new());
        assert_invalid(&missing.validate().unwrap_err());

        let duplicate =
            validated_with_bindings(original.clone(), vec![binding.clone(), binding.clone()]);
        assert_invalid(&duplicate.validate().unwrap_err());

        let original_proof = validated_with_bindings(original, vec![binding]).proof;
        let swapped = ValidatedMarketFact {
            fact: MarketFact::Quote(quote('S')),
            proof: original_proof,
        };
        assert_invalid(&swapped.validate().unwrap_err());
    }

    fn validated_with_bindings(
        fact: MarketFact,
        bindings: Vec<ResolvedUnitBinding>,
    ) -> ValidatedMarketFact {
        let scope = scope();
        let kind = fact_kind(&fact);
        let tenant_id = scope.tenant_id().clone();
        let scope_fingerprint = scope.fingerprint().clone();
        let proof = ResolvedMarketFactProof {
            fact_digest: ContentHash::digest(&fact_bytes(&fact)),
            binding_hash: proof_binding_hash(
                &fact,
                &scope_fingerprint,
                &tenant_id,
                kind,
                &bindings,
            ),
            scope_fingerprint,
            tenant_id,
            kind,
            fact_id: fact.id().clone(),
            bindings,
        };
        ValidatedMarketFact { fact, proof }
    }

    fn quote(suffix: char) -> Quote {
        Quote::new(QuoteInput {
            quote_id: id(suffix),
            instrument: VersionRef::new(id('K'), version(1)),
            owner: owner(),
            source: FactSource::new("internal", "quote", 1).unwrap(),
            observed_at: time(1),
            received_at: time(2),
            bid: Some(DecimalValue::new("10125", 2, UnitRef::new(id('P'), version(1))).unwrap()),
            ask: None,
            supersedes_id: None,
        })
        .unwrap()
    }

    fn assert_invalid(error: &crate::ApplicationError) {
        assert_eq!(
            error.category(),
            crate::ApplicationErrorCategory::ValidationFailed
        );
        assert!(!error.retryable());
    }

    fn scope() -> AccessScope {
        AccessScope::new(id('T'), id('A'), vec![id('Y')]).unwrap()
    }

    fn owner() -> OwnerRef {
        OwnerRef::new(id('T'), id('Y'))
    }

    fn version(value: u64) -> Version {
        Version::new(value).unwrap()
    }

    fn id(suffix: char) -> Ulid {
        Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
    }

    fn time(hour: u32) -> MarketTime {
        MarketTime::new(
            format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap(),
            "Asia/Shanghai",
            "2026-03-04".parse().unwrap(),
        )
        .unwrap()
    }
}
