use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, AuthorizedPrincipal, DATA_SOURCE_IMPORT_SCOPE, DataSourceAuthorizationRepository,
    DataSourceRepository, PageRequest, data_source_content_hash,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, ApplicationErrorDetail, CursorPage,
    GovernedInputUseCase,
};
use ficant_domain::VersionedDefinition;
use ficant_domain::governance::PlatformRole;
use ficant_domain::market::{
    DataSource, DataSourceAuthorization, DataSourceAuthorizationInput,
    DataSourceAuthorizationState, DataSourceInput, DataSourceKind, ImportInterface,
    PriceSourceType,
};
use ficant_domain::primitives::{
    ContentHash, EffectivePeriod, MarketTime, OwnerRef, Ulid, Version, VersionRef,
};

struct Repository {
    source: Option<DataSource>,
    authorization: Option<DataSourceAuthorization>,
    source_reads: Mutex<u32>,
    authorization_reads: Mutex<u32>,
}

#[async_trait]
impl DataSourceRepository for Repository {
    async fn register(
        &self,
        _: ficant_application::ports::RegisterDataSource,
    ) -> Result<DataSource, ApplicationError> {
        unreachable!()
    }

    async fn get_exact(
        &self,
        _: &AccessScope,
        reference: VersionRef,
    ) -> Result<Option<DataSource>, ApplicationError> {
        *self.source_reads.lock().unwrap() += 1;
        Ok(self.source.clone().filter(|source| {
            source.id() == reference.id() && source.version() == reference.version().get()
        }))
    }
}

#[async_trait]
impl DataSourceAuthorizationRepository for Repository {
    async fn publish_authorization(
        &self,
        _: ficant_application::ports::PublishDataSourceAuthorization,
    ) -> Result<DataSourceAuthorization, ApplicationError> {
        unreachable!()
    }

    async fn get_authorization_exact(
        &self,
        _: &AccessScope,
        reference: VersionRef,
    ) -> Result<Option<DataSourceAuthorization>, ApplicationError> {
        *self.authorization_reads.lock().unwrap() += 1;
        Ok(self.authorization.clone().filter(|authorization| {
            authorization.id() == reference.id()
                && authorization.version_value() == reference.version()
        }))
    }

    async fn list_authorizations_for_source(
        &self,
        _: &AccessScope,
        _: &OwnerRef,
        _: &VersionRef,
        _: Option<ImportInterface>,
        _: PageRequest,
    ) -> Result<CursorPage<DataSourceAuthorization>, ApplicationError> {
        Ok(CursorPage::new(Vec::new(), None))
    }
}

#[tokio::test]
async fn exact_active_capability_resolves_source_and_binds_mapping() {
    let source = source();
    let authorization = authorization(&source, DataSourceAuthorizationState::Active, period());
    let repository = Repository {
        source: Some(source.clone()),
        authorization: Some(authorization.clone()),
        source_reads: Mutex::new(0),
        authorization_reads: Mutex::new(0),
    };
    let resolved = GovernedInputUseCase::new(&repository, &repository)
        .resolve_authorized_data_source(
            &researcher(&owner(), DATA_SOURCE_IMPORT_SCOPE),
            &authorization.version_ref(),
            authorization.mapping_id(),
            authorization.mapping_hash(),
            ImportInterface::CanonicalQuoteSnapshot,
            &time(2026, 8, 13),
        )
        .await
        .unwrap();
    assert_eq!(resolved.data_source(), &source);
    assert_eq!(resolved.authorization_hash(), authorization.content_hash());
    assert_eq!(*repository.authorization_reads.lock().unwrap(), 1);
    assert_eq!(*repository.source_reads.lock().unwrap(), 1);
}

