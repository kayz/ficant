use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, AeadCursorCodec, ApplicationResult, AuthorizedPrincipal, CursorKey,
    ExactCatalogRead, NormalizedPortfolioContextResolution, PORTFOLIO_READ_SCOPE,
    PortfolioAnalyticsAuthorityCandidate, PortfolioAnalyticsAuthorityQuery,
    PortfolioAnalyticsEvidenceBinding, PortfolioAnalyticsEvidenceKind,
    PortfolioBondRatesAuthorityCandidate, PortfolioCatalogEvidenceBinding, PortfolioCatalogFilter,
    PortfolioCatalogRepository, PortfolioCatalogSnapshot, PortfolioCatalogTemporalScope,
    PortfolioContextInput, PortfolioCurrencyMode, PortfolioImmutableSnapshotAuthority,
    PortfolioLookThroughMode, PortfolioPeriodPreset, PortfolioRatesUnitRole,
    PortfolioRiskAuthority, PortfolioScopeAuthority, PortfolioScopeSelector,
    PortfolioUnitAuthorityBinding, PortfolioValuationAuthorityBinding,
    ResolvedPortfolioAnalyticsAuthority, VisibleCatalogRecord,
};
use ficant_application::{
    ApplicationErrorCategory, ListPortfolioCatalog, ListPortfolioCatalogCommand,
};
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, CalendarRequirement, FixedDecimal,
};
use ficant_domain::governance::PlatformRole;
use ficant_domain::portfolio::{
    Benchmark, BenchmarkInput, BenchmarkRef, Book, BookInput, Portfolio, PortfolioDecimalRounding,
    PortfolioGroup, PortfolioGroupInput, PortfolioInput, PortfolioMetricConvention,
    PortfolioMetricConventionInput, PortfolioMetricConventionRef, PortfolioMetricWeighting,
    PortfolioSnapshotBinding, PortfolioStatus,
};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};

#[test]
fn analytics_authority_hash_binds_units_scenario_and_remaining_years() {
    let candidate = analytics_authority_candidate();
    let baseline = candidate.canonical_content_hash();

    let mut reordered = candidate.clone();
    reordered.units.reverse();
    assert_eq!(reordered.canonical_content_hash(), baseline);

    let mut remaining_index_drift = candidate.clone();
    remaining_index_drift.bond_rates[0].remaining_years_value_index = 2;
    assert_ne!(remaining_index_drift.canonical_content_hash(), baseline);

    let mut remaining_value_drift = candidate.clone();
    remaining_value_drift.bond_rates[0].remaining_years =
        FixedDecimal::from_scaled(8_000_000_000_000);
    assert_ne!(remaining_value_drift.canonical_content_hash(), baseline);

    let mut unit_hash_drift = candidate.clone();
    unit_hash_drift.units[0].content_hash = ContentHash::digest(b"unit-drift");
    assert_ne!(unit_hash_drift.canonical_content_hash(), baseline);

    let query = PortfolioAnalyticsAuthorityQuery::new(
        owner(),
        subject(),
        PortfolioSnapshotBinding::new(
            id(5),
            ContentHash::digest(b"positions"),
            time(21, 2),
            time(21, 3),
        )
        .unwrap(),
        time(21, 3),
        time(21, 2),
    );
    assert_eq!(
        query.unwrap_err().category(),
        ApplicationErrorCategory::ValidationFailed
    );
}

#[test]
fn analytics_evidence_times_bind_the_request_and_conflicting_identity_fails() {
    let baseline = PortfolioAnalyticsEvidenceBinding {
        kind: PortfolioAnalyticsEvidenceKind::PositionSnapshot,
        object_id: id(5),
        version: None,
        content_hash: ContentHash::digest(b"positions"),
        observed_at: Some(time(21, 2)),
        visible_at: Some(time(21, 3)),
        effective_from: None,
        effective_to: None,
    };
    let baseline_authority = resolved_authority(vec![baseline.clone()]).unwrap();

    let mut observed_drift = baseline.clone();
    observed_drift.observed_at = Some(time(21, 1));
    let drifted_authority = resolved_authority(vec![observed_drift.clone()]).unwrap();
    assert_ne!(
        baseline_authority.request_fingerprint,
        drifted_authority.request_fingerprint
    );

    let mut hash_drift = baseline.clone();
    hash_drift.content_hash = ContentHash::digest(b"positions-drift");
    let hash_drifted_authority = resolved_authority(vec![hash_drift.clone()]).unwrap();
    assert_ne!(
        baseline_authority.request_fingerprint,
        hash_drifted_authority.request_fingerprint
    );

    let error = resolved_authority(vec![baseline.clone(), observed_drift]).unwrap_err();
    assert_eq!(
        error.category(),
        ApplicationErrorCategory::LineageIncomplete
    );
    let hash_error = resolved_authority(vec![baseline, hash_drift]).unwrap_err();
    assert_eq!(
        hash_error.category(),
        ApplicationErrorCategory::LineageIncomplete
    );
}

