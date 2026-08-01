use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use chrono::{Datelike, NaiveDate};
use ficant_domain::analytics::{
    AnalyticsObjectRef, BondTerms, BusinessDayConvention, CouponFrequency, DECIMAL_SCALE,
    DayCountConvention, FixedDecimal,
};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliverableInput, FuturesDeliveryBasketResult, FuturesDeliveryResult,
    FuturesDeliveryRule, is_deliverable_by_dates,
};
use ficant_domain::market::{
    Bond, BondBusinessDayConvention, BondCouponFrequency, BondDayCountConvention, FuturesContract,
    InstrumentKind, MarketRulePack,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{Artifact, ArtifactKind};
use ficant_domain::{ContentAddressed, DomainErrorCode, Lineaged, VersionedDefinition};

use crate::ports::{
    AccessScope, ApplicationResult, ArtifactRepository, BeginBlobStage, BlobStore, CanonicalQuote,
    CanonicalSnapshotDecoder, DefinitionRepository, DefinitionValue, FuturesDeliveryArtifactCodec,
    FuturesDeliveryEngine, FuturesDeliveryRuleParser, IdempotencyKey, InstrumentSubtype,
    IntegrityEventSink, PublishArtifact, RequiredVerifiedBlobRead, SafeTraceContext,
    SnapshotVerifiedReadMetadataRepository, VerifiedBlobReader, VerifiedBlobRole,
    VerifiedReadResourceKind, VerifyBlobStage, definition_content_hash,
};
use crate::use_cases::bond_analytics::map_analytics_error;
use crate::use_cases::verified_reads::{VerifiedSnapshotRead, VerifiedSnapshotReader};
use crate::{ApplicationError, ApplicationErrorCategory, map_domain_error};

pub const FUTURES_DELIVERY_MEDIA_TYPE: &str =
    "application/vnd.apache.arrow.file; profile=ficant.cgb-futures-delivery.v1";

/// Resolves the exact persisted `RulePack` binding into the provider-neutral delivery-rule shape.
///
/// This is deliberately separate from the numerical engine: all identity, authorization,
/// effective-time, content-hash, and typed-envelope checks complete before any engine call.
pub struct ResolveFuturesDeliveryRule<'a> {
    definitions: &'a dyn DefinitionRepository,
    parser: &'a dyn FuturesDeliveryRuleParser,
}

impl<'a> ResolveFuturesDeliveryRule<'a> {
    #[must_use]
    pub const fn new(
        definitions: &'a dyn DefinitionRepository,
        parser: &'a dyn FuturesDeliveryRuleParser,
    ) -> Self {
        Self {
            definitions,
            parser,
        }
    }

    /// Reads and parses the exact `RulePack` before a futures-delivery calculation.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed error for missing definitions/content, mismatched bindings, expired
    /// packs, hash drift, wrong typed envelopes, or missing required rule items.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        binding: &ficant_domain::analytics::AnalyticsObjectRef,
        valuation_at: MarketTime,
        product: CgbFuturesProduct,
    ) -> ApplicationResult<FuturesDeliveryRule> {
        let resolved = self
            .definitions
            .get_version(
                scope,
                binding.version_ref().id().clone(),
                binding.version_ref().version(),
            )
            .await?
            .ok_or_else(lineage_incomplete)?;
        let DefinitionValue::MarketRulePack(rule_pack) = resolved else {
            return Err(lineage_incomplete());
        };
        validate_delivery_rule_pack(scope, binding, &valuation_at, &rule_pack, self.parser)?;
        let content = rule_pack
            .content()
            .ok_or_else(|| ApplicationError::rule_pack_item_missing("context.rule_pack.content"))?;
        self.parser.parse(content, product)
    }
}

fn validate_delivery_rule_pack(
    scope: &AccessScope,
    binding: &ficant_domain::analytics::AnalyticsObjectRef,
    valuation_at: &MarketTime,
    rule_pack: &MarketRulePack,
    parser: &dyn FuturesDeliveryRuleParser,
) -> ApplicationResult<()> {
    if rule_pack.identity() != binding.version_ref().id().as_str()
        || rule_pack.version() != binding.version_ref().version().get()
    {
        return Err(lineage_incomplete());
    }
    scope.authorize(rule_pack.owner())?;
    if rule_pack.content_hash() != binding.content_hash() {
        return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
    }
    if rule_pack.effective().from().instant() > valuation_at.instant()
        || valuation_at.instant() >= rule_pack.effective().to().instant()
    {
        return Err(map_domain_error(DomainErrorCode::InvalidEffectiveTime));
    }
    let content = rule_pack
        .content()
        .ok_or_else(|| ApplicationError::rule_pack_item_missing("context.rule_pack.content"))?;
    rule_pack
        .content_hash()
        .verify(content.value())
        .map_err(map_domain_error)?;
    if rule_pack.market() != parser.market()
        || rule_pack.rule_type() != parser.rule_type()
        || content.type_url() != parser.type_url()
    {
        return Err(map_domain_error(DomainErrorCode::InvalidValue));
    }
    Ok(())
}

fn lineage_incomplete() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::LineageIncomplete, false)
}

/// Resolves one exact registered concrete futures contract.
pub struct ResolveFuturesContract<'a> {
    definitions: &'a dyn DefinitionRepository,
}

impl<'a> ResolveFuturesContract<'a> {
    #[must_use]
    pub const fn new(definitions: &'a dyn DefinitionRepository) -> Self {
        Self { definitions }
    }

