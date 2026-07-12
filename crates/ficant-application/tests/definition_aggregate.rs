use async_trait::async_trait;
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, ApplicationResult, DefinitionIdentity, DefinitionKind,
    DefinitionRepository, DefinitionValue, InstrumentDefinition, InstrumentSubtype,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory, IdempotencyKey};
use ficant_domain::market::{Bond, FuturesContract, Instrument, InstrumentInput, InstrumentKind};
use ficant_domain::primitives::{
    DecimalValue, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};

#[test]
fn bond_and_futures_are_complete_single_instrument_versions() {
    let bond_instrument = instrument('B', 1, InstrumentKind::Bond);
    let bond = Bond::new(
        &bond_instrument,
        "2026-01-01".parse().unwrap(),
        "2036-01-01".parse().unwrap(),
        amount("100000"),
    )
    .unwrap();
    let bond_definition =
        InstrumentDefinition::new(bond_instrument, Some(InstrumentSubtype::Bond(bond))).unwrap();

    let futures_instrument = instrument('F', 1, InstrumentKind::Futures);
    let futures = futures_contract(&futures_instrument, "1000");
    let futures_definition = InstrumentDefinition::new(
        futures_instrument,
        Some(InstrumentSubtype::FuturesContract(futures)),
    )
    .unwrap();

    for (name, definition) in [
        ("bond-v1", bond_definition),
        ("futures-v1", futures_definition),
    ] {
        let command =
            AppendDefinitionVersion::new(None, DefinitionValue::Instrument(definition), key(name))
                .unwrap();
        assert_eq!(command.value().version(), 1);
        assert_eq!(command.value().kind(), DefinitionKind::Instrument);
    }
}

#[test]
fn aggregate_rejects_identity_version_kind_and_subtype_shape_mismatch() {
    let target = instrument('B', 1, InstrumentKind::Bond);

    let other_identity = instrument('K', 1, InstrumentKind::Bond);
    let other_identity_bond = bond(&other_identity, "2036-01-01", "100000");
    assert_category(
        &InstrumentDefinition::new(
            target.clone(),
            Some(InstrumentSubtype::Bond(other_identity_bond)),
        )
        .unwrap_err(),
        ApplicationErrorCategory::VersionConflict,
    );

    let other_version = instrument('B', 2, InstrumentKind::Bond);
    let other_version_bond = bond(&other_version, "2036-01-01", "100000");
    assert_category(
        &InstrumentDefinition::new(
            target.clone(),
            Some(InstrumentSubtype::Bond(other_version_bond)),
        )
        .unwrap_err(),
        ApplicationErrorCategory::VersionConflict,
    );

    let futures_instrument = instrument('F', 1, InstrumentKind::Futures);
    let futures = futures_contract(&futures_instrument, "1000");
    assert_category(
        &InstrumentDefinition::new(
            target.clone(),
            Some(InstrumentSubtype::FuturesContract(futures)),
        )
        .unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );

    assert_category(
        &InstrumentDefinition::new(target, None).unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );
    assert_category(
        &InstrumentDefinition::new(futures_instrument, None).unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );

    let other = instrument('R', 1, InstrumentKind::Other);
    let bond_source = instrument('S', 1, InstrumentKind::Bond);
    let forbidden_bond = bond(&bond_source, "2036-01-01", "100000");
    assert_category(
        &InstrumentDefinition::new(other, Some(InstrumentSubtype::Bond(forbidden_bond)))
            .unwrap_err(),
        ApplicationErrorCategory::ValidationFailed,
    );
}

#[test]
fn definition_identity_and_append_expose_one_root_identity_and_version() {
    let instrument = instrument('B', 1, InstrumentKind::Bond);
    let definition_id = instrument.id().clone();
    let owner = instrument.owner().clone();
    let aggregate = InstrumentDefinition::new(
        instrument.clone(),
        Some(InstrumentSubtype::Bond(bond(
            &instrument,
            "2036-01-01",
            "100000",
        ))),
    )
    .unwrap();
    let value = DefinitionValue::Instrument(aggregate);
    let identity = DefinitionIdentity::new(
        definition_id.clone(),
        owner.clone(),
        DefinitionKind::Instrument,
        key("create-bond"),
    );
    let command = AppendDefinitionVersion::new(None, value, key("append-bond-v1")).unwrap();

    assert_eq!(identity.definition_id(), &definition_id);
    assert_eq!(identity.owner(), &owner);
    assert_eq!(identity.kind(), DefinitionKind::Instrument);
    assert_eq!(command.value().identity(), definition_id.as_str());
    assert_eq!(command.value().owner(), &owner);
    assert_eq!(command.value().kind(), DefinitionKind::Instrument);
    assert_eq!(command.value().version(), 1);
}