fn resolved_authority(
    evidence: Vec<PortfolioAnalyticsEvidenceBinding>,
) -> ApplicationResult<ResolvedPortfolioAnalyticsAuthority> {
    ResolvedPortfolioAnalyticsAuthority::new(
        id(26),
        &ContentHash::digest(b"authority"),
        PortfolioRiskAuthority {
            curve_snapshot_id: id(27),
            dv01_unit: UnitRef::new(id(20), version()),
            futures_data_snapshot_id: None,
        },
        Vec::new(),
        Vec::new(),
        evidence,
    )
}

fn analytics_authority_candidate() -> PortfolioAnalyticsAuthorityCandidate {
    let roles = [
        PortfolioRatesUnitRole::CurrencyAmount,
        PortfolioRatesUnitRole::PricePer100,
        PortfolioRatesUnitRole::Rate,
        PortfolioRatesUnitRole::Years,
        PortfolioRatesUnitRole::YearsSquared,
        PortfolioRatesUnitRole::Dv01Per100,
        PortfolioRatesUnitRole::Dv01,
        PortfolioRatesUnitRole::Dimensionless,
        PortfolioRatesUnitRole::ContractCount,
    ];
    let units = roles
        .into_iter()
        .enumerate()
        .map(|(offset, role)| PortfolioUnitAuthorityBinding {
            role,
            reference: UnitRef::new(id(14 + offset), version()),
            content_hash: ContentHash::digest(format!("unit-{role:?}").as_bytes()),
        })
        .collect();
    let bond_rates = vec![PortfolioBondRatesAuthorityCandidate {
        position_id: id(24),
        instrument_ref: version_ref(25),
        valuation: PortfolioValuationAuthorityBinding {
            valuation_id: id(23),
            source_revision: 1,
            content_hash: ContentHash::digest(b"valuation"),
            value_index: 0,
        },
        remaining_years_value_index: 1,
        mode: AnalyticsMode::PriceIn,
        input_value: FixedDecimal::from_scaled(101_230_000_000_000),
        remaining_years: FixedDecimal::from_scaled(7_350_000_000_000),
        settlement_date: NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
        calendar_requirement: CalendarRequirement::ExactMarket,
    }];

    PortfolioAnalyticsAuthorityCandidate {
        authority_set_id: id(26),
        owner: owner(),
        subject_ref: subject(),
        position_snapshot: PortfolioImmutableSnapshotAuthority {
            id: id(5),
            content_hash: ContentHash::digest(b"positions"),
        },
        curve_snapshot: PortfolioImmutableSnapshotAuthority {
            id: id(27),
            content_hash: ContentHash::digest(b"curve"),
        },
        data_snapshot: PortfolioImmutableSnapshotAuthority {
            id: id(28),
            content_hash: ContentHash::digest(b"data"),
        },
        futures_data_snapshot: None,
        tax_rule_pack: AnalyticsObjectRef::new(
            version_ref(29),
            ContentHash::digest(b"tax-rule-pack"),
        ),
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        visible_at: time(21, 3),
        units,
        bond_rates,
        content_hash: ContentHash::digest(b"placeholder"),
    }
}

