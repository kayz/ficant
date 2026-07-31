use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, CanonicalQuote, CanonicalSnapshotDecoder,
    DefinitionIdentity, DefinitionRepository, DefinitionValue, FuturesDeliveryEngine,
    FuturesDeliveryRuleParser, InstrumentDefinition, InstrumentSubtype, IntegrityEvent,
    IntegrityEventSink, RequiredVerifiedBlobRead, SafeTraceContext, SnapshotVerifiedReadMetadata,
    SnapshotVerifiedReadMetadataRepository, VerifiedBlobPayload, VerifiedBlobReader,
    VerifiedBlobRole,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, CalculateFuturesDeliveryBasket,
    FuturesDeliveryCandidateBinding, FuturesDeliveryInputBindings,
    MaterializeFuturesDeliveryInputs,
};
use ficant_domain::ContentAddressed;
use ficant_domain::analytics::{
    AnalyticsError, AnalyticsObjectRef, BondTerms, BusinessDayConvention, CouponFrequency,
    DayCountConvention, FixedDecimal,
};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliverableInput, FuturesDeliveryMeasures, FuturesDeliveryResult,
    FuturesDeliveryRule, FuturesDeliveryRuleInput,
};
use ficant_domain::market::{
    Bond, BondTaxAttributes, FuturesContract, IncomeTaxStatus, Instrument, InstrumentInput,
    InstrumentKind, MarketRulePack, MarketRulePackInput, RulePackContent, ValueAddedTaxStatus,
    VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};

const PARQUET: &[u8] = b"verified-parquet";
const MANIFEST: &[u8] = b"verified-manifest";
const RULE_TYPE: &str = "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack";

