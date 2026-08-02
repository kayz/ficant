use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{AccessScope, ApplicationResult, PositionSnapshotRepository};
use ficant_application::{ApplicationErrorCategory, ApplicationErrorDetail, PositionViewsUseCase};
use ficant_domain::ContentAddressed;
use ficant_domain::primitives::{
    ContentHash, DecimalValue, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::research::{
    AccountingClassification, AccountingClassificationState, Position, PositionHoldingForm,
    PositionInput, PositionSnapshot, PositionSnapshotInput,
};

struct Repository {
    snapshot: PositionSnapshot,
}

#[async_trait]
impl PositionSnapshotRepository for Repository {
    async fn get_position_snapshot(
        &self,
        _scope: &AccessScope,
        id: Ulid,
        knowledge: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>> {
        Ok((self.snapshot.id() == &id
            && self.snapshot.visible_at().instant() <= knowledge.instant())
        .then(|| self.snapshot.clone()))
    }
    async fn resolve_position_snapshot(
        &self,
        _scope: &AccessScope,
        subject: VersionRef,
        observed: MarketTime,
        knowledge: MarketTime,
    ) -> ApplicationResult<Option<PositionSnapshot>> {
        Ok((self.snapshot.subject_ref() == &subject
            && self.snapshot.observed_at() == &observed
            && self.snapshot.visible_at().instant() <= knowledge.instant())
        .then(|| self.snapshot.clone()))
    }
}

#[tokio::test]
async fn unknown_classification_fails_closed_and_reverse_repo_is_not_liquidity() {
    let snapshot = snapshot();
    let scope = AccessScope::new(id('T'), id('A'), vec![id('N')]).unwrap();
    let repository = Repository {
        snapshot: snapshot.clone(),
    };
    let use_case = PositionViewsUseCase::new(&repository);
    let error = use_case
        .capital_use(&scope, snapshot.id().clone(), time(2))
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert_eq!(
        error.detail(),
        Some(&ApplicationErrorDetail::UnknownAccountingPositions {
            position_ids: vec![snapshot.positions()[0].id().as_str().to_owned()],
        })
    );
    let views = use_case
        .views(&scope, snapshot.id().clone(), time(2))
        .await
        .unwrap();
    assert_eq!(views.coverage.imported_position_count(), 1);
    assert_eq!(views.coverage.participating_position_count(), 1);
    assert_eq!(views.coverage.missing_critical_field_record_count(), 0);
    assert!(views.coverage.source_confidence().is_none());
    assert_eq!(
        (
            views.positions[0].included_in_position_exposure,
            views.positions[0].included_in_available_liquidity,
            views.positions[0].collateral_fact
        ),
        (false, false, true)
    );
}

#[tokio::test]
async fn complete_snapshot_returns_coverage_on_views_and_capital_use() {
    let snapshot = snapshot_with_classification(AccountingClassificationState::Classified);
    let scope = AccessScope::new(id('T'), id('A'), vec![id('N')]).unwrap();
    let repository = Repository {
        snapshot: snapshot.clone(),
    };
    let use_case = PositionViewsUseCase::new(&repository);

    let views = use_case
        .views(&scope, snapshot.id().clone(), time(2))
        .await
        .unwrap();
    let capital = use_case
        .capital_use(&scope, snapshot.id().clone(), time(2))
        .await
        .unwrap();

    for coverage in [&views.coverage, &capital.coverage] {
        assert_eq!(coverage.imported_position_count(), 1);
        assert_eq!(coverage.participating_position_count(), 1);
        assert_eq!(coverage.missing_critical_field_record_count(), 0);
        assert!(coverage.source_confidence().is_none());
        assert_eq!(coverage.distinct_external_data_source_version_count(), 0);
    }
    assert_eq!(capital.total_capital_requirement.coefficient(), "3");
    assert_ne!(views.content_hash, *snapshot.content_hash());
    assert_ne!(capital.content_hash, *snapshot.content_hash());
}

fn snapshot() -> PositionSnapshot {
    snapshot_with_classification(AccountingClassificationState::Unknown)
}

fn snapshot_with_classification(state: AccountingClassificationState) -> PositionSnapshot {
    let unit = UnitRef::new(id('M'), Version::new(1).unwrap());
    let decimal = |value| DecimalValue::new(value, 0, unit.clone()).unwrap();
    let position = Position::new(PositionInput {
        position_id: id('P'),
        instrument_ref: VersionRef::new(id('J'), Version::new(1).unwrap()),
        quantity: decimal("1"),
        economic_value: decimal("100"),
        economic_pnl: decimal("2"),
        accounting_pnl: decimal("1"),
        capital_requirement: decimal("3"),
        accounting_classification: AccountingClassification::new(
            state,
            matches!(state, AccountingClassificationState::Classified)
                .then_some(ficant_domain::research::AccountingBook::Ac),
        )
        .unwrap(),
        holding_form: PositionHoldingForm::ReverseRepoCollateral,
    })
    .unwrap();
    let mut input = PositionSnapshotInput {
        snapshot_id: id('S'),
        owner: OwnerRef::new(id('T'), id('N')),
        subject_ref: VersionRef::new(id('V'), Version::new(1).unwrap()),
        observed_at: time(1),
        visible_at: time(1),
        content_hash: ContentHash::digest(b"x"),
        lineage: vec![LineageRef::content_addressed(
            id('K'),
            ContentHash::digest(b"l"),
        )],
        positions: vec![position],
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    PositionSnapshot::new(input).unwrap()
}
fn time(hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(2026, 7, 31, hour, 0, 0).unwrap(),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(),
    )
    .unwrap()
}
fn id(value: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{value}")).unwrap()
}