#[tokio::test]
async fn catalog_pages_in_frozen_business_order_and_rejects_filter_drift() {
    let repository = FakeRepository::new(fixture());
    let codec = codec();
    let use_case = ListPortfolioCatalog::new(&repository, &codec);
    let principal = principal(&owner(), vec![PORTFOLIO_READ_SCOPE]);
    let active_filter = filter(None, vec![PortfolioStatus::Active]);

    let first = use_case
        .execute(
            &principal,
            ListPortfolioCatalogCommand::new(active_filter.clone(), None, 1).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.portfolios().len(), 1);
    assert_eq!(first.portfolios()[0].record().value().code(), "ALPHA");
    assert_eq!(first.books().len(), 1);
    assert_eq!(first.groups().len(), 1);
    let cursor = first.next_cursor().unwrap().to_owned();

    let second = use_case
        .execute(
            &principal,
            ListPortfolioCatalogCommand::new(active_filter, Some(cursor.clone()), 1).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.portfolios().len(), 1);
    assert_eq!(second.portfolios()[0].record().value().code(), "ZETA");
    assert!(second.next_cursor().is_none());

    let calls_before_drift = repository.calls.load(Ordering::SeqCst);
    let drifted = filter(Some("alpha"), vec![PortfolioStatus::Active]);
    let error = use_case
        .execute(
            &principal,
            ListPortfolioCatalogCommand::new(drifted, Some(cursor), 1).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::Forbidden);
    assert_eq!(repository.calls.load(Ordering::SeqCst), calls_before_drift);
}

#[tokio::test]
async fn authorization_and_knowledge_drift_fail_before_exposing_catalog() {
    let repository = FakeRepository::new(fixture());
    let codec = codec();
    let use_case = ListPortfolioCatalog::new(&repository, &codec);
    let unauthorized_owner = OwnerRef::new(owner().tenant_id().clone(), id(14));
    let unauthorized = principal(&unauthorized_owner, vec![PORTFOLIO_READ_SCOPE]);
    let error = use_case
        .execute(
            &unauthorized,
            ListPortfolioCatalogCommand::new(filter(None, vec![]), None, 10).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::Forbidden);
    assert_eq!(repository.calls.load(Ordering::SeqCst), 0);

    let original = fixture();
    let future = PortfolioCatalogSnapshot::new(
        original.books().to_vec(),
        original.groups().to_vec(),
        original
            .portfolios()
            .iter()
            .cloned()
            .map(|record| VisibleCatalogRecord::new(record.into_value(), time(21, 6)))
            .collect(),
        original.benchmarks().to_vec(),
        original.metric_conventions().to_vec(),
    );
    let future_repository = FakeRepository::new(future);
    let future_case = ListPortfolioCatalog::new(&future_repository, &codec);
    let error = future_case
        .execute(
            &principal(&owner(), vec![PORTFOLIO_READ_SCOPE]),
            ListPortfolioCatalogCommand::new(filter(None, vec![]), None, 10).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);
}

#[tokio::test]
async fn normalized_context_changes_member_scope_and_period_without_provenance_only_drift() {
    let repository = FakeRepository::new(fixture());
    let codec = codec();
    let use_case = ListPortfolioCatalog::new(&repository, &codec);
    let researcher = principal(&owner(), vec![PORTFOLIO_READ_SCOPE]);
    let input = PortfolioContextInput {
        scope: PortfolioScopeSelector::Book(id(3)),
        valuation_at: time(21, 3),
        knowledge_at: time(21, 5),
        currency: PortfolioCurrencyMode::Cny,
        look_through: PortfolioLookThroughMode::Consolidated,
        benchmark_id: id(7),
        period: PortfolioPeriodPreset::SevenDays,
    };
    let resolution = use_case
        .normalize_context_with_evidence(&researcher, owner(), subject(), input.clone())
        .await
        .unwrap();
    let normalized = resolution.context();
    assert_eq!(normalized.scope.member_portfolios().len(), 2);
    assert_eq!(resolution.catalog_evidence().len(), 5);
    assert_eq!(
        normalized.period_to.instant() - normalized.period_from.instant(),
        chrono::Duration::days(7)
    );

    let one_day = use_case
        .normalize_context(
            &researcher,
            owner(),
            subject(),
            PortfolioContextInput {
                period: PortfolioPeriodPreset::OneDay,
                ..input
            },
        )
        .await
        .unwrap();
    assert_eq!(
        one_day.period_to.instant() - one_day.period_from.instant(),
        chrono::Duration::days(1)
    );
    assert_ne!(one_day.period_from, normalized.period_from);

    let resolved = use_case
        .resolve_aggregation_inputs_with_evidence(researcher.access_scope(), &resolution)
        .await
        .unwrap();
    assert_eq!(resolved.portfolios.len(), 2);
    assert_eq!(resolved.benchmark.value().reference().id(), &id(7));
    assert_eq!(
        resolved.benchmark_snapshot.content_hash(),
        resolved
            .benchmark
            .value()
            .position_snapshot()
            .content_hash()
    );
}