#[test]
fn subtype_business_field_changes_the_fcmd_v1_fingerprint() {
    let instrument = instrument('B', 1, InstrumentKind::Bond);
    let original = InstrumentDefinition::new(
        instrument.clone(),
        Some(InstrumentSubtype::Bond(bond(
            &instrument,
            "2036-01-01",
            "100000",
        ))),
    )
    .unwrap();
    let changed_maturity = InstrumentDefinition::new(
        instrument.clone(),
        Some(InstrumentSubtype::Bond(bond(
            &instrument,
            "2037-01-01",
            "100000",
        ))),
    )
    .unwrap();
    let changed_face_value = InstrumentDefinition::new(
        instrument.clone(),
        Some(InstrumentSubtype::Bond(bond(
            &instrument,
            "2036-01-01",
            "200000",
        ))),
    )
    .unwrap();

    let original = append(original, "same-request-shape");
    let changed_maturity = append(changed_maturity, "same-request-shape");
    let changed_face_value = append(changed_face_value, "same-request-shape");
    assert_ne!(original.fingerprint(), changed_maturity.fingerprint());
    assert_ne!(original.fingerprint(), changed_face_value.fingerprint());
}

#[test]
fn repository_port_can_be_implemented_with_the_aggregate_value() {
    fn assert_repository<T: DefinitionRepository>() {}
    assert_repository::<ContractDefinitionRepository>();
}

struct ContractDefinitionRepository;

#[async_trait]
impl DefinitionRepository for ContractDefinitionRepository {
    async fn create_identity(&self, _identity: DefinitionIdentity) -> ApplicationResult<()> {
        Err(not_used())
    }

    async fn append_version(
        &self,
        _command: AppendDefinitionVersion,
    ) -> ApplicationResult<DefinitionValue> {
        Err(not_used())
    }

    async fn get_version(
        &self,
        _scope: &AccessScope,
        _definition_id: Ulid,
        _version: Version,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        Err(not_used())
    }

    async fn resolve_as_of(
        &self,
        _scope: &AccessScope,
        _definition_id: Ulid,
        _instant: MarketTime,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        Err(not_used())
    }
}

fn append(definition: InstrumentDefinition, idempotency_key: &str) -> AppendDefinitionVersion {
    AppendDefinitionVersion::new(
        None,
        DefinitionValue::Instrument(definition),
        key(idempotency_key),
    )
    .unwrap()
}

fn instrument(suffix: char, value: u64, kind: InstrumentKind) -> Instrument {
    Instrument::new(InstrumentInput {
        instrument_id: id(suffix),
        version: version(value),
        owner: owner(),
        kind,
        market: "XSHG".to_owned(),
        symbol: format!("TEST-{suffix}"),
        currency: UnitRef::new(id('M'), version(1)),
        calendar: VersionRef::new(id('C'), version(1)),
    })
    .unwrap()
}

fn bond(instrument: &Instrument, maturity: &str, face_value_coefficient: &str) -> Bond {
    Bond::new(
        instrument,
        "2026-01-01".parse().unwrap(),
        maturity.parse().unwrap(),
        amount(face_value_coefficient),
    )
    .unwrap()
}

fn futures_contract(instrument: &Instrument, multiplier: &str) -> FuturesContract {
    FuturesContract::new(
        instrument,
        time(1),
        time(2),
        time(3),
        amount(multiplier),
        VersionRef::new(id('P'), version(1)),
    )
    .unwrap()
}

fn amount(coefficient: &str) -> DecimalValue {
    DecimalValue::new(coefficient, 2, UnitRef::new(id('M'), version(1))).unwrap()
}

fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        format!("2026-03-04T{hour:02}:00:00Z").parse().unwrap(),
        "Asia/Shanghai",
        "2026-03-04".parse().unwrap(),
    )
    .unwrap()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('Y'))
}

fn version(value: u64) -> Version {
    Version::new(value).unwrap()
}

fn key(value: &str) -> IdempotencyKey {
    IdempotencyKey::new(value).unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn not_used() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}

fn assert_category(error: &ApplicationError, expected: ApplicationErrorCategory) {
    assert_eq!(error.category(), expected);
}