#[test]
fn ac27_verified_snapshot_owns_delivery_candidates_and_prices() {
    let parser_calls = AtomicUsize::new(0);
    let engine_calls = AtomicUsize::new(0);
    let definitions = Definitions {
        values: vec![
            DefinitionValue::Instrument(futures_definition()),
            DefinitionValue::Instrument(bond_definition('B')),
            DefinitionValue::MarketRulePack(rule_pack()),
        ],
    };
    let snapshot = data_snapshot(time(12), time(12));
    let metadata = SnapshotMetadata(snapshot.clone());
    let blobs = Blobs;
    let sink = Sink;
    let decoder = Quotes {
        quote_visible_at: time(12),
        include_second_bond: false,
    };
    let parser = Parser {
        calls: &parser_calls,
    };
    let engine = Engine {
        calls: &engine_calls,
    };
    let materializer = MaterializeFuturesDeliveryInputs::new(
        &definitions,
        &metadata,
        &blobs,
        &sink,
        &decoder,
        &parser,
    );
    let verified_bindings = request_bindings(snapshot.content_hash().clone(), decimal(100));
    let verified_inputs = block_on(materializer.execute(
        &scope(),
        &verified_bindings,
        SafeTraceContext::new("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap(),
    ))
    .expect("the exact snapshot-derived candidate set and bid price must materialize");
    assert_eq!(verified_inputs.len(), 1);
    assert_eq!(verified_inputs[0].spot_clean_price(), decimal(100));
    assert_eq!(verified_inputs[0].futures_clean_price(), decimal(99));
    let _ = CalculateFuturesDeliveryBasket::new(&engine).execute(&verified_inputs);
    assert_eq!(parser_calls.load(Ordering::SeqCst), 1);
    assert_eq!(engine_calls.load(Ordering::SeqCst), 1);

    parser_calls.store(0, Ordering::SeqCst);
    engine_calls.store(0, Ordering::SeqCst);
    let bindings = request_bindings(snapshot.content_hash().clone(), decimal(110));

    let outcome = block_on(materializer.execute(
        &scope(),
        &bindings,
        SafeTraceContext::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
    ))
    .and_then(|inputs| CalculateFuturesDeliveryBasket::new(&engine).execute(&inputs));

    let error = outcome.expect_err(
        "a caller price outside the verified snapshot bid/ask must fail before parsing or native calculation",
    );
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert!(!error.retryable());
    assert_eq!(
        parser_calls.load(Ordering::SeqCst),
        0,
        "a rejected snapshot/input binding must fail before RulePack parsing"
    );
    assert_eq!(
        engine_calls.load(Ordering::SeqCst),
        0,
        "a rejected snapshot/input binding must fail before native calculation"
    );
}

#[test]
fn ac27_accepts_a_later_visible_revision_of_the_same_historical_as_of() {
    let parser_calls = AtomicUsize::new(0);
    let definitions = Definitions {
        values: vec![
            DefinitionValue::Instrument(futures_definition()),
            DefinitionValue::Instrument(bond_definition('B')),
            DefinitionValue::MarketRulePack(rule_pack()),
        ],
    };
    let snapshot = data_snapshot(time(12), time(13));
    let decoder = Quotes {
        quote_visible_at: time(13),
        include_second_bond: false,
    };
    let inputs = materialize(
        &definitions,
        &SnapshotMetadata(snapshot.clone()),
        &decoder,
        &Parser {
            calls: &parser_calls,
        },
        &request_bindings(snapshot.content_hash().clone(), decimal(100)),
    )
    .expect("later knowledge about the same historical as_of is a valid immutable view");

    assert_eq!(inputs.len(), 1);
    assert_eq!(parser_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn ac27_rejects_current_as_of_and_incomplete_snapshot_eligible_set() {
    let parser_calls = AtomicUsize::new(0);
    let engine_calls = AtomicUsize::new(0);
    let engine = Engine {
        calls: &engine_calls,
    };
    let definitions = Definitions {
        values: vec![
            DefinitionValue::Instrument(futures_definition()),
            DefinitionValue::Instrument(bond_definition('B')),
            DefinitionValue::Instrument(bond_definition('G')),
            DefinitionValue::MarketRulePack(rule_pack()),
        ],
    };
    let current_snapshot = data_snapshot(time(13), time(13));
    let current_outcome = materialize(
        &definitions,
        &SnapshotMetadata(current_snapshot.clone()),
        &Quotes {
            quote_visible_at: time(13),
            include_second_bond: false,
        },
        &Parser {
            calls: &parser_calls,
        },
        &request_bindings(current_snapshot.content_hash().clone(), decimal(100)),
    )
    .and_then(|inputs| CalculateFuturesDeliveryBasket::new(&engine).execute(&inputs));
    assert_eq!(
        current_outcome.unwrap_err().category(),
        ApplicationErrorCategory::LineageIncomplete
    );
    assert_eq!(parser_calls.load(Ordering::SeqCst), 0);
    assert_eq!(engine_calls.load(Ordering::SeqCst), 0);

    let historical_snapshot = data_snapshot(time(12), time(12));
    let incomplete_outcome = materialize(
        &definitions,
        &SnapshotMetadata(historical_snapshot.clone()),
        &Quotes {
            quote_visible_at: time(12),
            include_second_bond: true,
        },
        &Parser {
            calls: &parser_calls,
        },
        &request_bindings(historical_snapshot.content_hash().clone(), decimal(100)),
    )
    .and_then(|inputs| CalculateFuturesDeliveryBasket::new(&engine).execute(&inputs));
    assert_eq!(
        incomplete_outcome.unwrap_err().category(),
        ApplicationErrorCategory::ValidationFailed
    );
    assert_eq!(
        parser_calls.load(Ordering::SeqCst),
        1,
        "the delivery rule is necessarily parsed once to derive the eligible snapshot set"
    );
    assert_eq!(engine_calls.load(Ordering::SeqCst), 0);
}

fn materialize(
    definitions: &Definitions,
    metadata: &SnapshotMetadata,
    decoder: &Quotes,
    parser: &Parser<'_>,
    bindings: &FuturesDeliveryInputBindings,
) -> Result<Vec<FuturesDeliverableInput>, ApplicationError> {
    block_on(
        MaterializeFuturesDeliveryInputs::new(
            definitions,
            metadata,
            &Blobs,
            &Sink,
            decoder,
            parser,
        )
        .execute(
            &scope(),
            bindings,
            SafeTraceContext::new("cccccccccccccccccccccccccccccccc").unwrap(),
        ),
    )
}

struct Definitions {
    values: Vec<DefinitionValue>,
}

#[async_trait]
impl DefinitionRepository for Definitions {
    async fn create_identity(&self, _identity: DefinitionIdentity) -> Result<(), ApplicationError> {
        unreachable!("materialization performs exact reads only")
    }

    async fn append_version(
        &self,
        _command: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        unreachable!("materialization performs exact reads only")
    }

    async fn get_version(
        &self,
        _scope: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        Ok(self
            .values
            .iter()
            .find(|value| {
                value.identity() == definition_id.as_str() && value.version() == version.get()
            })
            .cloned())
    }

    async fn resolve_as_of(
        &self,
        _scope: &AccessScope,
        _definition_id: Ulid,
        _instant: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        unreachable!("materialization performs exact reads only")
    }
}

struct SnapshotMetadata(ficant_domain::research::DataSnapshot);

#[async_trait]
impl SnapshotVerifiedReadMetadataRepository for SnapshotMetadata {
    async fn get_verified_read_metadata(
        &self,
        _scope: &AccessScope,
        snapshot_id: Ulid,
    ) -> Result<Option<SnapshotVerifiedReadMetadata>, ApplicationError> {
        if snapshot_id != *self.0.id() {
            return Ok(None);
        }
        SnapshotVerifiedReadMetadata::data(
            self.0.clone(),
            PARQUET.len() as u64,
            MANIFEST.len() as u64,
        )
        .map(Some)
    }
}

struct Blobs;

#[async_trait]
impl VerifiedBlobReader for Blobs {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> Result<VerifiedBlobPayload, ApplicationError> {
        let bytes = match request.blob_role() {
            VerifiedBlobRole::DataParquet => PARQUET,
            VerifiedBlobRole::DataManifest => MANIFEST,
            _ => unreachable!("AC27 reads the two DataSnapshot roles only"),
        };
        request.verify_bytes(sink, bytes.to_vec()).await
    }
}

struct Sink;

#[async_trait]
impl IntegrityEventSink for Sink {
    async fn emit(&self, _event: IntegrityEvent) -> Result<(), ApplicationError> {
        unreachable!("fixture payloads match frozen hashes and sizes")
    }
}

struct Quotes {
    quote_visible_at: MarketTime,
    include_second_bond: bool,
}

#[async_trait]
impl CanonicalSnapshotDecoder for Quotes {
    async fn decode_quotes(
        &self,
        snapshot: &ficant_domain::research::DataSnapshot,
        parquet: &[u8],
        manifest: &[u8],
    ) -> Result<Vec<CanonicalQuote>, ApplicationError> {
        assert_eq!(snapshot.content_hash(), &ContentHash::digest(PARQUET));
        assert_eq!(parquet, PARQUET);
        assert_eq!(manifest, MANIFEST);
        let mut quotes = vec![
            CanonicalQuote::new(
                reference('B'),
                time(11),
                self.quote_visible_at.clone(),
                "2026-03-04".parse().unwrap(),
                Some(decimal(100)),
                Some(decimal(101)),
                price_unit(),
            ),
            CanonicalQuote::new(
                reference('F'),
                time(11),
                self.quote_visible_at.clone(),
                "2026-03-04".parse().unwrap(),
                Some(decimal(99)),
                Some(decimal(100)),
                price_unit(),
            ),
        ];
        if self.include_second_bond {
            quotes.push(CanonicalQuote::new(
                reference('G'),
                time(11),
                self.quote_visible_at.clone(),
                "2026-03-04".parse().unwrap(),
                Some(decimal(102)),
                Some(decimal(103)),
                price_unit(),
            ));
        }
        Ok(quotes)
    }
}

struct Parser<'a> {
    calls: &'a AtomicUsize,
}

impl FuturesDeliveryRuleParser for Parser<'_> {
    fn market(&self) -> &'static str {
        "CFFEX"
    }

    fn rule_type(&self) -> &'static str {
        "cgb-futures"
    }

    fn type_url(&self) -> &'static str {
        RULE_TYPE
    }

    fn parse(
        &self,
        _content: &RulePackContent,
        _product: CgbFuturesProduct,
    ) -> Result<FuturesDeliveryRule, ApplicationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(rule())
    }
}