    /// Resolves only an exact `Instrument(Futures, FuturesContract)` definition version.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed error for absent, inaccessible, wrong-version, non-futures,
    /// subtype-less, or differently rule-bound definitions.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        binding: &AnalyticsObjectRef,
        expected_owner: &OwnerRef,
        expected_rule_pack_ref: &VersionRef,
    ) -> ApplicationResult<FuturesContract> {
        let resolved = self
            .definitions
            .get_version(
                scope,
                binding.version_ref().id().clone(),
                binding.version_ref().version(),
            )
            .await?
            .ok_or_else(lineage_incomplete)?;
        let DefinitionValue::Instrument(definition) = resolved else {
            return Err(lineage_incomplete());
        };
        scope.authorize(definition.owner())?;
        if definition.owner() != expected_owner
            || definition.identity() != binding.version_ref().id().as_str()
            || definition.version() != binding.version_ref().version().get()
            || definition.instrument().kind() != InstrumentKind::Futures
        {
            return Err(lineage_incomplete());
        }
        let Some(InstrumentSubtype::FuturesContract(contract)) = definition.subtype() else {
            return Err(lineage_incomplete());
        };
        if contract.instrument() != binding.version_ref()
            || contract.rule_pack() != expected_rule_pack_ref
        {
            return Err(lineage_incomplete());
        }
        Ok(contract.clone())
    }
}

/// Request-side candidate values that must agree with registered Bond facts and snapshot quotes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesDeliveryCandidateBinding {
    bond: AnalyticsObjectRef,
    terms: BondTerms,
    spot_clean_price: FixedDecimal,
}

impl FuturesDeliveryCandidateBinding {
    #[must_use]
    pub const fn new(
        bond: AnalyticsObjectRef,
        terms: BondTerms,
        spot_clean_price: FixedDecimal,
    ) -> Self {
        Self {
            bond,
            terms,
            spot_clean_price,
        }
    }

    #[must_use]
    pub fn bond(&self) -> &AnalyticsObjectRef {
        &self.bond
    }

    #[must_use]
    pub fn terms(&self) -> &BondTerms {
        &self.terms
    }

    #[must_use]
    pub const fn spot_clean_price(&self) -> FixedDecimal {
        self.spot_clean_price
    }
}

/// Frozen request-side values consumed while materializing exact futures-delivery inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesDeliveryInputBindings {
    owner: OwnerRef,
    futures_contract: AnalyticsObjectRef,
    rule_pack: AnalyticsObjectRef,
    snapshot: AnalyticsObjectRef,
    valuation_at: MarketTime,
    purchase_date: NaiveDate,
    delivery_month_first: NaiveDate,
    delivery_date: NaiveDate,
    product: CgbFuturesProduct,
    candidates: Vec<FuturesDeliveryCandidateBinding>,
    futures_clean_price: FixedDecimal,
    financing_rate: FixedDecimal,
    price_unit: UnitRef,
}

impl FuturesDeliveryInputBindings {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        owner: OwnerRef,
        futures_contract: AnalyticsObjectRef,
        rule_pack: AnalyticsObjectRef,
        snapshot: AnalyticsObjectRef,
        valuation_at: MarketTime,
        purchase_date: NaiveDate,
        delivery_month_first: NaiveDate,
        delivery_date: NaiveDate,
        product: CgbFuturesProduct,
        candidates: Vec<FuturesDeliveryCandidateBinding>,
        futures_clean_price: FixedDecimal,
        financing_rate: FixedDecimal,
        price_unit: UnitRef,
    ) -> Self {
        Self {
            owner,
            futures_contract,
            rule_pack,
            snapshot,
            valuation_at,
            purchase_date,
            delivery_month_first,
            delivery_date,
            product,
            candidates,
            futures_clean_price,
            financing_rate,
            price_unit,
        }
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
    pub fn candidates(&self) -> &[FuturesDeliveryCandidateBinding] {
        &self.candidates
    }

    #[must_use]
    pub fn price_unit(&self) -> &UnitRef {
        &self.price_unit
    }
}

/// Converts verified snapshot facts and exact Definitions into native delivery inputs.
pub struct MaterializeFuturesDeliveryInputs<'a> {
    definitions: &'a dyn DefinitionRepository,
    snapshots: VerifiedSnapshotReader<'a>,
    decoder: &'a dyn CanonicalSnapshotDecoder,
    parser: &'a dyn FuturesDeliveryRuleParser,
}