#[tokio::test]
async fn catalog_evidence_rejects_hash_subsecond_timezone_and_post_read_drift() {
    let codec = codec();
    let researcher = principal(&owner(), vec![PORTFOLIO_READ_SCOPE]);
    let input = PortfolioContextInput {
        scope: PortfolioScopeSelector::Book(id(3)),
        valuation_at: time(21, 3),
        knowledge_at: time(21, 5),
        currency: PortfolioCurrencyMode::Cny,
        look_through: PortfolioLookThroughMode::Consolidated,
        benchmark_id: id(7),
        period: PortfolioPeriodPreset::SevenDays,
    };
    let repository = FakeRepository::new(fixture());
    let use_case = ListPortfolioCatalog::new(&repository, &codec);
    let resolution = use_case
        .normalize_context_with_evidence(&researcher, owner(), subject(), input.clone())
        .await
        .unwrap();
    use_case
        .resolve_aggregation_inputs_with_evidence(researcher.access_scope(), &resolution)
        .await
        .unwrap();

    let baseline = resolution.catalog_evidence()[0].clone();
    let subsecond_visible = MarketTime::new(
        baseline.visible_at().instant() + chrono::Duration::nanoseconds(1),
        baseline.visible_at().market_timezone(),
        baseline.visible_at().local_trading_date(),
    )
    .unwrap();
    let instant = baseline.visible_at().instant();
    let timezone = chrono_tz::America::Los_Angeles;
    let timezone_visible = MarketTime::new(
        instant,
        "America/Los_Angeles",
        instant.with_timezone(&timezone).date_naive(),
    )
    .unwrap();
    let variants = [
        PortfolioCatalogEvidenceBinding::new(
            baseline.role(),
            baseline.reference().clone(),
            baseline.content_hash().clone(),
            subsecond_visible,
            baseline.effective_from().clone(),
            baseline.effective_to().clone(),
        )
        .unwrap(),
        PortfolioCatalogEvidenceBinding::new(
            baseline.role(),
            baseline.reference().clone(),
            baseline.content_hash().clone(),
            timezone_visible,
            baseline.effective_from().clone(),
            baseline.effective_to().clone(),
        )
        .unwrap(),
        PortfolioCatalogEvidenceBinding::new(
            baseline.role(),
            baseline.reference().clone(),
            ContentHash::digest(b"catalog-hash-drift"),
            baseline.visible_at().clone(),
            baseline.effective_from().clone(),
            baseline.effective_to().clone(),
        )
        .unwrap(),
    ];
    for drifted in variants {
        let mut evidence = resolution.catalog_evidence().to_vec();
        evidence[0] = drifted;
        let drifted_resolution =
            NormalizedPortfolioContextResolution::new(resolution.context().clone(), evidence)
                .unwrap();
        let error = use_case
            .resolve_aggregation_inputs_with_evidence(
                researcher.access_scope(),
                &drifted_resolution,
            )
            .await
            .unwrap_err();
        assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);
    }

    let original_visible = time(21, 3);
    let original_instant = original_visible.instant();
    let post_read_visible = MarketTime::new(
        original_instant,
        "America/Los_Angeles",
        original_instant.with_timezone(&timezone).date_naive(),
    )
    .unwrap();
    let drift_repository =
        FakeRepository::with_exact_benchmark_visible_drift(fixture(), post_read_visible);
    let drift_case = ListPortfolioCatalog::new(&drift_repository, &codec);
    let drift_resolution = drift_case
        .normalize_context_with_evidence(&researcher, owner(), subject(), input)
        .await
        .unwrap();
    let error = drift_case
        .resolve_aggregation_inputs_with_evidence(researcher.access_scope(), &drift_resolution)
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::HashMismatch);
}

