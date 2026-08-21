use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use async_trait::async_trait;
use ficant_application::ports::{
    AppendDefinitionVersion, AppendMarketFact, CorrectMarketFact, DefinitionIdentity,
    DefinitionRepository, DefinitionValue, InstrumentDefinition, MarketFact, MarketFactFieldRole,
    MarketFactRulePackResolver, MarketFactUnitResolver,
};
use ficant_application::{AccessScope, ApplicationError, ApplicationErrorCategory, IdempotencyKey};
use ficant_domain::VersionedDefinition;
use ficant_domain::market::{
    Cashflow, CashflowInput, CashflowType, FactSource, Instrument, InstrumentInput, InstrumentKind,
    MarketRulePack, MarketRulePackTimesInput, Quote, QuoteInput, Trade, TradeInput, Unit,
    UnitInput, Valuation, ValuationInput, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, MarketTime, OwnerRef, Ulid, UnitRef, Version,
};

#[test]
fn q2_inv_01_quote_rate_and_trade_quantity_price_fail_at_application_boundary() {
    let definitions = FakeDefinitions::new([unit('P', "price", 4, 12), unit('R', "rate", 6, 12)]);
    let resolver = MarketFactUnitResolver::new(&definitions);

    let quote_error =
        block_on(resolver.resolve(&scope(), MarketFact::Quote(quote(decimal("10125", 2, 'R')))))
            .expect_err("Quote bid RATE must not cross the application boundary");
    assert_invalid_unit(&quote_error);

    let trade_error = block_on(resolver.resolve(
        &scope(),
        MarketFact::Trade(trade(decimal("10125", 2, 'P'), decimal("10", 0, 'P'))),
    ))
    .expect_err("Trade quantity PRICE must not cross the application boundary");
    assert_invalid_unit(&trade_error);
    assert_eq!(definitions.calls.load(Ordering::SeqCst), 3);
}

#[test]
fn resolver_rejects_missing_kind_identity_version_tenant_dimension_scale_and_precision() {
    let fact = MarketFact::Quote(quote(decimal("10125", 2, 'P')));
    let cases = [
        FakeDefinitions::new([]),
        FakeDefinitions::with_value(
            'P',
            1,
            DefinitionValue::Instrument(other_instrument_definition('P')),
        ),
        FakeDefinitions::with_value('P', 1, DefinitionValue::Unit(unit('R', "price", 4, 12))),
        FakeDefinitions::with_value(
            'P',
            1,
            DefinitionValue::Unit(unit_with_owner('P', 2, "price", 4, 12, owner())),
        ),
        FakeDefinitions::with_value('P', 1, DefinitionValue::Unit(unit('P', "price", 4, 12))),
        FakeDefinitions::with_value(
            'P',
            1,
            DefinitionValue::Unit(unit_with_owner(
                'P',
                1,
                "price",
                4,
                12,
                OwnerRef::new(id('K'), id('Y')),
            )),
        ),
        FakeDefinitions::with_value('P', 1, DefinitionValue::Unit(unit('P', "rate", 4, 12))),
        FakeDefinitions::with_value('P', 1, DefinitionValue::Unit(unit('P', "price", 1, 12))),
        FakeDefinitions::with_value('P', 1, DefinitionValue::Unit(unit('P', "price", 2, 4))),
    ];

    for (index, definitions) in cases.iter().enumerate() {
        let result =
            block_on(MarketFactUnitResolver::new(definitions).resolve(&scope(), fact.clone()));
        if index == 4 {
            result.expect("control Unit must resolve");
        } else {
            assert_invalid_unit(&result.expect_err("invalid Unit definition must fail closed"));
        }
    }
}