impl<'a> MaterializeFuturesDeliveryInputs<'a> {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        definitions: &'a dyn DefinitionRepository,
        snapshot_metadata: &'a dyn SnapshotVerifiedReadMetadataRepository,
        blob_reader: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
        decoder: &'a dyn CanonicalSnapshotDecoder,
        parser: &'a dyn FuturesDeliveryRuleParser,
    ) -> Self {
        Self {
            definitions,
            snapshots: VerifiedSnapshotReader::new(
                snapshot_metadata,
                blob_reader,
                integrity_events,
            ),
            decoder,
            parser,
        }
    }

    /// Verifies all persisted facts before constructing any native-engine input.
    ///
    /// # Errors
    ///
    /// Returns a non-retryable, fail-closed application error for snapshot, quote, Definition,
    /// static-term, candidate-set, or price drift.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        bindings: &FuturesDeliveryInputBindings,
        trace: SafeTraceContext,
    ) -> ApplicationResult<Vec<FuturesDeliverableInput>> {
        scope.authorize(&bindings.owner)?;
        ResolveFuturesContract::new(self.definitions)
            .execute(
                scope,
                &bindings.futures_contract,
                &bindings.owner,
                bindings.rule_pack.version_ref(),
            )
            .await?;

        let verified = self
            .snapshots
            .read(scope, bindings.snapshot.version_ref().id().clone(), trace)
            .await?;
        let VerifiedSnapshotRead::Data {
            snapshot,
            parquet,
            manifest,
        } = verified
        else {
            return Err(lineage_incomplete());
        };
        if snapshot.id() != bindings.snapshot.version_ref().id()
            || snapshot.content_hash() != bindings.snapshot.content_hash()
            || snapshot.owner() != &bindings.owner
            || snapshot.as_of() != &bindings.valuation_at
            || snapshot.visible_at().instant() < snapshot.as_of().instant()
        {
            return Err(lineage_incomplete());
        }

        let decoded = self
            .decoder
            .decode_quotes(&snapshot, parquet.bytes(), manifest.bytes())
            .await?;
        let quotes = exact_request_quotes(&snapshot, bindings, decoded)?;
        let request_bonds = self
            .validate_requested_bonds(scope, bindings, &quotes)
            .await?;

        let rule = ResolveFuturesDeliveryRule::new(self.definitions, self.parser)
            .execute(
                scope,
                &bindings.rule_pack,
                bindings.valuation_at.clone(),
                bindings.product,
            )
            .await?;
        let eligible = self
            .eligible_snapshot_bonds(scope, bindings, &quotes, &rule)
            .await?;
        let requested = request_bonds.keys().cloned().collect::<BTreeSet<_>>();
        if requested != eligible {
            return Err(invalid());
        }

        bindings
            .candidates
            .iter()
            .map(|candidate| {
                FuturesDeliverableInput::new(
                    bindings.owner.clone(),
                    bindings.futures_contract.clone(),
                    candidate.bond.clone(),
                    bindings.rule_pack.clone(),
                    bindings.snapshot.clone(),
                    bindings.valuation_at.clone(),
                    bindings.purchase_date,
                    bindings.delivery_month_first,
                    bindings.delivery_date,
                    bindings.product,
                    rule.clone(),
                    candidate.terms.clone(),
                    candidate.spot_clean_price,
                    bindings.futures_clean_price,
                    bindings.financing_rate,
                )
                .map_err(map_domain_error)
            })
            .collect()
    }

    async fn validate_requested_bonds(
        &self,
        scope: &AccessScope,
        bindings: &FuturesDeliveryInputBindings,
        quotes: &BTreeMap<VersionRef, CanonicalQuote>,
    ) -> ApplicationResult<BTreeMap<VersionRef, Bond>> {
        if bindings.candidates.is_empty() {
            return Err(invalid());
        }
        let mut bonds = BTreeMap::new();
        for candidate in &bindings.candidates {
            let reference = candidate.bond.version_ref();
            if bonds.contains_key(reference) {
                return Err(invalid());
            }
            let bond = resolve_bond(self.definitions, scope, reference, &bindings.owner).await?;
            validate_registered_bond_terms(&bond, &candidate.terms)?;
            let quote = quotes.get(reference).ok_or_else(invalid)?;
            if !quote_matches(quote, candidate.spot_clean_price) {
                return Err(invalid());
            }
            bonds.insert(reference.clone(), bond);
        }
        let futures_quote = quotes
            .get(bindings.futures_contract.version_ref())
            .ok_or_else(invalid)?;
        if !quote_matches(futures_quote, bindings.futures_clean_price) {
            return Err(invalid());
        }
        Ok(bonds)
    }

    async fn eligible_snapshot_bonds(
        &self,
        scope: &AccessScope,
        bindings: &FuturesDeliveryInputBindings,
        quotes: &BTreeMap<VersionRef, CanonicalQuote>,
        rule: &FuturesDeliveryRule,
    ) -> ApplicationResult<BTreeSet<VersionRef>> {
        let mut eligible = BTreeSet::new();
        for reference in quotes.keys() {
            if reference == bindings.futures_contract.version_ref() {
                continue;
            }
            let Some(value) = self
                .definitions
                .get_version(scope, reference.id().clone(), reference.version())
                .await?
            else {
                continue;
            };
            let DefinitionValue::Instrument(definition) = value else {
                continue;
            };
            if definition.owner() != &bindings.owner {
                continue;
            }
            let Some(InstrumentSubtype::Bond(bond)) = definition.subtype() else {
                continue;
            };
            if is_deliverable_by_dates(
                rule,
                bond.first_issue_date(),
                bond.maturity_date(),
                bindings.delivery_month_first,
            )
            .map_err(map_domain_error)?
            {
                eligible.insert(reference.clone());
            }
        }
        Ok(eligible)
    }
}

/// Exact server-derived delivery inputs and the hashes consumed by portfolio risk.
#[derive(Clone, Debug)]
pub struct RegisteredFuturesDeliveryMaterialization {
    inputs: Vec<FuturesDeliverableInput>,
    rule: FuturesDeliveryRule,
    contract: FuturesContract,
    input_evidence_hashes: Vec<ContentHash>,
    lineage: Vec<LineageRef>,
}

impl RegisteredFuturesDeliveryMaterialization {
    #[must_use]
    pub fn inputs(&self) -> &[FuturesDeliverableInput] {
        &self.inputs
    }

    #[must_use]
    pub fn rule(&self) -> &FuturesDeliveryRule {
        &self.rule
    }

    #[must_use]
    pub fn contract(&self) -> &FuturesContract {
        &self.contract
    }

    #[must_use]
    pub fn input_evidence_hashes(&self) -> &[ContentHash] {
        &self.input_evidence_hashes
    }

    #[must_use]
    pub fn lineage(&self) -> &[LineageRef] {
        &self.lineage
    }
}

/// Builds one complete concrete-futures delivery basket without caller-supplied market values.
pub struct MaterializeRegisteredFuturesDelivery<'a> {
    definitions: &'a dyn DefinitionRepository,
    snapshots: VerifiedSnapshotReader<'a>,
    decoder: &'a dyn CanonicalSnapshotDecoder,
    parser: &'a dyn FuturesDeliveryRuleParser,
}

