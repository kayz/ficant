use chrono::{TimeZone, Utc};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    AccountingBook, AccountingClassification, AccountingClassificationState, Position,
    PositionHoldingForm, PositionInput, PositionSnapshot, PositionSnapshotInput,
};
use ficant_domain::{ContentAddressed, DomainErrorCode};

#[test]
fn snapshot_hash_and_double_time_are_immutable() {
    let mut input = snapshot_input(
        PositionHoldingForm::Owned,
        AccountingClassificationState::Classified,
        Some(AccountingBook::Ac),
    );
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    let snapshot = PositionSnapshot::new(input.clone()).unwrap();
    assert_eq!(snapshot.content_hash(), &input.content_hash);

    let mut revised_lineage = input.clone();
    revised_lineage.lineage = vec![LineageRef::content_addressed(
        id('K'),
        ContentHash::digest(b"revised-lineage"),
    )];
    assert_ne!(
        PositionSnapshot::content_hash_for(&revised_lineage),
        input.content_hash
    );

    input.content_hash = ContentHash::digest(b"wrong");
    assert_eq!(
        PositionSnapshot::new(input).unwrap_err(),
        DomainErrorCode::ContentHashMismatch
    );

    let mut invalid_time = snapshot_input(
        PositionHoldingForm::Owned,
        AccountingClassificationState::Classified,
        Some(AccountingBook::Ac),
    );
    invalid_time.observed_at = market_time(2);
    invalid_time.visible_at = market_time(1);
    invalid_time.content_hash = PositionSnapshot::content_hash_for(&invalid_time);
    assert_eq!(
        PositionSnapshot::new(invalid_time).unwrap_err(),
        DomainErrorCode::InvalidEffectiveTime
    );
}

#[test]
fn accounting_state_is_explicit_and_closed() {
    assert!(
        AccountingClassification::new(
            AccountingClassificationState::Classified,
            Some(AccountingBook::Fvtpl)
        )
        .is_ok()
    );
    assert!(
        AccountingClassification::new(AccountingClassificationState::NotApplicable, None).is_ok()
    );
    assert!(AccountingClassification::new(AccountingClassificationState::Unknown, None).is_ok());
    assert_eq!(
        AccountingClassification::new(AccountingClassificationState::Classified, None).unwrap_err(),
        DomainErrorCode::InvalidValue
    );
    assert_eq!(
        AccountingClassification::new(
            AccountingClassificationState::Unknown,
            Some(AccountingBook::Ac)
        )
        .unwrap_err(),
        DomainErrorCode::InvalidValue
    );
}

#[test]
fn repo_forms_never_promote_collateral_to_available_liquidity() {
    let classification = AccountingClassification::new(
        AccountingClassificationState::Classified,
        Some(AccountingBook::Ac),
    )
    .unwrap();
    let owned = position('A', PositionHoldingForm::Owned, classification.clone());
    let repo_sold = position('B', PositionHoldingForm::RepoSold, classification.clone());
    let reverse = position(
        'C',
        PositionHoldingForm::ReverseRepoCollateral,
        classification,
    );
    assert_eq!(
        (
            owned.includes_position_exposure(),
            owned.includes_available_liquidity()
        ),
        (true, true)
    );
    assert_eq!(
        (
            repo_sold.includes_position_exposure(),
            repo_sold.includes_available_liquidity()
        ),
        (true, false)
    );
    assert_eq!(
        (
            reverse.includes_position_exposure(),
            reverse.includes_available_liquidity()
        ),
        (false, false)
    );
}

#[test]
fn accounting_and_economic_views_remain_separate_imported_facts() {
    let ac = position(
        'A',
        PositionHoldingForm::Owned,
        AccountingClassification::new(
            AccountingClassificationState::Classified,
            Some(AccountingBook::Ac),
        )
        .unwrap(),
    );
    let fvtpl = Position::new(PositionInput {
        position_id: id('B'),
        instrument_ref: ac.instrument_ref().clone(),
        quantity: ac.quantity().clone(),
        economic_value: ac.economic_value().clone(),
        economic_pnl: ac.economic_pnl().clone(),
        accounting_pnl: decimal("13", id('M')),
        capital_requirement: ac.capital_requirement().clone(),
        accounting_classification: AccountingClassification::new(
            AccountingClassificationState::Classified,
            Some(AccountingBook::Fvtpl),
        )
        .unwrap(),
        holding_form: PositionHoldingForm::Owned,
    })
    .unwrap();
    assert_eq!(ac.quantity(), fvtpl.quantity());
    assert_eq!(ac.economic_value(), fvtpl.economic_value());
    assert_eq!(ac.economic_pnl(), fvtpl.economic_pnl());
    assert_ne!(ac.accounting_pnl(), fvtpl.accounting_pnl());
    assert_eq!(
        ac.accounting_classification().book(),
        Some(AccountingBook::Ac)
    );
    assert_eq!(
        fvtpl.accounting_classification().book(),
        Some(AccountingBook::Fvtpl)
    );
}

#[test]
fn capital_values_only_aggregate_with_the_same_unit() {
    let amount = decimal("20", id('M'));
    assert_eq!(
        amount
            .checked_add(&decimal("3", id('M')))
            .unwrap()
            .coefficient(),
        "23"
    );
    assert_eq!(
        amount.checked_add(&decimal("3", id('Q'))).unwrap_err(),
        DomainErrorCode::InvalidUnit
    );
}

fn snapshot_input(
    form: PositionHoldingForm,
    state: AccountingClassificationState,
    book: Option<AccountingBook>,
) -> PositionSnapshotInput {
    let classification = AccountingClassification::new(state, book).unwrap();
    PositionSnapshotInput {
        snapshot_id: id('S'),
        owner: OwnerRef::new(id('T'), id('N')),
        subject_ref: VersionRef::new(id('V'), Version::new(1).unwrap()),
        observed_at: market_time(1),
        visible_at: market_time(1),
        content_hash: ContentHash::digest(b"placeholder"),
        lineage: vec![LineageRef::content_addressed(
            id('K'),
            ContentHash::digest(b"lineage"),
        )],
        positions: vec![position('A', form, classification)],
    }
}

fn position(
    suffix: char,
    form: PositionHoldingForm,
    classification: AccountingClassification,
) -> Position {
    Position::new(PositionInput {
        position_id: id(suffix),
        instrument_ref: VersionRef::new(id('J'), Version::new(1).unwrap()),
        quantity: decimal("100", id('Q')),
        economic_value: decimal("1000", id('M')),
        economic_pnl: decimal("10", id('M')),
        accounting_pnl: decimal("7", id('M')),
        capital_requirement: decimal("20", id('M')),
        accounting_classification: classification,
        holding_form: form,
    })
    .unwrap()
}

fn decimal(coefficient: &str, unit: Ulid) -> DecimalValue {
    DecimalValue::new(coefficient, 0, UnitRef::new(unit, Version::new(1).unwrap())).unwrap()
}

fn market_time(hour: u32) -> MarketTime {
    let instant = Utc.with_ymd_and_hms(2026, 7, 31, hour, 0, 0).unwrap();
    MarketTime::new(
        instant,
        "Asia/Shanghai",
        chrono::NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(),
    )
    .unwrap()
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