#[test]
fn legal_cashflow_quote_trade_and_valuation_produce_read_only_bindings() {
    let definitions = FakeDefinitions::new([
        unit('C', "currency", 2, 18),
        unit('P', "price", 4, 18),
        unit('N', "notional", 0, 18),
    ]);
    let resolver = MarketFactUnitResolver::new(&definitions);
    let facts = [
        MarketFact::Cashflow(cashflow(decimal("1250", 2, 'C'))),
        MarketFact::Quote(quote(decimal("10125", 2, 'P'))),
        MarketFact::Trade(trade(decimal("10125", 2, 'P'), decimal("10", 0, 'N'))),
        MarketFact::Valuation(valuation(vec![
            decimal("10050", 2, 'P'),
            decimal("10075", 2, 'P'),
        ])),
    ];

    for (index, fact) in facts.into_iter().enumerate() {
        let validated = block_on(resolver.resolve(&scope(), fact)).unwrap();
        assert!(!validated.proof().bindings().is_empty());
        let expected_roles = match index {
            0 => vec![(MarketFactFieldRole::Currency, 0)],
            1 => vec![(MarketFactFieldRole::Price, 0)],
            2 => vec![
                (MarketFactFieldRole::Price, 0),
                (MarketFactFieldRole::Notional, 0),
            ],
            3 => vec![
                (MarketFactFieldRole::Price, 0),
                (MarketFactFieldRole::Price, 1),
            ],
            _ => unreachable!(),
        };
        assert_eq!(
            validated
                .proof()
                .bindings()
                .iter()
                .map(|binding| (binding.role(), binding.ordinal()))
                .collect::<Vec<_>>(),
            expected_roles
        );
        for binding in validated.proof().bindings() {
            assert_eq!(binding.dimension(), expected_dimension(binding.role()));
        }
        let fully_validated =
            block_on(MarketFactRulePackResolver::new(&definitions).resolve(&scope(), validated))
                .unwrap();
        AppendMarketFact::new(fully_validated, key(&format!("legal-{index}"))).unwrap();
    }
}

#[test]
fn append_fingerprint_ignores_proof_metadata_and_correction_requires_validated_fact() {
    let narrow = FakeDefinitions::new([unit('P', "price", 2, 6)]);
    let wide = FakeDefinitions::new([unit('P', "price", 4, 18)]);
    let fact = MarketFact::Quote(quote(decimal("10125", 2, 'P')));
    let narrow_command =
        AppendMarketFact::new(fully_resolve(&narrow, fact.clone()), key("same-intent")).unwrap();
    let wide_command =
        AppendMarketFact::new(fully_resolve(&wide, fact), key("same-intent")).unwrap();
    assert_eq!(narrow_command.fingerprint(), wide_command.fingerprint());
    assert_ne!(
        narrow_command.proof().binding_hash(),
        wide_command.proof().binding_hash()
    );

    let original_id = id('Q');
    let correction = MarketFact::Quote(quote_with_identity_and_supersedes(
        'S',
        Some(original_id.clone()),
        decimal("10150", 2, 'P'),
    ));
    let validated = fully_resolve(&wide, correction);
    CorrectMarketFact::new(original_id, validated, key("correction")).unwrap();
}

struct FakeDefinitions {
    values: BTreeMap<(String, u64), DefinitionValue>,
    calls: AtomicUsize,
}