impl<'a> MaterializeRegisteredFuturesDelivery<'a> {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        definitions: &'a dyn DefinitionRepository,
        snapshot_metadata: &'a dyn SnapshotVerifiedReadMetadataRepository,
        blob_reader: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
        decoder: &'a dyn CanonicalSnapshotDecoder,
        parser: &'a dyn FuturesDeliveryRuleParser,
    ) -> Self {
        Self {
            definitions,
            snapshots: VerifiedSnapshotReader::new(
                snapshot_metadata,
                blob_reader,
                integrity_events,
            ),
            decoder,
            parser,
        }
    }

    /// Materializes a risk-ready exact contract, complete eligible basket, and exact midpoints.
    ///
    /// # Errors
    ///
    /// Fails closed when any definition, rule, snapshot, quote, owner, time, unit, or hash
    /// binding is absent or inconsistent.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn execute(
        &self,
        scope: &AccessScope,
        owner: &OwnerRef,
        futures_contract_ref: &VersionRef,
        data_snapshot_id: Ulid,
        valuation_at: &MarketTime,
        knowledge_at: &MarketTime,
        trace: SafeTraceContext,
    ) -> ApplicationResult<RegisteredFuturesDeliveryMaterialization> {
        scope.authorize(owner)?;
        let definition = self
            .definitions
            .get_version(
                scope,
                futures_contract_ref.id().clone(),
                futures_contract_ref.version(),
            )
            .await?
            .ok_or_else(lineage_incomplete)?;
        let DefinitionValue::Instrument(instrument) = definition else {
            return Err(lineage_incomplete());
        };
        if instrument.owner() != owner
            || instrument.instrument().version_ref() != *futures_contract_ref
            || instrument.instrument().kind() != InstrumentKind::Futures
        {
            return Err(lineage_incomplete());
        }
        let Some(InstrumentSubtype::FuturesContract(contract)) = instrument.subtype() else {
            return Err(lineage_incomplete());
        };
        let product_code = contract.product_code().ok_or_else(lineage_incomplete)?;
        let price_unit = contract.price_unit().ok_or_else(lineage_incomplete)?;
        if contract.instrument() != futures_contract_ref
            || valuation_at.instant() > contract.last_trade_time().instant()
            || contract.last_trade_time().market_timezone() != valuation_at.market_timezone()
            || contract.expiry_time().market_timezone() != valuation_at.market_timezone()
            || contract.settlement_time().market_timezone() != valuation_at.market_timezone()
        {
            return Err(lineage_incomplete());
        }
        let product = self.parser.parse_product_code(product_code)?;

        let rule_value = self
            .definitions
            .get_version(
                scope,
                contract.rule_pack().id().clone(),
                contract.rule_pack().version(),
            )
            .await?
            .ok_or_else(lineage_incomplete)?;
        let DefinitionValue::MarketRulePack(rule_pack) = rule_value else {
            return Err(lineage_incomplete());
        };
        let rule_binding = AnalyticsObjectRef::new(
            contract.rule_pack().clone(),
            rule_pack.content_hash().clone(),
        );
        validate_delivery_rule_pack(scope, &rule_binding, valuation_at, &rule_pack, self.parser)?;
        if rule_pack.owner() != owner {
            return Err(lineage_incomplete());
        }
        let content = rule_pack
            .content()
            .ok_or_else(|| ApplicationError::rule_pack_item_missing("context.rule_pack.content"))?;
        let rule = self.parser.parse_for_portfolio_risk(content, product)?;

        let verified = self
            .snapshots
            .read(scope, data_snapshot_id.clone(), trace)
            .await?;
        let VerifiedSnapshotRead::Data {
            snapshot,
            parquet,
            manifest,
        } = verified
        else {
            return Err(lineage_incomplete());
        };
        if snapshot.id() != &data_snapshot_id
            || snapshot.owner() != owner
            || snapshot.as_of() != valuation_at
            || snapshot.visible_at().instant() < snapshot.as_of().instant()
            || snapshot.visible_at().instant() > knowledge_at.instant()
        {
            return Err(lineage_incomplete());
        }
        let decoded = self
            .decoder
            .decode_quotes(&snapshot, parquet.bytes(), manifest.bytes())
            .await?;
        let quotes = exact_midpoint_quotes(&snapshot, valuation_at, price_unit, decoded)?;
        let futures_quote = quotes
            .get(futures_contract_ref)
            .ok_or_else(lineage_incomplete)?;
        let futures_clean_price = exact_midpoint(futures_quote)?;
        let delivery_date = contract.settlement_time().local_trading_date();
        let delivery_month_first = delivery_date.with_day(1).ok_or_else(invalid)?;
        let purchase_date = valuation_at.local_trading_date();
        if purchase_date >= delivery_date {
            return Err(invalid());
        }

        let contract_hash =
            definition_content_hash(&DefinitionValue::Instrument(instrument.clone()));
        let contract_ref =
            AnalyticsObjectRef::new(futures_contract_ref.clone(), contract_hash.clone());
        let snapshot_ref = AnalyticsObjectRef::new(
            VersionRef::new(
                data_snapshot_id.clone(),
                Version::new(1).map_err(map_domain_error)?,
            ),
            snapshot.content_hash().clone(),
        );
        let mut inputs = Vec::new();
        let mut evidence = vec![
            contract_hash,
            definition_content_hash(&DefinitionValue::MarketRulePack(rule_pack.clone())),
            snapshot.content_hash().clone(),
            quote_evidence_hash(futures_quote),
        ];
        let mut lineage = vec![
            LineageRef::new(
                futures_contract_ref.id().clone(),
                Some(futures_contract_ref.version()),
                Some(definition_content_hash(&DefinitionValue::Instrument(
                    instrument.clone(),
                ))),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(
                contract.rule_pack().id().clone(),
                Some(contract.rule_pack().version()),
                Some(rule_pack.content_hash().clone()),
            )
            .map_err(map_domain_error)?,
            LineageRef::new(
                data_snapshot_id,
                None,
                Some(snapshot.content_hash().clone()),
            )
            .map_err(map_domain_error)?,
        ];
        for (reference, quote) in &quotes {
            if reference == futures_contract_ref {
                continue;
            }
            let Some(value) = self
                .definitions
                .get_version(scope, reference.id().clone(), reference.version())
                .await?
            else {
                continue;
            };
            let DefinitionValue::Instrument(candidate) = value else {
                continue;
            };
            if candidate.owner() != owner {
                continue;
            }
            let Some(InstrumentSubtype::Bond(bond)) = candidate.subtype() else {
                continue;
            };
            if !is_deliverable_by_dates(
                &rule,
                bond.first_issue_date(),
                bond.maturity_date(),
                delivery_month_first,
            )
            .map_err(map_domain_error)?
            {
                continue;
            }
            let terms = registered_bond_terms(bond)?;
            let candidate_hash = definition_content_hash(&DefinitionValue::Instrument(candidate));
            evidence.push(candidate_hash.clone());
            evidence.push(quote_evidence_hash(quote));
            lineage.push(
                LineageRef::new(
                    reference.id().clone(),
                    Some(reference.version()),
                    Some(candidate_hash.clone()),
                )
                .map_err(map_domain_error)?,
            );
            inputs.push(
                FuturesDeliverableInput::new(
                    owner.clone(),
                    contract_ref.clone(),
                    AnalyticsObjectRef::new(reference.clone(), candidate_hash),
                    rule_binding.clone(),
                    snapshot_ref.clone(),
                    valuation_at.clone(),
                    purchase_date,
                    delivery_month_first,
                    delivery_date,
                    product,
                    rule.clone(),
                    terms,
                    exact_midpoint(quote)?,
                    futures_clean_price,
                    FixedDecimal::ZERO,
                )
                .map_err(map_domain_error)?,
            );
        }
        if inputs.is_empty() {
            return Err(invalid());
        }
        inputs.sort_by(|left, right| left.bond().version_ref().cmp(right.bond().version_ref()));
        evidence.extend(inputs.iter().map(FuturesDeliverableInput::fingerprint));
        evidence.sort_unstable();
        evidence.dedup();
        lineage.sort_by(|left, right| {
            left.object_id()
                .cmp(right.object_id())
                .then_with(|| left.version().cmp(&right.version()))
        });
        Ok(RegisteredFuturesDeliveryMaterialization {
            inputs,
            rule,
            contract: contract.clone(),
            input_evidence_hashes: evidence,
            lineage,
        })
    }
}