struct Engine<'a> {
    calls: &'a AtomicUsize,
}

impl FuturesDeliveryEngine for Engine<'_> {
    fn calculate(
        &self,
        input: &FuturesDeliverableInput,
    ) -> Result<FuturesDeliveryResult, AnalyticsError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(FuturesDeliveryResult::new(
            input.clone(),
            FuturesDeliveryMeasures::new(
                1,
                1,
                decimal(1),
                FixedDecimal::ZERO,
                FixedDecimal::ZERO,
                FixedDecimal::ZERO,
                decimal(100),
                decimal(100),
                decimal(1),
                FixedDecimal::ZERO,
                FixedDecimal::ZERO,
                FixedDecimal::ZERO,
                FixedDecimal::ZERO,
                FixedDecimal::ZERO,
            )
            .map_err(|_| AnalyticsError::Internal)?,
        ))
    }
}

fn request_bindings(
    snapshot_hash: ContentHash,
    spot_clean_price: FixedDecimal,
) -> FuturesDeliveryInputBindings {
    FuturesDeliveryInputBindings::new(
        owner(),
        object('F', ContentHash::digest(b"futures")),
        object('R', ContentHash::digest(b"rule")),
        object('D', snapshot_hash),
        time(12),
        "2026-03-05".parse().unwrap(),
        "2026-06-01".parse().unwrap(),
        "2026-06-15".parse().unwrap(),
        CgbFuturesProduct::TenYear,
        vec![FuturesDeliveryCandidateBinding::new(
            object('B', ContentHash::digest(b"bond")),
            bond_terms(),
            spot_clean_price,
        )],
        decimal(99),
        decimal(2),
        price_unit(),
    )
}