impl FakeDefinitions {
    fn new(units: impl IntoIterator<Item = Unit>) -> Self {
        let mut values = units
            .into_iter()
            .map(|unit| {
                (
                    (unit.identity().to_owned(), unit.version()),
                    DefinitionValue::Unit(unit),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let rule = rule_pack();
        values.insert(
            (rule.identity().to_owned(), rule.version()),
            DefinitionValue::MarketRulePack(rule),
        );
        Self {
            values,
            calls: AtomicUsize::new(0),
        }
    }

    fn with_value(requested_suffix: char, requested_version: u64, value: DefinitionValue) -> Self {
        Self {
            values: BTreeMap::from([(
                (id(requested_suffix).as_str().to_owned(), requested_version),
                value,
            )]),
            calls: AtomicUsize::new(0),
        }
    }
}

fn fully_resolve(
    definitions: &FakeDefinitions,
    fact: MarketFact,
) -> ficant_application::ports::FullyValidatedMarketFact {
    let unit = block_on(MarketFactUnitResolver::new(definitions).resolve(&scope(), fact)).unwrap();
    block_on(MarketFactRulePackResolver::new(definitions).resolve(&scope(), unit)).unwrap()
}

fn rule_pack() -> MarketRulePack {
    MarketRulePack::new_with_times(MarketRulePackTimesInput {
        rule_pack_id: id('R'),
        version: version(1),
        owner: owner(),
        market: "XSHG".to_owned(),
        rule_type: "valuation".to_owned(),
        source: "test".to_owned(),
        from: time(1),
        to: time(3),
        verification_status: VerificationStatus::Verified,
        content_hash: ContentHash::digest(b"rule-pack"),
    })
    .unwrap()
}

#[async_trait]
impl DefinitionRepository for FakeDefinitions {
    async fn create_identity(&self, _identity: DefinitionIdentity) -> Result<(), ApplicationError> {
        unreachable!()
    }

    async fn append_version(
        &self,
        _command: AppendDefinitionVersion,
    ) -> Result<DefinitionValue, ApplicationError> {
        unreachable!()
    }

    async fn get_version(
        &self,
        _scope: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .values
            .get(&(definition_id.as_str().to_owned(), version.get()))
            .cloned())
    }

    async fn resolve_as_of(
        &self,
        _scope: &AccessScope,
        _definition_id: Ulid,
        _instant: MarketTime,
    ) -> Result<Option<DefinitionValue>, ApplicationError> {
        unreachable!()
    }
}

fn quote(bid: DecimalValue) -> Quote {
    quote_with_identity_and_supersedes('Q', None, bid)
}

fn quote_with_identity_and_supersedes(
    quote_suffix: char,
    supersedes_id: Option<Ulid>,
    bid: DecimalValue,
) -> Quote {
    Quote::new(QuoteInput {
        quote_id: id(quote_suffix),
        instrument: version_ref('K'),
        owner: owner(),
        source: FactSource::new("unit-test", "quote", 1).unwrap(),
        observed_at: time(1),
        received_at: time(2),
        bid: Some(bid),
        ask: None,
        supersedes_id,
    })
    .unwrap()
}

fn cashflow(amount: DecimalValue) -> Cashflow {
    Cashflow::new(CashflowInput {
        cashflow_id: id('F'),
        bond: version_ref('B'),
        payment_time: time(2),
        amount,
        owner: owner(),
        source: FactSource::new("unit-test", "cashflow", 1).unwrap(),
        supersedes_id: None,
        cashflow_type: CashflowType::Coupon,
        schedule_id: "schedule-1".to_owned(),
        sequence: 1,
    })
    .unwrap()
}

fn valuation(values: Vec<DecimalValue>) -> Valuation {
    Valuation::new(ValuationInput {
        valuation_id: id('V'),
        instrument: version_ref('K'),
        owner: owner(),
        source: FactSource::new("unit-test", "valuation", 1).unwrap(),
        valuation_at: time(2),
        method: "mark".to_owned(),
        rule_pack: version_ref('R'),
        values,
        supersedes_id: None,
    })
    .unwrap()
}

fn trade(price: DecimalValue, quantity: DecimalValue) -> Trade {
    Trade::new(TradeInput {
        trade_id: id('D'),
        instrument: version_ref('K'),
        owner: owner(),
        source: FactSource::new("unit-test", "trade", 1).unwrap(),
        executed_at: time(2),
        price,
        quantity,
        supersedes_id: None,
    })
    .unwrap()
}

fn unit(suffix: char, dimension: &str, scale: u32, precision: u32) -> Unit {
    unit_with_owner(suffix, 1, dimension, scale, precision, owner())
}

fn unit_with_owner(
    suffix: char,
    unit_version: u64,
    dimension: &str,
    scale: u32,
    precision: u32,
    unit_owner: OwnerRef,
) -> Unit {
    Unit::new(UnitInput {
        unit_id: id(suffix),
        version: version(unit_version),
        owner: unit_owner,
        code: format!("UNIT_{suffix}"),
        dimension: dimension.to_owned(),
        scale,
        precision,
    })
    .unwrap()
}

fn other_instrument_definition(suffix: char) -> InstrumentDefinition {
    let instrument = Instrument::new(InstrumentInput {
        instrument_id: id(suffix),
        version: version(1),
        owner: owner(),
        kind: InstrumentKind::Other,
        market: "XSHG".to_owned(),
        symbol: format!("TEST-{suffix}"),
        currency: UnitRef::new(id('C'), version(1)),
        calendar: version_ref('A'),
    })
    .unwrap();
    InstrumentDefinition::new(instrument, None).unwrap()
}

fn decimal(coefficient: &str, scale: u32, unit_suffix: char) -> DecimalValue {
    DecimalValue::new(
        coefficient,
        scale,
        UnitRef::new(id(unit_suffix), version(1)),
    )
    .unwrap()
}

fn assert_invalid_unit(error: &ApplicationError) {
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert!(!error.retryable());
}

fn expected_dimension(role: MarketFactFieldRole) -> &'static str {
    match role {
        MarketFactFieldRole::Currency => "currency",
        MarketFactFieldRole::Price => "price",
        MarketFactFieldRole::Notional => "notional",
        MarketFactFieldRole::Rate => "rate",
        MarketFactFieldRole::Years => "years",
    }
}

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).unwrap()
}

fn scope() -> AccessScope {
    AccessScope::new(id('T'), id('A'), vec![id('Y')]).unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('Y'))
}

fn version_ref(suffix: char) -> ficant_domain::primitives::VersionRef {
    ficant_domain::primitives::VersionRef::new(id(suffix), version(1))
}

fn version(value: u64) -> Version {
    Version::new(value).unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn time(hour: u32) -> MarketTime {
    let instant = format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap();
    MarketTime::new(instant, "Asia/Shanghai", "2026-03-04".parse().unwrap()).unwrap()
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    struct ThreadWake(std::thread::Thread);
    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}