fn registered_bond_terms(bond: &Bond) -> ApplicationResult<BondTerms> {
    let pricing = bond.pricing_terms().ok_or_else(lineage_incomplete)?;
    let tax = bond.tax_attributes().ok_or_else(lineage_incomplete)?;
    BondTerms::with_issuance(
        bond.first_issue_date(),
        bond.current_issue_date(),
        bond.maturity_date(),
        match pricing.frequency() {
            BondCouponFrequency::Annual => CouponFrequency::Annual,
            BondCouponFrequency::Semiannual => CouponFrequency::Semiannual,
        },
        match pricing.day_count() {
            BondDayCountConvention::ActActBondIsma => DayCountConvention::ActActBondIsma,
        },
        match pricing.business_day() {
            BondBusinessDayConvention::Following => BusinessDayConvention::Following,
        },
        decimal_to_fixed_exact(pricing.coupon_rate())?,
        decimal_to_fixed_exact(bond.face_value())?,
        decimal_to_fixed_exact(bond.cumulative_issued_amount())?,
        tax,
    )
    .map_err(map_domain_error)
}

fn decimal_to_fixed_exact(value: &DecimalValue) -> ApplicationResult<FixedDecimal> {
    if value.scale() > 12 {
        return Err(invalid());
    }
    let coefficient = value.coefficient().parse::<i128>().map_err(|_| invalid())?;
    let factor = 10_i128
        .checked_pow(12 - value.scale())
        .ok_or_else(invalid)?;
    Ok(FixedDecimal::from_scaled(
        coefficient.checked_mul(factor).ok_or_else(invalid)?,
    ))
}

fn exact_midpoint(quote: &CanonicalQuote) -> ApplicationResult<FixedDecimal> {
    let bid = quote.bid().ok_or_else(invalid)?;
    let ask = quote.ask().ok_or_else(invalid)?;
    let sum = bid.scaled().checked_add(ask.scaled()).ok_or_else(invalid)?;
    if sum % 2 != 0 {
        return Err(invalid());
    }
    Ok(FixedDecimal::from_scaled(sum / 2))
}

fn exact_midpoint_quotes(
    snapshot: &ficant_domain::research::DataSnapshot,
    valuation_at: &MarketTime,
    price_unit: &UnitRef,
    decoded: Vec<CanonicalQuote>,
) -> ApplicationResult<BTreeMap<VersionRef, CanonicalQuote>> {
    let mut quotes = BTreeMap::new();
    for quote in decoded {
        if quote.observed_at().instant() > quote.visible_at().instant()
            || quote.observed_at().market_timezone() != snapshot.as_of().market_timezone()
            || quote.visible_at().market_timezone() != snapshot.visible_at().market_timezone()
            || quote.observed_at().local_trading_date() != quote.local_trading_date()
        {
            return Err(invalid());
        }
        if quote.local_trading_date() != valuation_at.local_trading_date()
            || quote.observed_at().instant() > snapshot.as_of().instant()
            || quote.visible_at().instant() > snapshot.visible_at().instant()
            || quote.unit() != price_unit
        {
            continue;
        }
        let bid = quote.bid().ok_or_else(invalid)?;
        let ask = quote.ask().ok_or_else(invalid)?;
        if !bid.is_positive() || !ask.is_positive() || bid > ask {
            return Err(invalid());
        }
        exact_midpoint(&quote)?;
        match quotes.entry(quote.instrument().clone()) {
            Entry::Vacant(entry) => {
                entry.insert(quote);
            }
            Entry::Occupied(mut entry) => {
                let current = entry.get();
                let ordering = quote
                    .observed_at()
                    .instant()
                    .cmp(&current.observed_at().instant())
                    .then_with(|| {
                        quote
                            .visible_at()
                            .instant()
                            .cmp(&current.visible_at().instant())
                    });
                match ordering {
                    std::cmp::Ordering::Greater => {
                        entry.insert(quote);
                    }
                    std::cmp::Ordering::Equal => return Err(invalid()),
                    std::cmp::Ordering::Less => {}
                }
            }
        }
    }
    Ok(quotes)
}