fn futures_definition() -> InstrumentDefinition {
    let instrument = instrument('F', InstrumentKind::Futures);
    let contract = FuturesContract::new(
        &instrument,
        time(13),
        time(14),
        time(15),
        DecimalValue::new("1", 0, UnitRef::new(id('N'), version())).unwrap(),
        reference('R'),
    )
    .unwrap();
    InstrumentDefinition::new(
        instrument,
        Some(InstrumentSubtype::FuturesContract(contract)),
    )
    .unwrap()
}

fn bond_definition(suffix: char) -> InstrumentDefinition {
    let instrument = instrument(suffix, InstrumentKind::Bond);
    let bond = Bond::with_issuance(
        &instrument,
        "2025-01-01".parse().unwrap(),
        "2025-06-01".parse().unwrap(),
        "2034-06-15".parse().unwrap(),
        DecimalValue::new("100", 0, currency_unit()).unwrap(),
        tax_attributes(),
        DecimalValue::new("100", 0, currency_unit()).unwrap(),
    )
    .unwrap();
    InstrumentDefinition::new(instrument, Some(InstrumentSubtype::Bond(bond))).unwrap()
}

fn instrument(suffix: char, kind: InstrumentKind) -> Instrument {
    Instrument::new(InstrumentInput {
        instrument_id: id(suffix),
        version: version(),
        owner: owner(),
        kind,
        market: "CFFEX".to_owned(),
        symbol: format!("{suffix}-fixture"),
        currency: currency_unit(),
        calendar: reference('C'),
    })
    .unwrap()
}

fn bond_terms() -> BondTerms {
    BondTerms::with_issuance(
        "2025-01-01".parse().unwrap(),
        "2025-06-01".parse().unwrap(),
        "2034-06-15".parse().unwrap(),
        CouponFrequency::Semiannual,
        DayCountConvention::ActActBondIsma,
        BusinessDayConvention::Following,
        decimal(3),
        decimal(100),
        decimal(100),
        tax_attributes(),
    )
    .unwrap()
}

fn tax_attributes() -> BondTaxAttributes {
    BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Taxable)
}

fn rule_pack() -> MarketRulePack {
    let content = RulePackContent::new(RULE_TYPE, b"rule".to_vec()).unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id('R'),
            version: version(),
            owner: owner(),
            market: "CFFEX".to_owned(),
            rule_type: "cgb-futures".to_owned(),
            source: "fixture".to_owned(),
            effective: EffectivePeriod::new(time(1), time(15)).unwrap(),
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(content.value()),
        },
        content,
    )
    .unwrap()
}

fn rule() -> FuturesDeliveryRule {
    FuturesDeliveryRule::new(FuturesDeliveryRuleInput {
        original_term_max_months: 120,
        residual_min_months: 78,
        residual_max_months: None,
        delivery_months: vec![3, 6, 9, 12],
        nominal_coupon: decimal(3),
        face_quote_basis: decimal(100),
        accrued_interest_day_count: 365,
        conversion_factor_rounding_places: 4,
        accrued_interest_rounding_places: 7,
        annual_day_basis: 365,
    })
    .unwrap()
}

fn data_snapshot(
    as_of: MarketTime,
    visible_at: MarketTime,
) -> ficant_domain::research::DataSnapshot {
    ficant_domain::research::DataSnapshot::new(ficant_domain::research::DataSnapshotInput {
        data_snapshot_id: id('D'),
        owner: owner(),
        visible_at,
        as_of,
        schema_hash: ContentHash::digest(b"schema"),
        manifest_hash: ContentHash::digest(MANIFEST),
        blob_content_hash: ContentHash::digest(PARQUET),
        lineage: vec![LineageRef::content_addressed(
            id('L'),
            ContentHash::digest(b"source"),
        )],
    })
    .unwrap()
}

fn object(suffix: char, hash: ContentHash) -> AnalyticsObjectRef {
    AnalyticsObjectRef::new(reference(suffix), hash)
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('A'), vec![id('O')]).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('O'))
}

fn reference(suffix: char) -> VersionRef {
    VersionRef::new(id(suffix), version())
}

fn price_unit() -> UnitRef {
    UnitRef::new(id('P'), version())
}

fn currency_unit() -> UnitRef {
    UnitRef::new(id('Y'), version())
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn decimal(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value * 1_000_000_000_000)
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-04".parse().unwrap(),
    )
    .unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => 'J',
        'L' => 'M',
        'O' => 'Q',
        'U' => 'W',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut context = Context::from_waker(Waker::noop());
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}