#[tokio::test]
async fn default_context_is_first_active_portfolio_and_requires_researcher_scope() {
    let repository = FakeRepository::new(fixture());
    let codec = codec();
    let use_case = ListPortfolioCatalog::new(&repository, &codec);
    let researcher = principal(&owner(), vec![PORTFOLIO_READ_SCOPE]);
    let context = use_case
        .get_default_context(&researcher, owner(), subject(), time(21, 5))
        .await
        .unwrap();
    let member = context.scope.member_portfolios().first().unwrap();
    assert_eq!(member.object_id(), &id(9), "ALPHA is first in frozen order");

    let no_scope = principal(&owner(), vec![]);
    let error = use_case
        .get_default_context(&no_scope, owner(), subject(), time(21, 5))
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::Forbidden);
}

#[tokio::test]
async fn scope_authority_is_unique_authorized_and_bitemporal() {
    let codec = codec();
    let researcher = principal(&owner(), vec![PORTFOLIO_READ_SCOPE]);
    let repository = FakeRepository::new(fixture());
    let use_case = ListPortfolioCatalog::new(&repository, &codec);
    let authority = use_case
        .resolve_scope_authority(
            &researcher,
            &PortfolioScopeSelector::Portfolio(id(9)),
            &time(21, 3),
            &time(21, 5),
        )
        .await
        .unwrap();
    assert_eq!(authority.owner(), &owner());
    assert_eq!(authority.subject_ref(), &subject());

    let calls = repository.calls.load(Ordering::SeqCst);
    let error = use_case
        .resolve_scope_authority(
            &researcher,
            &PortfolioScopeSelector::Portfolio(id(9)),
            &time(21, 5),
            &time(21, 3),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::ValidationFailed);
    assert_eq!(repository.calls.load(Ordering::SeqCst), calls);

    let error = use_case
        .resolve_scope_authority(
            &researcher,
            &PortfolioScopeSelector::Portfolio(id(9)),
            &time(21, 2),
            &time(21, 2),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::NotFound);

    let unauthorized = PortfolioScopeAuthority::new(
        OwnerRef::new(owner().tenant_id().clone(), id(14)),
        subject(),
    );
    let repository = FakeRepository::with_authorities(fixture(), vec![unauthorized]);
    let use_case = ListPortfolioCatalog::new(&repository, &codec);
    let error = use_case
        .resolve_scope_authority(
            &researcher,
            &PortfolioScopeSelector::Portfolio(id(9)),
            &time(21, 3),
            &time(21, 5),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::Forbidden);

    let authority = PortfolioScopeAuthority::new(owner(), subject());
    let repository =
        FakeRepository::with_authorities(fixture(), vec![authority.clone(), authority]);
    let use_case = ListPortfolioCatalog::new(&repository, &codec);
    let error = use_case
        .resolve_scope_authority(
            &researcher,
            &PortfolioScopeSelector::Portfolio(id(9)),
            &time(21, 3),
            &time(21, 5),
        )
        .await
        .unwrap_err();
    assert_eq!(error.category(), ApplicationErrorCategory::StateConflict);
}

struct FakeRepository {
    snapshot: PortfolioCatalogSnapshot,
    authorities: Option<Vec<PortfolioScopeAuthority>>,
    exact_benchmark_visible_at: Option<MarketTime>,
    calls: AtomicUsize,
}

impl FakeRepository {
    fn new(snapshot: PortfolioCatalogSnapshot) -> Self {
        Self {
            snapshot,
            authorities: None,
            exact_benchmark_visible_at: None,
            calls: AtomicUsize::new(0),
        }
    }

    fn with_authorities(
        snapshot: PortfolioCatalogSnapshot,
        authorities: Vec<PortfolioScopeAuthority>,
    ) -> Self {
        Self {
            snapshot,
            authorities: Some(authorities),
            exact_benchmark_visible_at: None,
            calls: AtomicUsize::new(0),
        }
    }