fn quote_evidence_hash(quote: &CanonicalQuote) -> ContentHash {
    let mut bytes = Vec::new();
    append_quote_field(&mut bytes, quote.instrument().id().as_str().as_bytes());
    append_quote_field(
        &mut bytes,
        &quote.instrument().version().get().to_be_bytes(),
    );
    append_quote_field(
        &mut bytes,
        &quote
            .observed_at()
            .instant()
            .timestamp_micros()
            .to_be_bytes(),
    );
    append_quote_field(
        &mut bytes,
        &quote
            .visible_at()
            .instant()
            .timestamp_micros()
            .to_be_bytes(),
    );
    append_quote_field(
        &mut bytes,
        quote.local_trading_date().to_string().as_bytes(),
    );
    append_quote_field(
        &mut bytes,
        &quote
            .bid()
            .map_or(i128::MIN, FixedDecimal::scaled)
            .to_be_bytes(),
    );
    append_quote_field(
        &mut bytes,
        &quote
            .ask()
            .map_or(i128::MIN, FixedDecimal::scaled)
            .to_be_bytes(),
    );
    append_quote_field(&mut bytes, quote.unit().unit_id().as_str().as_bytes());
    append_quote_field(&mut bytes, &quote.unit().version().get().to_be_bytes());
    ContentHash::digest(&bytes)
}

fn append_quote_field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn exact_request_quotes(
    snapshot: &ficant_domain::research::DataSnapshot,
    bindings: &FuturesDeliveryInputBindings,
    decoded: Vec<CanonicalQuote>,
) -> ApplicationResult<BTreeMap<VersionRef, CanonicalQuote>> {
    let mut quotes = BTreeMap::new();
    for quote in decoded {
        if quote.observed_at().instant() > quote.visible_at().instant()
            || quote.observed_at().market_timezone() != snapshot.as_of().market_timezone()
            || quote.visible_at().market_timezone() != snapshot.visible_at().market_timezone()
            || quote.observed_at().local_trading_date() != quote.local_trading_date()
        {
            return Err(invalid());
        }
        if quote.local_trading_date() != bindings.valuation_at.local_trading_date()
            || quote.observed_at().instant() > snapshot.as_of().instant()
            || quote.visible_at().instant() > snapshot.visible_at().instant()
            || quote.unit() != &bindings.price_unit
        {
            continue;
        }
        if quote.bid().is_none() && quote.ask().is_none()
            || quote.bid().is_some_and(|value| !value.is_positive())
            || quote.ask().is_some_and(|value| !value.is_positive())
            || matches!((quote.bid(), quote.ask()), (Some(bid), Some(ask)) if bid > ask)
        {
            return Err(invalid());
        }
        match quotes.entry(quote.instrument().clone()) {
            Entry::Vacant(entry) => {
                entry.insert(quote);
            }
            Entry::Occupied(mut entry) => {
                let current = entry.get();
                let ordering = quote
                    .observed_at()
                    .instant()
                    .cmp(&current.observed_at().instant())
                    .then_with(|| {
                        quote
                            .visible_at()
                            .instant()
                            .cmp(&current.visible_at().instant())
                    });
                match ordering {
                    std::cmp::Ordering::Greater => {
                        entry.insert(quote);
                    }
                    std::cmp::Ordering::Equal => return Err(invalid()),
                    std::cmp::Ordering::Less => {}
                }
            }
        }
    }
    Ok(quotes)
}

async fn resolve_bond(
    definitions: &dyn DefinitionRepository,
    scope: &AccessScope,
    reference: &VersionRef,
    owner: &OwnerRef,
) -> ApplicationResult<Bond> {
    let resolved = definitions
        .get_version(scope, reference.id().clone(), reference.version())
        .await?
        .ok_or_else(lineage_incomplete)?;
    let DefinitionValue::Instrument(definition) = resolved else {
        return Err(lineage_incomplete());
    };
    scope.authorize(definition.owner())?;
    if definition.owner() != owner
        || definition.identity() != reference.id().as_str()
        || definition.version() != reference.version().get()
        || definition.instrument().kind() != InstrumentKind::Bond
    {
        return Err(lineage_incomplete());
    }
    let Some(InstrumentSubtype::Bond(bond)) = definition.subtype() else {
        return Err(lineage_incomplete());
    };
    if bond.instrument() != reference {
        return Err(lineage_incomplete());
    }
    Ok(bond.clone())
}

fn validate_registered_bond_terms(bond: &Bond, terms: &BondTerms) -> ApplicationResult<()> {
    if bond.first_issue_date() != terms.first_issue_date()
        || bond.current_issue_date() != terms.current_issue_date()
        || bond.maturity_date() != terms.maturity_date()
        || !decimal_matches_fixed(bond.face_value(), terms.face_amount())
    {
        return Err(invalid());
    }
    Ok(())
}