#[tokio::test]
async fn missing_or_drifted_capability_fails_before_source_resolution() {
    let source = source();
    let authorization = authorization(&source, DataSourceAuthorizationState::Active, period());
    let mut cases = Vec::new();
    cases.push((
        None,
        authorization.mapping_id().clone(),
        authorization.mapping_hash().clone(),
        time(2026, 8, 13),
    ));
    cases.push((
        Some(authorization.clone()),
        id('Z'),
        authorization.mapping_hash().clone(),
        time(2026, 8, 13),
    ));
    cases.push((
        Some(authorization.clone()),
        authorization.mapping_id().clone(),
        ContentHash::digest(b"wrong"),
        time(2026, 8, 13),
    ));
    cases.push((
        Some(authorization.clone()),
        authorization.mapping_id().clone(),
        authorization.mapping_hash().clone(),
        time(2027, 8, 13),
    ));

    for (candidate, mapping_id, mapping_hash, at) in cases {
        let repository = Repository {
            source: Some(source.clone()),
            authorization: candidate,
            source_reads: Mutex::new(0),
            authorization_reads: Mutex::new(0),
        };
        let error = GovernedInputUseCase::new(&repository, &repository)
            .resolve_authorized_data_source(
                &researcher(&owner(), DATA_SOURCE_IMPORT_SCOPE),
                &authorization.version_ref(),
                &mapping_id,
                &mapping_hash,
                ImportInterface::CanonicalQuoteSnapshot,
                &at,
            )
            .await
            .unwrap_err();
        assert_eq!(error.category(), ApplicationErrorCategory::Forbidden);
        assert!(matches!(
            error.detail(),
            Some(ApplicationErrorDetail::DataSourceNotAuthorized { .. })
        ));
        assert_eq!(*repository.source_reads.lock().unwrap(), 0);
    }
}

#[tokio::test]
async fn role_and_scope_drift_fail_before_capability_repository() {
    let source = source();
    let authorization = authorization(&source, DataSourceAuthorizationState::Active, period());
    for principal in [
        principal(
            &owner(),
            PlatformRole::PlatformAdmin,
            DATA_SOURCE_IMPORT_SCOPE,
        ),
        researcher(&owner(), "data-sources:read"),
    ] {
        let repository = Repository {
            source: Some(source.clone()),
            authorization: Some(authorization.clone()),
            source_reads: Mutex::new(0),
            authorization_reads: Mutex::new(0),
        };
        let error = GovernedInputUseCase::new(&repository, &repository)
            .resolve_authorized_data_source(
                &principal,
                &authorization.version_ref(),
                authorization.mapping_id(),
                authorization.mapping_hash(),
                ImportInterface::CanonicalQuoteSnapshot,
                &time(2026, 8, 13),
            )
            .await
            .unwrap_err();
        assert_eq!(error.category(), ApplicationErrorCategory::Forbidden);
        assert_eq!(*repository.authorization_reads.lock().unwrap(), 0);
        assert_eq!(*repository.source_reads.lock().unwrap(), 0);
    }
}

fn authorization(
    source: &DataSource,
    state: DataSourceAuthorizationState,
    effective: EffectivePeriod,
) -> DataSourceAuthorization {
    DataSourceAuthorization::new(DataSourceAuthorizationInput {
        authorization_id: id('V'),
        version: Version::new(1).unwrap(),
        owner: source.owner().clone(),
        data_source: VersionRef::new(source.id().clone(), Version::new(source.version()).unwrap()),
        data_source_hash: data_source_content_hash(source),
        import_interface: ImportInterface::CanonicalQuoteSnapshot,
        canonical_schema_id: source.canonical_schema_id().to_owned(),
        canonical_schema_hash: source.canonical_schema_hash().clone(),
        effective,
        state,
        supersedes: None,
        mapping_id: id('M'),
        mapping_hash: ContentHash::digest(b"mapping"),
    })
    .unwrap()
}

fn source() -> DataSource {
    DataSource::new(DataSourceInput {
        data_source_id: id('D'),
        version: Version::new(1).unwrap(),
        owner: owner(),
        kind: DataSourceKind::FileNdjson,
        name: "CGB quotes".to_owned(),
        connection_binding: "cgb-primary".to_owned(),
        dataset: "quotes".to_owned(),
        canonical_schema_id: "ficant.market.quote.canonical.v1".to_owned(),
        canonical_schema_hash: ContentHash::digest(b"schema"),
    })
    .unwrap()
    .with_price_source_type(PriceSourceType::ActiveQuote)
    .unwrap()
}

fn principal(owner: &OwnerRef, role: PlatformRole, scope: &str) -> AuthorizedPrincipal {
    AuthorizedPrincipal::new(
        "same-human".to_owned(),
        id('A'),
        owner.tenant_id().clone(),
        vec![owner.owner_id().clone()],
        role,
        vec![scope.to_owned()],
        ContentHash::digest(b"credential"),
    )
    .unwrap()
}

fn researcher(owner: &OwnerRef, scope: &str) -> AuthorizedPrincipal {
    principal(owner, PlatformRole::Researcher, scope)
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('P'))
}
fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}
fn period() -> EffectivePeriod {
    EffectivePeriod::new(time(2026, 1, 1), time(2027, 1, 1)).unwrap()
}
fn time(year: i32, month: u32, day: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap(),
        "UTC",
        NaiveDate::from_ymd_opt(year, month, day).unwrap(),
    )
    .unwrap()
}