    fn with_exact_benchmark_visible_drift(
        snapshot: PortfolioCatalogSnapshot,
        visible_at: MarketTime,
    ) -> Self {
        Self {
            snapshot,
            authorities: None,
            exact_benchmark_visible_at: Some(visible_at),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl PortfolioCatalogRepository for FakeRepository {
    async fn find_scope_authorities(
        &self,
        scope: &AccessScope,
        selector: &PortfolioScopeSelector,
        valuation_at: &MarketTime,
        knowledge_at: &MarketTime,
    ) -> ApplicationResult<Vec<PortfolioScopeAuthority>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(authorities) = &self.authorities {
            return Ok(authorities.clone());
        }
        let result = match selector {
            PortfolioScopeSelector::Book(id) => self
                .snapshot
                .books()
                .iter()
                .filter(|record| record.value().reference().id() == id)
                .filter_map(|record| {
                    visible_authority(
                        scope,
                        record.value().owner(),
                        record.value().subject_ref(),
                        record.value().effective_from(),
                        record.value().effective_to(),
                        record.visible_at(),
                        (valuation_at, knowledge_at),
                    )
                })
                .collect(),
            PortfolioScopeSelector::Group(id) => self
                .snapshot
                .groups()
                .iter()
                .filter(|record| record.value().reference().id() == id)
                .filter_map(|record| {
                    visible_authority(
                        scope,
                        record.value().owner(),
                        record.value().subject_ref(),
                        record.value().effective_from(),
                        record.value().effective_to(),
                        record.visible_at(),
                        (valuation_at, knowledge_at),
                    )
                })
                .collect(),
            PortfolioScopeSelector::Portfolio(id) => self
                .snapshot
                .portfolios()
                .iter()
                .filter(|record| record.value().reference().id() == id)
                .filter_map(|record| {
                    visible_authority(
                        scope,
                        record.value().owner(),
                        record.value().subject_ref(),
                        record.value().effective_from(),
                        record.value().effective_to(),
                        record.visible_at(),
                        (valuation_at, knowledge_at),
                    )
                })
                .collect(),
        };
        Ok(result)
    }

    async fn read_catalog_snapshot(
        &self,
        _scope: &AccessScope,
        _temporal: &PortfolioCatalogTemporalScope,
    ) -> ApplicationResult<PortfolioCatalogSnapshot> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.snapshot.clone())
    }