fn decimal_matches_fixed(value: &DecimalValue, expected: FixedDecimal) -> bool {
    let Ok(coefficient) = value.coefficient().parse::<i128>() else {
        return false;
    };
    match value.scale().cmp(&DECIMAL_SCALE) {
        std::cmp::Ordering::Equal => coefficient == expected.scaled(),
        std::cmp::Ordering::Less => 10_i128
            .checked_pow(DECIMAL_SCALE - value.scale())
            .and_then(|factor| coefficient.checked_mul(factor))
            .is_some_and(|scaled| scaled == expected.scaled()),
        std::cmp::Ordering::Greater => {
            let Some(factor) = 10_i128.checked_pow(value.scale() - DECIMAL_SCALE) else {
                return false;
            };
            coefficient % factor == 0 && coefficient / factor == expected.scaled()
        }
    }
}

fn quote_matches(quote: &CanonicalQuote, requested: FixedDecimal) -> bool {
    quote.bid() == Some(requested) || quote.ask() == Some(requested)
}

fn invalid() -> ApplicationError {
    map_domain_error(DomainErrorCode::InvalidValue)
}

pub struct CalculateFuturesDeliveryBasket<'a> {
    engine: &'a dyn FuturesDeliveryEngine,
}

impl<'a> CalculateFuturesDeliveryBasket<'a> {
    #[must_use]
    pub const fn new(engine: &'a dyn FuturesDeliveryEngine) -> Self {
        Self { engine }
    }

    /// Calculates a homogeneous delivery basket and selects CTD by maximum IRR.
    ///
    /// # Errors
    ///
    /// Returns validation failure for an empty, duplicate, or mixed-contract basket and maps
    /// stable engine failures without publishing partial results.
    pub fn execute(
        &self,
        inputs: &[FuturesDeliverableInput],
    ) -> ApplicationResult<FuturesDeliveryBasketResult> {
        let Some(first) = inputs.first() else {
            return Err(map_domain_error(DomainErrorCode::InvalidValue));
        };
        if inputs.iter().skip(1).any(|input| {
            input.owner() != first.owner()
                || input.futures_contract() != first.futures_contract()
                || input.rule_pack() != first.rule_pack()
                || input.snapshot() != first.snapshot()
                || input.valuation_at() != first.valuation_at()
                || input.purchase_date() != first.purchase_date()
                || input.delivery_month_first() != first.delivery_month_first()
                || input.delivery_date() != first.delivery_date()
                || input.product() != first.product()
                || input.rule() != first.rule()
                || input.futures_clean_price() != first.futures_clean_price()
                || input.financing_rate() != first.financing_rate()
        }) {
            return Err(map_domain_error(DomainErrorCode::BrokenLineage));
        }
        let candidates = inputs
            .iter()
            .map(|input| {
                let result = self.engine.calculate(input).map_err(map_analytics_error)?;
                result.validate_against(input).map_err(map_domain_error)?;
                Ok(result)
            })
            .collect::<ApplicationResult<Vec<_>>>()?;
        let ctd_index = select_ctd(&candidates);
        FuturesDeliveryBasketResult::new(candidates, ctd_index).map_err(map_domain_error)
    }
}

fn select_ctd(candidates: &[FuturesDeliveryResult]) -> usize {
    let mut best = 0;
    for index in 1..candidates.len() {
        let candidate = candidates[index].measures();
        let incumbent = candidates[best].measures();
        let candidate_id = candidates[index].input().bond().version_ref().id();
        let incumbent_id = candidates[best].input().bond().version_ref().id();
        if candidate.implied_repo_rate() > incumbent.implied_repo_rate()
            || (candidate.implied_repo_rate() == incumbent.implied_repo_rate()
                && (candidate.net_basis() < incumbent.net_basis()
                    || (candidate.net_basis() == incumbent.net_basis()
                        && candidate_id < incumbent_id)))
        {
            best = index;
        }
    }
    best
}

pub struct PublishFuturesDelivery<'a> {
    engine: &'a dyn FuturesDeliveryEngine,
    codec: &'a dyn FuturesDeliveryArtifactCodec,
    blobs: &'a dyn BlobStore,
    artifacts: &'a dyn ArtifactRepository,
}

impl<'a> PublishFuturesDelivery<'a> {
    #[must_use]
    pub const fn new(
        engine: &'a dyn FuturesDeliveryEngine,
        codec: &'a dyn FuturesDeliveryArtifactCodec,
        blobs: &'a dyn BlobStore,
        artifacts: &'a dyn ArtifactRepository,
    ) -> Self {
        Self {
            engine,
            codec,
            blobs,
            artifacts,
        }
    }

    /// Calculates, encodes, verifies and publishes one immutable basket Artifact.
    ///
    /// # Errors
    ///
    /// Returns without metadata publication when authorization, calculation, staging,
    /// verification or repository publication fails.
    pub async fn execute(
        &self,
        scope: AccessScope,
        artifact_id: Ulid,
        inputs: &[FuturesDeliverableInput],
        idempotency_key: IdempotencyKey,
    ) -> ApplicationResult<Artifact> {
        let first = first_input(inputs)?;
        scope.authorize(first.owner())?;
        let result = CalculateFuturesDeliveryBasket::new(self.engine).execute(inputs)?;
        let encoded = self.codec.encode(&result).map_err(map_analytics_error)?;
        let expected_hash = encoded.content_hash().clone();
        let expected_size = encoded.size();
        let stage = self
            .blobs
            .begin_stage(BeginBlobStage::new(
                scope.clone(),
                first.owner().clone(),
                expected_size,
                idempotency_key.clone(),
            )?)
            .await?;
        if let Err(error) = self
            .blobs
            .append_chunk(&scope, &stage, encoded.into_bytes())
            .await
        {
            let _ = self.blobs.discard_stage(&scope, &stage).await;
            return Err(error);
        }
        let verification = VerifyBlobStage::new(
            scope.clone(),
            stage.clone(),
            expected_hash.clone(),
            expected_size,
        )?;
        let verified = match self.blobs.verify_and_promote(verification).await {
            Ok(verified) => verified,
            Err(error) => {
                let _ = self.blobs.discard_stage(&scope, &stage).await;
                return Err(error);
            }
        };
        let artifact = Artifact::new(
            artifact_id,
            first.owner().clone(),
            ArtifactKind::Generic,
            FUTURES_DELIVERY_MEDIA_TYPE,
            expected_hash,
            expected_size,
            futures_delivery_lineage(inputs)?,
        )
        .map_err(map_domain_error)?;
        self.artifacts
            .publish_verified_blob(PublishArtifact::new(artifact, verified, idempotency_key)?)
            .await
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuturesDeliveryReplay {
    artifact: Artifact,
    stored: FuturesDeliveryBasketResult,
    recalculated: FuturesDeliveryBasketResult,
}

impl FuturesDeliveryReplay {
    #[must_use]
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }
    #[must_use]
    pub fn stored(&self) -> &FuturesDeliveryBasketResult {
        &self.stored
    }
    #[must_use]
    pub fn recalculated(&self) -> &FuturesDeliveryBasketResult {
        &self.recalculated
    }
}

pub struct ReplayFuturesDelivery<'a> {
    engine: &'a dyn FuturesDeliveryEngine,
    codec: &'a dyn FuturesDeliveryArtifactCodec,
    artifacts: &'a dyn ArtifactRepository,
    reader: &'a dyn VerifiedBlobReader,
    integrity_events: &'a dyn IntegrityEventSink,
}

