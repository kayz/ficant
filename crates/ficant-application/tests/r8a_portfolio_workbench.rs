use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, TimeZone, Utc};
use ficant_application::ports::{
    ApplicationResult, AuthorizedPrincipal, Clock, ExactPortfolioScope, ExactPortfolioScopeKind,
    IdGenerator, NormalizedPortfolioContext, NormalizedPortfolioContextResolution,
    PortfolioCatalogEvidenceBinding, PortfolioCatalogEvidenceRole, PortfolioCatalogFilter,
    PortfolioCatalogPage, PortfolioCatalogTemporalScope, PortfolioContextInput,
    PortfolioCurrencyMode, PortfolioLookThroughMode, PortfolioPeriodPreset,
    PortfolioScopeAuthority, PortfolioScopeSelector, SubjectRepository,
};
use ficant_application::use_cases::portfolio_workbench::{
    NonFormalReadEvidence, OwnedPortfolioWorkbenchCatalogEvidenceFactory,
    PORTFOLIO_WORKBENCH_SCHEMA_VERSION, PortfolioCatalogRead, PortfolioDefaultContextResult,
    PortfolioInstrumentRead, PortfolioPageDataMode, PortfolioPageSelection, PortfolioPageState,
    PortfolioWorkbenchCatalogEvidenceFactory, PortfolioWorkbenchContextResolver,
    PortfolioWorkbenchErrorCode, PortfolioWorkbenchPageId, PortfolioWorkbenchPageSource,
    PortfolioWorkbenchSourceError, PortfolioWorkbenchUseCase,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_domain::governance::PlatformRole;
use ficant_domain::portfolio::{BenchmarkRef, PortfolioMetricConventionRef};
use ficant_domain::primitives::{
    ContentHash, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef, Version, VersionRef,
};
use ficant_domain::subject::{
    AccessSet, FundingTier, Subject, SubjectRecord, SubjectStateSnapshot, SubjectVersion,
    TaxTreatment,
};
use ficant_runtime::{
    FormalInputBinding, FormalInputBindingInput, FormalInputKind, FormalInputReference,
};

#[tokio::test]
async fn every_context_dimension_changes_the_normalized_backend_read_and_page_fingerprint() {
    let baseline = baseline_input();
    let resolver = FakeContexts::new(normalize(owner(), subject(), baseline.clone()));
    let pages = FakePages::success(PortfolioPageDataMode::Real);
    let clock = FixedClock(time(22, 1));
    let ids = SequentialIds::default();
    let workbench = PortfolioWorkbenchUseCase::new(&resolver, &pages, &clock, &ids);

    let baseline_page = get_p01(&workbench, baseline.clone()).await;
    assert_eq!(
        baseline_page.schema_version(),
        PORTFOLIO_WORKBENCH_SCHEMA_VERSION
    );
    assert!(baseline_page.projection().is_some());
    assert!(baseline_page.typed_error().is_none());
    let baseline_fingerprint = baseline_page
        .provenance()
        .expect("baseline provenance")
        .request_fingerprint()
        .clone();

    let mut scope = baseline.clone();
    scope.scope = PortfolioScopeSelector::Book(id(8));
    let mut valuation = baseline.clone();
    valuation.valuation_at = time(19, 9);
    let mut knowledge = baseline.clone();
    knowledge.knowledge_at = time(22, 9);
    let mut currency = baseline.clone();
    currency.currency = PortfolioCurrencyMode::Cny;
    let mut look_through = baseline.clone();
    look_through.look_through = PortfolioLookThroughMode::Consolidated;
    let mut benchmark = baseline.clone();
    benchmark.benchmark_id = id(9);
    let mut period = baseline;
    period.period = PortfolioPeriodPreset::SevenDays;

    for input in [
        scope,
        valuation,
        knowledge,
        currency,
        look_through,
        benchmark,
        period,
    ] {
        let page = get_p01(&workbench, input).await;
        assert_ne!(
            page.provenance()
                .expect("variant provenance")
                .request_fingerprint(),
            &baseline_fingerprint
        );
    }

    let reads = pages.contexts.lock().expect("page contexts");
    assert_eq!(reads.len(), 8);
    assert_ne!(reads[1].scope.selected(), reads[0].scope.selected());
    assert_ne!(reads[2].valuation_at, reads[0].valuation_at);
    assert_ne!(reads[3].knowledge_at, reads[0].knowledge_at);
    assert_ne!(reads[4].currency_unit, reads[0].currency_unit);
    assert_ne!(
        reads[5].scope.member_portfolios(),
        reads[0].scope.member_portfolios()
    );
    assert_ne!(reads[6].benchmark, reads[0].benchmark);
    assert_ne!(reads[7].period_from, reads[0].period_from);
}

#[tokio::test]
async fn success_stale_and_typed_stale_failure_are_not_interchangeable() {
    let input = baseline_input();
    let normalized = normalize(owner(), subject(), input.clone());
    let resolver = FakeContexts::new(normalized);
    let clock = FixedClock(time(22, 1));
    let ids = SequentialIds::default();

    let stale_success = FakePages::success(PortfolioPageDataMode::Stale);
    let success_workbench = PortfolioWorkbenchUseCase::new(&resolver, &stale_success, &clock, &ids);
    let success = get_p01(&success_workbench, input.clone()).await;
    assert_eq!(success.data_mode(), PortfolioPageDataMode::Stale);
    assert!(success.projection().is_some());
    assert!(success.typed_error().is_none());
    assert!(success.provenance().is_some());

    let stale_failure =
        FakePages::failure(PortfolioWorkbenchSourceError::Stale { retryable: false });
    let failure_workbench = PortfolioWorkbenchUseCase::new(&resolver, &stale_failure, &clock, &ids);
    let failure = get_p01(&failure_workbench, input).await;
    assert_eq!(failure.data_mode(), PortfolioPageDataMode::Error);
    assert!(failure.projection().is_none());
    assert!(failure.provenance().is_none());
    assert_eq!(
        failure.typed_error().expect("typed stale").code(),
        PortfolioWorkbenchErrorCode::Stale
    );
}

#[tokio::test]
async fn seven_closed_error_categories_never_return_a_success_projection() {
    let cases = [
        (
            PortfolioWorkbenchSourceError::Application(error(
                ApplicationErrorCategory::Unauthenticated,
                false,
            )),
            PortfolioWorkbenchErrorCode::Unauthenticated,
        ),
        (
            PortfolioWorkbenchSourceError::Application(error(
                ApplicationErrorCategory::Forbidden,
                false,
            )),
            PortfolioWorkbenchErrorCode::Forbidden,
        ),
        (
            PortfolioWorkbenchSourceError::Application(error(
                ApplicationErrorCategory::NotFound,
                false,
            )),
            PortfolioWorkbenchErrorCode::NotFound,
        ),
        (
            PortfolioWorkbenchSourceError::Application(error(
                ApplicationErrorCategory::VersionConflict,
                true,
            )),
            PortfolioWorkbenchErrorCode::Conflict,
        ),
        (
            PortfolioWorkbenchSourceError::Stale { retryable: false },
            PortfolioWorkbenchErrorCode::Stale,
        ),
        (
            PortfolioWorkbenchSourceError::Application(error(
                ApplicationErrorCategory::HashMismatch,
                false,
            )),
            PortfolioWorkbenchErrorCode::Integrity,
        ),
        (
            PortfolioWorkbenchSourceError::Application(error(
                ApplicationErrorCategory::StorageUnavailable,
                true,
            )),
            PortfolioWorkbenchErrorCode::Unavailable,
        ),
    ];

    for (source_error, expected) in cases {
        let input = baseline_input();
        let resolver = FakeContexts::new(normalize(owner(), subject(), input.clone()));
        let pages = FakePages::failure(source_error);
        let clock = FixedClock(time(22, 1));
        let ids = SequentialIds::default();
        let workbench = PortfolioWorkbenchUseCase::new(&resolver, &pages, &clock, &ids);
        let page = get_p01(&workbench, input).await;
        let typed = page.typed_error().expect("closed typed error");
        assert_eq!(typed.code(), expected);
        assert_eq!(typed.trace_id().len(), 32);
        assert!(!typed.safe_message().is_empty());
        assert_eq!(page.data_mode(), PortfolioPageDataMode::Error);
        assert_eq!(page.page_state(), PortfolioPageState::Blocked);
        assert!(page.projection().is_none());
        assert!(page.provenance().is_none());
    }
}

#[tokio::test]
async fn mode_evidence_and_selection_invariants_fail_closed_before_backend_handoff() {
    let input = baseline_input();
    let normalized = normalize(owner(), subject(), input.clone());
    let catalog = catalog_page(&normalized);
    let evidence = vec![catalog_evidence(&normalized)];
    let fingerprint = ContentHash::digest(b"catalog-source");

    assert!(
        PortfolioCatalogRead::new(
            catalog.clone(),
            PortfolioPageDataMode::Real,
            PortfolioPageState::Empty,
            evidence.clone(),
            fingerprint.clone(),
        )
        .is_ok()
    );
    assert!(
        PortfolioCatalogRead::new(
            catalog.clone(),
            PortfolioPageDataMode::Stale,
            PortfolioPageState::Empty,
            evidence.clone(),
            fingerprint.clone(),
        )
        .is_ok()
    );
    assert!(
        PortfolioCatalogRead::new(
            catalog.clone(),
            PortfolioPageDataMode::Partial,
            PortfolioPageState::Empty,
            evidence.clone(),
            fingerprint.clone(),
        )
        .is_err()
    );
    assert!(
        PortfolioCatalogRead::new(
            catalog.clone(),
            PortfolioPageDataMode::Error,
            PortfolioPageState::Blocked,
            evidence,
            fingerprint.clone(),
        )
        .is_err()
    );
    assert!(
        PortfolioCatalogRead::new(
            catalog,
            PortfolioPageDataMode::Real,
            PortfolioPageState::Empty,
            Vec::new(),
            fingerprint,
        )
        .is_err()
    );

    let resolver = FakeContexts::new(normalized);
    let pages = FakePages::success(PortfolioPageDataMode::Real);
    let clock = FixedClock(time(22, 1));
    let ids = SequentialIds::default();
    let workbench = PortfolioWorkbenchUseCase::new(&resolver, &pages, &clock, &ids);
    let invalid_p01 = workbench
        .get_page(
            &principal(),
            owner(),
            subject(),
            PortfolioWorkbenchPageId::P01,
            input.clone(),
            Some(PortfolioPageSelection::new(version_ref(12))),
        )
        .await;
    assert_eq!(
        invalid_p01.expect_err("P01 selection must fail").category(),
        ApplicationErrorCategory::ValidationFailed
    );
    let invalid_p04 = workbench
        .get_page(
            &principal(),
            owner(),
            subject(),
            PortfolioWorkbenchPageId::P04,
            input,
            None,
        )
        .await;
    assert_eq!(
        invalid_p04.expect_err("P04 selection required").category(),
        ApplicationErrorCategory::ValidationFailed
    );
    assert_eq!(pages.calls.load(Ordering::SeqCst), 0);
    assert_eq!(resolver.normalize_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn default_context_is_backend_resolved_and_role_failure_is_closed() {
    let normalized = normalize(owner(), subject(), baseline_input());
    let resolver = FakeContexts::new(normalized.clone());
    let pages = FakePages::success(PortfolioPageDataMode::Real);
    let clock = FixedClock(time(22, 1));
    let ids = SequentialIds::default();
    let workbench = PortfolioWorkbenchUseCase::new(&resolver, &pages, &clock, &ids);

    let default_result = workbench
        .get_default_context(&principal(), owner(), subject(), time(22, 1))
        .await
        .expect("default context");
    assert_eq!(
        default_result,
        PortfolioDefaultContextResult::Context(Box::new(normalized))
    );

    let denied = workbench
        .get_default_context(&admin_principal(), owner(), subject(), time(22, 1))
        .await
        .expect("closed default error");
    match denied {
        PortfolioDefaultContextResult::Error(error) => {
            assert_eq!(error.code(), PortfolioWorkbenchErrorCode::Forbidden);
        }
        PortfolioDefaultContextResult::Context(_) => panic!("admin must not receive context"),
    }
    assert_eq!(resolver.default_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn selector_authority_zero_ambiguous_and_unlisted_stop_before_normalization() {
    for category in [
        ApplicationErrorCategory::NotFound,
        ApplicationErrorCategory::StateConflict,
        ApplicationErrorCategory::Forbidden,
    ] {
        let input = baseline_input();
        let resolver =
            FakeContexts::with_scope_error(normalize(owner(), subject(), input.clone()), category);
        let pages = FakePages::success(PortfolioPageDataMode::Real);
        let clock = FixedClock(time(22, 1));
        let ids = SequentialIds::default();
        let workbench = PortfolioWorkbenchUseCase::new(&resolver, &pages, &clock, &ids);

        let error = workbench
            .get_page_for_selector(&principal(), PortfolioWorkbenchPageId::P01, input, None)
            .await
            .expect_err("scope authority must fail closed");

        assert_eq!(error.category(), category);
        assert_eq!(resolver.scope_calls.load(Ordering::SeqCst), 1);
        assert_eq!(resolver.normalize_calls.load(Ordering::SeqCst), 0);
        assert_eq!(pages.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn selector_forwards_one_immutable_catalog_resolution_to_the_page_source() {
    let input = baseline_input();
    let resolution = normalized_resolution(normalize(owner(), subject(), input.clone()))
        .expect("normalized resolution");
    let resolver = PresetResolutionContexts {
        resolution: resolution.clone(),
    };
    let pages = FakePages::success(PortfolioPageDataMode::Real);
    let clock = FixedClock(time(22, 1));
    let ids = SequentialIds::default();
    let workbench = PortfolioWorkbenchUseCase::new(&resolver, &pages, &clock, &ids);

    let page = workbench
        .get_page_for_selector(&principal(), PortfolioWorkbenchPageId::P01, input, None)
        .await
        .expect("P01 envelope");

    assert!(page.projection().is_some());
    assert_eq!(
        pages
            .resolutions
            .lock()
            .expect("page resolutions")
            .as_slice(),
        &[resolution]
    );
}

#[tokio::test]
async fn p01_non_formal_evidence_binds_catalog_subsecond_timezone_and_local_date() {
    let context = normalize(owner(), subject(), baseline_input());
    let baseline = normalized_resolution(context.clone()).expect("baseline resolution");
    let factory = OwnedPortfolioWorkbenchCatalogEvidenceFactory::new(Arc::new(SubjectFixture));
    let catalog = catalog_page(&context);
    let baseline_read = factory
        .evidence(&principal(), &baseline, &catalog)
        .await
        .expect("baseline evidence")
        .into_iter()
        .next()
        .expect("one catalog read");

    let selected = baseline_read
        .consumed_inputs()
        .iter()
        .find(|input| input.role() == "normalized_scope_selected")
        .expect("selected catalog input");
    let selected_authority = baseline
        .catalog_evidence()
        .iter()
        .find(|binding| binding.role() == PortfolioCatalogEvidenceRole::SelectedPortfolio)
        .expect("selected catalog authority");
    assert_eq!(selected.visible_at(), Some(selected_authority.visible_at()));
    assert_eq!(
        selected.effective_from(),
        Some(selected_authority.effective_from())
    );
    assert_eq!(
        selected.effective_to(),
        Some(selected_authority.effective_to())
    );

    for drift in [CatalogTimeDrift::Nanosecond, CatalogTimeDrift::Timezone] {
        let variant = drift_catalog_visibility(&baseline, drift);
        let variant_read = factory
            .evidence(&principal(), &variant, &catalog)
            .await
            .expect("drifted evidence")
            .into_iter()
            .next()
            .expect("one drifted read");
        assert_ne!(
            variant_read.request_fingerprint(),
            baseline_read.request_fingerprint()
        );
        let variant_selected = variant_read
            .consumed_inputs()
            .iter()
            .find(|input| input.role() == "normalized_scope_selected")
            .expect("drifted selected input");
        assert_ne!(variant_selected.visible_at(), selected.visible_at());
    }
}

async fn get_p01(
    workbench: &PortfolioWorkbenchUseCase<'_>,
    input: PortfolioContextInput,
) -> ficant_application::use_cases::portfolio_workbench::PortfolioPageEnvelope {
    workbench
        .get_page(
            &principal(),
            owner(),
            subject(),
            PortfolioWorkbenchPageId::P01,
            input,
            None,
        )
        .await
        .expect("P01 envelope")
}

struct FakeContexts {
    default: NormalizedPortfolioContext,
    scope_error: Option<ApplicationError>,
    scope_calls: AtomicUsize,
    normalize_calls: AtomicUsize,
    default_calls: AtomicUsize,
}

impl FakeContexts {
    fn new(default: NormalizedPortfolioContext) -> Self {
        Self {
            default,
            scope_error: None,
            scope_calls: AtomicUsize::new(0),
            normalize_calls: AtomicUsize::new(0),
            default_calls: AtomicUsize::new(0),
        }
    }

    fn with_scope_error(
        default: NormalizedPortfolioContext,
        category: ApplicationErrorCategory,
    ) -> Self {
        Self {
            default,
            scope_error: Some(error(category, false)),
            scope_calls: AtomicUsize::new(0),
            normalize_calls: AtomicUsize::new(0),
            default_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl PortfolioWorkbenchContextResolver for FakeContexts {
    async fn resolve_scope_authority(
        &self,
        _principal: &AuthorizedPrincipal,
        _selector: &PortfolioScopeSelector,
        _valuation_at: &MarketTime,
        _knowledge_at: &MarketTime,
    ) -> ApplicationResult<PortfolioScopeAuthority> {
        self.scope_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = &self.scope_error {
            return Err(error.clone());
        }
        Ok(PortfolioScopeAuthority::new(
            self.default.owner.clone(),
            self.default.subject_ref.clone(),
        ))
    }

    async fn normalize_context(
        &self,
        _principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        input: PortfolioContextInput,
    ) -> ApplicationResult<NormalizedPortfolioContextResolution> {
        self.normalize_calls.fetch_add(1, Ordering::SeqCst);
        normalized_resolution(normalize(owner, subject_ref, input))
    }

    async fn get_default_context(
        &self,
        _principal: &AuthorizedPrincipal,
        _owner: OwnerRef,
        _subject_ref: VersionRef,
        _knowledge_at: MarketTime,
    ) -> ApplicationResult<NormalizedPortfolioContext> {
        self.default_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.default.clone())
    }
}

struct PresetResolutionContexts {
    resolution: NormalizedPortfolioContextResolution,
}

#[async_trait]
impl PortfolioWorkbenchContextResolver for PresetResolutionContexts {
    async fn resolve_scope_authority(
        &self,
        _principal: &AuthorizedPrincipal,
        _selector: &PortfolioScopeSelector,
        _valuation_at: &MarketTime,
        _knowledge_at: &MarketTime,
    ) -> ApplicationResult<PortfolioScopeAuthority> {
        Ok(PortfolioScopeAuthority::new(
            self.resolution.context().owner.clone(),
            self.resolution.context().subject_ref.clone(),
        ))
    }

    async fn normalize_context(
        &self,
        _principal: &AuthorizedPrincipal,
        owner: OwnerRef,
        subject_ref: VersionRef,
        _input: PortfolioContextInput,
    ) -> ApplicationResult<NormalizedPortfolioContextResolution> {
        if self.resolution.context().owner != owner
            || self.resolution.context().subject_ref != subject_ref
        {
            return Err(error(ApplicationErrorCategory::HashMismatch, false));
        }
        Ok(self.resolution.clone())
    }

    async fn get_default_context(
        &self,
        _principal: &AuthorizedPrincipal,
        _owner: OwnerRef,
        _subject_ref: VersionRef,
        _knowledge_at: MarketTime,
    ) -> ApplicationResult<NormalizedPortfolioContext> {
        Ok(self.resolution.context().clone())
    }
}

#[derive(Clone)]
enum CatalogOutcome {
    Success(PortfolioPageDataMode),
    Failure(PortfolioWorkbenchSourceError),
}

struct FakePages {
    outcome: CatalogOutcome,
    calls: AtomicUsize,
    contexts: Mutex<Vec<NormalizedPortfolioContext>>,
    resolutions: Mutex<Vec<NormalizedPortfolioContextResolution>>,
}

impl FakePages {
    fn success(mode: PortfolioPageDataMode) -> Self {
        Self {
            outcome: CatalogOutcome::Success(mode),
            calls: AtomicUsize::new(0),
            contexts: Mutex::new(Vec::new()),
            resolutions: Mutex::new(Vec::new()),
        }
    }

    fn failure(error: PortfolioWorkbenchSourceError) -> Self {
        Self {
            outcome: CatalogOutcome::Failure(error),
            calls: AtomicUsize::new(0),
            contexts: Mutex::new(Vec::new()),
            resolutions: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl PortfolioWorkbenchPageSource for FakePages {
    async fn read_catalog(
        &self,
        _principal: &AuthorizedPrincipal,
        resolution: &NormalizedPortfolioContextResolution,
    ) -> Result<PortfolioCatalogRead, PortfolioWorkbenchSourceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let context = resolution.context();
        self.contexts
            .lock()
            .expect("page contexts")
            .push(context.clone());
        self.resolutions
            .lock()
            .expect("page resolutions")
            .push(resolution.clone());
        match &self.outcome {
            CatalogOutcome::Success(mode) => {
                let catalog = catalog_page(context);
                let source_fingerprint = ContentHash::digest(format!("{resolution:?}").as_bytes());
                Ok(PortfolioCatalogRead::new(
                    catalog,
                    *mode,
                    PortfolioPageState::Empty,
                    vec![catalog_evidence(context)],
                    source_fingerprint,
                )?)
            }
            CatalogOutcome::Failure(error) => Err(error.clone()),
        }
    }

    async fn get_overview(
        &self,
        _principal: &AuthorizedPrincipal,
        _resolution: &NormalizedPortfolioContextResolution,
    ) -> Result<
        ficant_application::use_cases::portfolio_aggregation::PortfolioOverview,
        PortfolioWorkbenchSourceError,
    > {
        Err(error(ApplicationErrorCategory::NotFound, false).into())
    }

    async fn read_instrument(
        &self,
        _principal: &AuthorizedPrincipal,
        _resolution: &NormalizedPortfolioContextResolution,
        _selection: &PortfolioPageSelection,
    ) -> Result<PortfolioInstrumentRead, PortfolioWorkbenchSourceError> {
        Err(error(ApplicationErrorCategory::NotFound, false).into())
    }
}

fn catalog_page(context: &NormalizedPortfolioContext) -> PortfolioCatalogPage {
    let temporal = PortfolioCatalogTemporalScope::new(
        context.owner.clone(),
        context.subject_ref.clone(),
        context.valuation_at.clone(),
        context.knowledge_at.clone(),
    )
    .expect("catalog temporal");
    let filter = PortfolioCatalogFilter::new(temporal, Vec::new(), None).expect("catalog filter");
    PortfolioCatalogPage::new(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        filter.fingerprint().clone(),
    )
}

fn catalog_evidence(context: &NormalizedPortfolioContext) -> NonFormalReadEvidence {
    let subject = FormalInputBinding::new(FormalInputBindingInput {
        role: "subject".to_owned(),
        kind: FormalInputKind::Subject,
        owner: context.owner.clone(),
        reference: FormalInputReference::Object(
            LineageRef::new(
                context.subject_ref.id().clone(),
                Some(context.subject_ref.version()),
                Some(ContentHash::digest(b"subject")),
            )
            .expect("subject lineage"),
        ),
        observed_at: None,
        visible_at: None,
        effective_from: None,
        effective_to: None,
    })
    .expect("subject binding");
    NonFormalReadEvidence::new(
        "ficant.portfolio-catalog-read.v1".to_owned(),
        vec![subject],
        ContentHash::digest(format!("{:?}", context.scope).as_bytes()),
    )
    .expect("catalog evidence")
}

fn normalize(
    owner: OwnerRef,
    subject_ref: VersionRef,
    input: PortfolioContextInput,
) -> NormalizedPortfolioContext {
    let selected = match input.scope {
        PortfolioScopeSelector::Book(value) => ExactPortfolioScopeKind::Book(lineage(value)),
        PortfolioScopeSelector::Group(value) => ExactPortfolioScopeKind::Group(lineage(value)),
        PortfolioScopeSelector::Portfolio(value) => {
            ExactPortfolioScopeKind::Portfolio(lineage(value))
        }
    };
    let mut members = vec![lineage(id(6))];
    if input.look_through != PortfolioLookThroughMode::None {
        members.push(lineage(id(7)));
    }
    let days = match input.period {
        PortfolioPeriodPreset::OneDay => 1,
        PortfolioPeriodPreset::SevenDays => 7,
        PortfolioPeriodPreset::ThirtyDays => 30,
        PortfolioPeriodPreset::YearToDate => 180,
        PortfolioPeriodPreset::OneYear => 365,
    };
    let period_to = input.valuation_at.clone();
    let period_from = shifted_time(&period_to, days);
    let currency_unit = UnitRef::new(
        if input.currency == PortfolioCurrencyMode::Original {
            id(10)
        } else {
            id(11)
        },
        version(),
    );
    NormalizedPortfolioContext {
        owner,
        subject_ref,
        scope: ExactPortfolioScope::new(selected, members),
        valuation_at: input.valuation_at,
        knowledge_at: input.knowledge_at,
        currency: input.currency,
        currency_unit,
        look_through: input.look_through,
        benchmark: BenchmarkRef::new(
            VersionRef::new(input.benchmark_id, version()),
            ContentHash::digest(b"benchmark"),
        ),
        period: input.period,
        period_from,
        period_to,
        metric_convention: PortfolioMetricConventionRef::new(
            version_ref(13),
            ContentHash::digest(b"metric-convention"),
        ),
    }
}

fn normalized_resolution(
    context: NormalizedPortfolioContext,
) -> ApplicationResult<NormalizedPortfolioContextResolution> {
    let visible_at = time(18, 0);
    let effective_from = time(17, 0);
    let effective_to = time(30, 0);
    let (selected_role, selected) = match context.scope.selected() {
        ExactPortfolioScopeKind::Book(reference) => {
            (PortfolioCatalogEvidenceRole::SelectedBook, reference)
        }
        ExactPortfolioScopeKind::Group(reference) => {
            (PortfolioCatalogEvidenceRole::SelectedGroup, reference)
        }
        ExactPortfolioScopeKind::Portfolio(reference) => {
            (PortfolioCatalogEvidenceRole::SelectedPortfolio, reference)
        }
    };
    let mut evidence = vec![catalog_binding_from_lineage(
        selected_role,
        selected,
        &visible_at,
        &effective_from,
        &effective_to,
    )?];
    for member in context.scope.member_portfolios() {
        evidence.push(catalog_binding_from_lineage(
            PortfolioCatalogEvidenceRole::MemberPortfolio,
            member,
            &visible_at,
            &effective_from,
            &effective_to,
        )?);
    }
    evidence.push(PortfolioCatalogEvidenceBinding::new(
        PortfolioCatalogEvidenceRole::Benchmark,
        context.benchmark.reference().clone(),
        context.benchmark.content_hash().clone(),
        visible_at.clone(),
        effective_from.clone(),
        effective_to.clone(),
    )?);
    evidence.push(PortfolioCatalogEvidenceBinding::new(
        PortfolioCatalogEvidenceRole::MetricConvention,
        context.metric_convention.reference().clone(),
        context.metric_convention.content_hash().clone(),
        visible_at,
        effective_from,
        effective_to,
    )?);
    NormalizedPortfolioContextResolution::new(context, evidence)
}

fn catalog_binding_from_lineage(
    role: PortfolioCatalogEvidenceRole,
    reference: &LineageRef,
    visible_at: &MarketTime,
    effective_from: &MarketTime,
    effective_to: &MarketTime,
) -> ApplicationResult<PortfolioCatalogEvidenceBinding> {
    PortfolioCatalogEvidenceBinding::new(
        role,
        VersionRef::new(
            reference.object_id().clone(),
            reference.version().expect("exact catalog version"),
        ),
        reference
            .content_hash()
            .expect("exact catalog content hash")
            .clone(),
        visible_at.clone(),
        effective_from.clone(),
        effective_to.clone(),
    )
}

#[derive(Clone, Copy)]
enum CatalogTimeDrift {
    Nanosecond,
    Timezone,
}

fn drift_catalog_visibility(
    baseline: &NormalizedPortfolioContextResolution,
    drift: CatalogTimeDrift,
) -> NormalizedPortfolioContextResolution {
    let mut evidence = baseline.catalog_evidence().to_vec();
    let selected_index = evidence
        .iter()
        .position(|binding| {
            matches!(
                binding.role(),
                PortfolioCatalogEvidenceRole::SelectedBook
                    | PortfolioCatalogEvidenceRole::SelectedGroup
                    | PortfolioCatalogEvidenceRole::SelectedPortfolio
            )
        })
        .expect("selected evidence");
    let selected = &evidence[selected_index];
    let visible_at = match drift {
        CatalogTimeDrift::Nanosecond => {
            let instant = selected.visible_at().instant() + Duration::nanoseconds(1);
            let local_date = instant
                .with_timezone(&chrono_tz::Asia::Shanghai)
                .date_naive();
            MarketTime::new(instant, "Asia/Shanghai", local_date).expect("nanosecond visibility")
        }
        CatalogTimeDrift::Timezone => {
            let instant = selected.visible_at().instant();
            let local_date = instant
                .with_timezone(&chrono_tz::Pacific::Honolulu)
                .date_naive();
            MarketTime::new(instant, "Pacific/Honolulu", local_date).expect("timezone visibility")
        }
    };
    evidence[selected_index] = PortfolioCatalogEvidenceBinding::new(
        selected.role(),
        selected.reference().clone(),
        selected.content_hash().clone(),
        visible_at,
        selected.effective_from().clone(),
        selected.effective_to().clone(),
    )
    .expect("drifted catalog evidence");
    NormalizedPortfolioContextResolution::new(baseline.context().clone(), evidence)
        .expect("drifted catalog resolution")
}

struct SubjectFixture;

#[async_trait]
impl SubjectRepository for SubjectFixture {
    async fn register_subject(&self, _value: SubjectRecord) -> ApplicationResult<SubjectRecord> {
        panic!("catalog evidence performs exact reads only")
    }

    async fn get_subject(&self, reference: VersionRef) -> ApplicationResult<Option<SubjectRecord>> {
        Ok((reference == subject()).then(subject_record))
    }

    async fn register_subject_state(
        &self,
        _value: SubjectStateSnapshot,
    ) -> ApplicationResult<SubjectStateSnapshot> {
        panic!("catalog evidence performs exact reads only")
    }

    async fn get_subject_state(
        &self,
        _snapshot_id: Ulid,
        _knowledge_at: DateTime<Utc>,
    ) -> ApplicationResult<Option<SubjectStateSnapshot>> {
        panic!("catalog evidence performs exact reads only")
    }
}

fn subject_record() -> SubjectRecord {
    let subject_value = Subject::new_owned(subject().id().clone(), owner(), "Portfolio Subject")
        .expect("owned subject");
    let version_value = SubjectVersion::new(
        subject(),
        AccessSet::new(["CN"], ["portfolio"]).expect("subject access"),
        FundingTier::ROnly,
        TaxTreatment::new("fixture-vat", "fixture-income").expect("tax treatment"),
        "fixture-assessment",
        "fixture-liability",
        None,
    )
    .expect("subject version");
    SubjectRecord::new(subject_value, version_value).expect("subject record")
}

fn baseline_input() -> PortfolioContextInput {
    PortfolioContextInput {
        scope: PortfolioScopeSelector::Portfolio(id(6)),
        valuation_at: time(20, 9),
        knowledge_at: time(21, 9),
        currency: PortfolioCurrencyMode::Original,
        look_through: PortfolioLookThroughMode::None,
        benchmark_id: id(5),
        period: PortfolioPeriodPreset::OneDay,
    }
}

fn shifted_time(value: &MarketTime, days: i64) -> MarketTime {
    let instant = value.instant() - Duration::days(days);
    let local_date = instant
        .with_timezone(&chrono_tz::Asia::Shanghai)
        .date_naive();
    MarketTime::new(instant, "Asia/Shanghai", local_date).expect("shifted market time")
}

fn principal() -> AuthorizedPrincipal {
    AuthorizedPrincipal::new(
        "researcher@example.test".to_owned(),
        id(3),
        owner().tenant_id().clone(),
        vec![owner().owner_id().clone()],
        PlatformRole::Researcher,
        [
            "portfolio:read",
            "positions:read",
            "rates:analyze",
            "facts:read",
            "definitions:read",
            "artifacts:read",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        ContentHash::digest(b"credential"),
    )
    .expect("principal")
}

fn admin_principal() -> AuthorizedPrincipal {
    AuthorizedPrincipal::new(
        "admin@example.test".to_owned(),
        id(3),
        owner().tenant_id().clone(),
        vec![owner().owner_id().clone()],
        PlatformRole::PlatformAdmin,
        vec!["portfolio:read".to_owned()],
        ContentHash::digest(b"credential"),
    )
    .expect("admin principal")
}

fn owner() -> OwnerRef {
    OwnerRef::new(id(1), id(2))
}

fn subject() -> VersionRef {
    version_ref(4)
}

fn version_ref(index: usize) -> VersionRef {
    VersionRef::new(id(index), version())
}

fn lineage(value: Ulid) -> LineageRef {
    LineageRef::new(
        value,
        Some(version()),
        Some(ContentHash::digest(b"lineage")),
    )
    .expect("lineage")
}

fn version() -> Version {
    Version::new(1).expect("version")
}

fn time(day: u32, hour: u32) -> MarketTime {
    let instant = Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0).unwrap();
    let local_date = instant
        .with_timezone(&chrono_tz::Asia::Shanghai)
        .date_naive();
    MarketTime::new(instant, "Asia/Shanghai", local_date).expect("market time")
}

fn error(category: ApplicationErrorCategory, retryable: bool) -> ApplicationError {
    ApplicationError::new(category, retryable)
}

#[derive(Default)]
struct SequentialIds {
    next: AtomicUsize,
}

impl IdGenerator for SequentialIds {
    fn next_id(&self) -> ApplicationResult<Ulid> {
        let offset = self.next.fetch_add(1, Ordering::SeqCst);
        Ok(id(20 + offset))
    }
}

struct FixedClock(MarketTime);

impl Clock for FixedClock {
    fn now(&self) -> ApplicationResult<MarketTime> {
        Ok(self.0.clone())
    }
}

fn id(index: usize) -> Ulid {
    const SUFFIXES: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let suffix = char::from(SUFFIXES[index]);
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).expect("ULID")
}