    async fn read_book_exact(
        &self,
        _scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<Book>>> {
        Ok(exact(self.snapshot.books(), read, Book::subject_ref))
    }

    async fn read_group_exact(
        &self,
        _scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<PortfolioGroup>>> {
        Ok(exact(
            self.snapshot.groups(),
            read,
            PortfolioGroup::subject_ref,
        ))
    }

    async fn read_portfolio_exact(
        &self,
        _scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<Portfolio>>> {
        Ok(exact(
            self.snapshot.portfolios(),
            read,
            Portfolio::subject_ref,
        ))
    }

    async fn read_benchmark_exact(
        &self,
        _scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<Benchmark>>> {
        Ok(
            exact(self.snapshot.benchmarks(), read, Benchmark::subject_ref).map(|record| {
                self.exact_benchmark_visible_at
                    .as_ref()
                    .map_or(record.clone(), |visible_at| {
                        VisibleCatalogRecord::new(record.into_value(), visible_at.clone())
                    })
            }),
        )
    }

    async fn read_metric_convention_exact(
        &self,
        _scope: &AccessScope,
        read: &ExactCatalogRead,
    ) -> ApplicationResult<Option<VisibleCatalogRecord<PortfolioMetricConvention>>> {
        Ok(self
            .snapshot
            .metric_conventions()
            .iter()
            .find(|record| {
                record.value().reference() == read.reference()
                    && record.value().content_hash() == read.content_hash()
                    && record.value().owner() == read.temporal().owner()
            })
            .cloned())
    }

    async fn resolve_currency_unit(
        &self,
        _scope: &AccessScope,
        owner: &OwnerRef,
        currency_code: &str,
    ) -> ApplicationResult<Option<UnitRef>> {
        assert_eq!(owner, &crate_owner());
        assert_eq!(currency_code, "CNY");
        Ok(Some(UnitRef::new(id(13), version())))
    }
}

fn visible_authority(
    scope: &AccessScope,
    owner: &OwnerRef,
    subject_ref: &VersionRef,
    effective_from: &MarketTime,
    effective_to: &MarketTime,
    visible_at: &MarketTime,
    boundary: (&MarketTime, &MarketTime),
) -> Option<PortfolioScopeAuthority> {
    let (valuation_at, knowledge_at) = boundary;
    (scope.allows(owner)
        && effective_from.instant() <= valuation_at.instant()
        && valuation_at.instant() < effective_to.instant()
        && visible_at.instant() <= knowledge_at.instant())
    .then(|| PortfolioScopeAuthority::new(owner.clone(), subject_ref.clone()))
}

fn exact<T, F>(
    records: &[VisibleCatalogRecord<T>],
    read: &ExactCatalogRead,
    subject: F,
) -> Option<VisibleCatalogRecord<T>>
where
    T: Clone + ContentAddressed + VersionedDefinition,
    F: Fn(&T) -> &VersionRef,
{
    records
        .iter()
        .find(|record| {
            record.value().identity() == read.reference().id().as_str()
                && record.value().version() == read.reference().version().get()
                && record.value().content_hash() == read.content_hash()
                && subject(record.value()) == read.temporal().subject_ref()
        })
        .cloned()
}

fn fixture() -> PortfolioCatalogSnapshot {
    let book = fixture_book();
    let book_ref = lineage(&book);
    let group = fixture_group(book_ref.clone());
    let group_ref = lineage(&group);
    let benchmark = fixture_benchmark();
    let convention = fixture_convention();
    let alpha = fixture_portfolio(
        9,
        "ALPHA",
        fixture_snapshot(5, b"positions-a"),
        &book_ref,
        &group_ref,
        &benchmark,
        &convention,
    );
    let zeta = fixture_portfolio(
        10,
        "ZETA",
        fixture_snapshot(11, b"positions-z"),
        &book_ref,
        &group_ref,
        &benchmark,
        &convention,
    );
    let visible = time(21, 3);
    PortfolioCatalogSnapshot::new(
        vec![VisibleCatalogRecord::new(book, visible.clone())],
        vec![VisibleCatalogRecord::new(group, visible.clone())],
        vec![
            VisibleCatalogRecord::new(zeta, visible.clone()),
            VisibleCatalogRecord::new(alpha, visible.clone()),
        ],
        vec![VisibleCatalogRecord::new(benchmark, visible.clone())],
        vec![VisibleCatalogRecord::new(convention, visible)],
    )
}

fn fixture_book() -> Book {
    let mut book_input = BookInput {
        book: version_ref(3),
        owner: owner(),
        subject_ref: subject(),
        code: "BOOK-CGB".to_owned(),
        display_name: "CGB Book".to_owned(),
        status: PortfolioStatus::Active,
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    book_input.content_hash = Book::content_hash_for(&book_input);
    Book::new(book_input).unwrap()
}

fn fixture_group(book: LineageRef) -> PortfolioGroup {
    let mut group_input = PortfolioGroupInput {
        group: version_ref(4),
        owner: owner(),
        subject_ref: subject(),
        book,
        parent_group: None,
        code: "GOV".to_owned(),
        display_name: "Government".to_owned(),
        status: PortfolioStatus::Active,
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    group_input.content_hash = PortfolioGroup::content_hash_for(&group_input);
    PortfolioGroup::new(group_input).unwrap()
}

fn fixture_snapshot(identity: usize, payload: &[u8]) -> PortfolioSnapshotBinding {
    PortfolioSnapshotBinding::new(
        id(identity),
        ContentHash::digest(payload),
        time(21, 2),
        time(21, 3),
    )
    .unwrap()
}

fn fixture_benchmark() -> Benchmark {
    let benchmark_snapshot = PortfolioSnapshotBinding::new(
        id(6),
        ContentHash::digest(b"benchmark-positions"),
        time(21, 2),
        time(21, 3),
    )
    .unwrap();
    let mut benchmark_input = BenchmarkInput {
        benchmark: version_ref(7),
        owner: owner(),
        subject_ref: subject(),
        code: "CGB-BENCH".to_owned(),
        display_name: "CGB Benchmark".to_owned(),
        position_snapshot: benchmark_snapshot,
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    benchmark_input.content_hash = Benchmark::content_hash_for(&benchmark_input);
    Benchmark::new(benchmark_input).unwrap()
}

fn fixture_convention() -> PortfolioMetricConvention {
    let mut convention_input = PortfolioMetricConventionInput {
        convention: version_ref(8),
        owner: owner(),
        schema_id: "ficant.portfolio-metric-convention.v1".to_owned(),
        ytm_weighting: PortfolioMetricWeighting::MarketValueTimesModifiedDuration,
        duration_weighting: PortfolioMetricWeighting::MarketValue,
        convexity_weighting: PortfolioMetricWeighting::MarketValue,
        coupon_weighting: PortfolioMetricWeighting::Notional,
        remaining_life_weighting: PortfolioMetricWeighting::Notional,
        rounding: PortfolioDecimalRounding::TiesToEven,
        freshness_limit_seconds: 86_400,
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    convention_input.content_hash = PortfolioMetricConvention::content_hash_for(&convention_input);
    PortfolioMetricConvention::new(convention_input).unwrap()
}

fn fixture_portfolio(
    identity: usize,
    code: &str,
    snapshot: PortfolioSnapshotBinding,
    book: &LineageRef,
    group: &LineageRef,
    benchmark: &Benchmark,
    convention: &PortfolioMetricConvention,
) -> Portfolio {
    let mut input = PortfolioInput {
        portfolio: version_ref(identity),
        owner: owner(),
        subject_ref: subject(),
        book: book.clone(),
        group: group.clone(),
        code: code.to_owned(),
        display_name: format!("{code} Portfolio"),
        status: PortfolioStatus::Active,
        position_snapshot: snapshot,
        benchmark: BenchmarkRef::new(
            benchmark.reference().clone(),
            benchmark.content_hash().clone(),
        ),
        metric_convention: PortfolioMetricConventionRef::new(
            convention.reference().clone(),
            convention.content_hash().clone(),
        ),
        effective_from: time(20, 0),
        effective_to: time(22, 0),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = Portfolio::content_hash_for(&input);
    Portfolio::new(input).unwrap()
}

fn filter(search: Option<&str>, statuses: Vec<PortfolioStatus>) -> PortfolioCatalogFilter {
    PortfolioCatalogFilter::new(
        PortfolioCatalogTemporalScope::new(owner(), subject(), time(21, 3), time(21, 5)).unwrap(),
        statuses,
        search.map(str::to_owned),
    )
    .unwrap()
}

fn principal(owner: &OwnerRef, scopes: Vec<&str>) -> AuthorizedPrincipal {
    AuthorizedPrincipal::new(
        "researcher@example.test".to_owned(),
        id(12),
        owner.tenant_id().clone(),
        vec![owner.owner_id().clone()],
        PlatformRole::Researcher,
        scopes.into_iter().map(str::to_owned).collect(),
        ContentHash::digest(b"credential"),
    )
    .unwrap()
}

fn codec() -> AeadCursorCodec {
    AeadCursorCodec::new(CursorKey::new("r8a-test", [9_u8; 32]).unwrap(), vec![]).unwrap()
}

fn lineage<T>(value: &T) -> LineageRef
where
    T: ContentAddressed + VersionedDefinition,
{
    LineageRef::new(
        Ulid::new(value.identity().to_owned()).unwrap(),
        Some(Version::new(value.version()).unwrap()),
        Some(value.content_hash().clone()),
    )
    .unwrap()
}

fn time(day: u32, hour: u32) -> MarketTime {
    let instant = Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0).unwrap();
    let local = instant
        .with_timezone(&chrono_tz::Asia::Shanghai)
        .date_naive();
    MarketTime::new(instant, "Asia/Shanghai", local).unwrap()
}

fn crate_owner() -> OwnerRef {
    owner()
}

fn owner() -> OwnerRef {
    OwnerRef::new(id(1), id(2))
}

fn subject() -> VersionRef {
    version_ref(0)
}

fn version_ref(index: usize) -> VersionRef {
    VersionRef::new(id(index), version())
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(index: usize) -> Ulid {
    const SUFFIXES: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let suffix = char::from(SUFFIXES[index]);
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