impl<'a> ReplayFuturesDelivery<'a> {
    #[must_use]
    pub const fn new(
        engine: &'a dyn FuturesDeliveryEngine,
        codec: &'a dyn FuturesDeliveryArtifactCodec,
        artifacts: &'a dyn ArtifactRepository,
        reader: &'a dyn VerifiedBlobReader,
        integrity_events: &'a dyn IntegrityEventSink,
    ) -> Self {
        Self {
            engine,
            codec,
            artifacts,
            reader,
            integrity_events,
        }
    }

    /// Reads verified bytes, decodes the exact input binding and deterministically recalculates.
    ///
    /// # Errors
    ///
    /// Returns for authorization, lineage, integrity, decoding or replay drift.
    pub async fn execute(
        &self,
        scope: &AccessScope,
        artifact_id: Ulid,
        expected_inputs: &[FuturesDeliverableInput],
        trace: SafeTraceContext,
    ) -> ApplicationResult<FuturesDeliveryReplay> {
        let first = first_input(expected_inputs)?;
        scope.authorize(first.owner())?;
        let artifact = self
            .artifacts
            .get_metadata(scope, artifact_id.clone())
            .await?
            .ok_or_else(|| ApplicationError::new(ApplicationErrorCategory::NotFound, false))?;
        validate_artifact(scope, &artifact_id, &artifact, expected_inputs)?;
        let request = RequiredVerifiedBlobRead::new(
            scope.clone(),
            artifact.owner().clone(),
            VerifiedReadResourceKind::Artifact,
            artifact.id().clone(),
            VerifiedBlobRole::ArtifactPayload,
            artifact.content_hash().clone(),
            artifact.blob_size(),
            trace,
        )?;
        let payload = self
            .reader
            .read_required(&request, self.integrity_events)
            .await?;
        let stored = self
            .codec
            .decode(payload.bytes(), expected_inputs)
            .map_err(map_analytics_error)?;
        let recalculated =
            CalculateFuturesDeliveryBasket::new(self.engine).execute(expected_inputs)?;
        if stored != recalculated {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        let replay = self
            .codec
            .encode(&recalculated)
            .map_err(map_analytics_error)?;
        if replay.content_hash() != artifact.content_hash()
            || replay.size() != artifact.blob_size()
            || replay.bytes() != payload.bytes()
        {
            return Err(map_domain_error(DomainErrorCode::ContentHashMismatch));
        }
        Ok(FuturesDeliveryReplay {
            artifact,
            stored,
            recalculated,
        })
    }
}

fn first_input(inputs: &[FuturesDeliverableInput]) -> ApplicationResult<&FuturesDeliverableInput> {
    inputs
        .first()
        .ok_or_else(|| map_domain_error(DomainErrorCode::InvalidValue))
}

fn futures_delivery_lineage(
    inputs: &[FuturesDeliverableInput],
) -> ApplicationResult<Vec<LineageRef>> {
    let first = first_input(inputs)?;
    let mut lineage = vec![LineageRef::versioned(
        first.futures_contract().version_ref().id().clone(),
        first.futures_contract().version_ref().version(),
    )];
    lineage.extend(inputs.iter().map(|input| {
        LineageRef::versioned(
            input.bond().version_ref().id().clone(),
            input.bond().version_ref().version(),
        )
    }));
    lineage.push(
        LineageRef::new(
            first.rule_pack().version_ref().id().clone(),
            Some(first.rule_pack().version_ref().version()),
            Some(first.rule_pack().content_hash().clone()),
        )
        .map_err(map_domain_error)?,
    );
    lineage.push(LineageRef::content_addressed(
        first.snapshot().version_ref().id().clone(),
        first.snapshot().content_hash().clone(),
    ));
    Ok(lineage)
}

fn validate_artifact(
    scope: &AccessScope,
    artifact_id: &Ulid,
    artifact: &Artifact,
    inputs: &[FuturesDeliverableInput],
) -> ApplicationResult<()> {
    let first = first_input(inputs)?;
    scope.authorize(artifact.owner())?;
    if artifact.id() != artifact_id
        || artifact.owner() != first.owner()
        || artifact.kind() != ArtifactKind::Generic
        || artifact.media_type() != FUTURES_DELIVERY_MEDIA_TYPE
        || artifact.lineage() != futures_delivery_lineage(inputs)?.as_slice()
    {
        return Err(map_domain_error(DomainErrorCode::BrokenLineage));
    }
    Ok(())
}
