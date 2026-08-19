use std::sync::Arc;

use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use ficant_api::{
    FoundationChangeGrpcService, PlatformApplication, PlatformPort, SessionPolicy, SystemClock,
    TrustedIdentity,
};
use ficant_application::ports::{
    AccessScope, AeadCursorCodec, ApplicationResult, CursorKey, CursorPage, FoundationChangeFilter,
    FoundationChangeRepository, PageRequest,
};
use ficant_contracts::ficant::core::v1 as pb;
use ficant_contracts::ficant::core::v1::foundation_change_service_server::FoundationChangeService;
use ficant_domain::governance::{
    ChangeJustification, FoundationChangeOperation, FoundationChangeRecord,
    FoundationChangeRecordInput, FoundationResourceKind, FoundationResourceRef, PlatformRole,
    SourceDocumentRef,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

struct Repository {
    change: FoundationChangeRecord,
}

#[async_trait]
impl FoundationChangeRepository for Repository {
    async fn get_change(
        &self,
        scope: &AccessScope,
        record_id: &Ulid,
    ) -> ApplicationResult<Option<FoundationChangeRecord>> {
        scope.authorize(self.change.owner())?;
        Ok((self.change.record_id() == record_id).then(|| self.change.clone()))
    }

    async fn list_changes(
        &self,
        scope: &AccessScope,
        filter: &FoundationChangeFilter,
        page: PageRequest,
    ) -> ApplicationResult<CursorPage<FoundationChangeRecord>> {
        page.authorize_scope(scope)?;
        scope.authorize(self.change.owner())?;
        let matches = filter
            .resource_ref()
            .is_none_or(|value| value == self.change.resource().canonical_ref())
            && filter
                .actor_id()
                .is_none_or(|value| value == self.change.actor_id());
        Ok(CursorPage::new(
            matches.then(|| self.change.clone()).into_iter().collect(),
            None,
        ))
    }
}

#[tokio::test]
async fn admin_reads_exact_change_and_all_server_owned_evidence() {
    let change = change();
    let service = service(PlatformRole::PlatformAdmin, change.clone());
    let response = service
        .get_foundation_change(Request::new(pb::GetFoundationChangeRequest {
            record_id: Some(proto_id('R')),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::get_foundation_change_response::Result::Change(value)) = response.result else {
        panic!("admin must read the exact change")
    };
    assert_eq!(value.record_id, Some(proto_id('R')));
    assert_eq!(value.actor_id, Some(proto_id('A')));
    assert_eq!(value.active_role, pb::PlatformRole::PlatformAdmin as i32);
    assert_eq!(value.operation, "data-source.register");
    assert_eq!(value.resource_ref, change.resource().canonical_ref());
    assert_eq!(value.change.unwrap().sources.len(), 1);
    assert_eq!(value.request_fingerprint.unwrap().value.len(), 32);

    let listed = service
        .list_foundation_changes(Request::new(pb::ListFoundationChangesRequest {
            resource_ref: change.resource().canonical_ref(),
            actor_id: Some(proto_id('A')),
            occurred_from: None,
            occurred_to: None,
            page: Some(pb::PageRequest {
                page_size: 10,
                cursor: String::new(),
            }),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::list_foundation_changes_response::Result::Changes(values)) = listed.result else {
        panic!("admin list must return change records")
    };
    assert_eq!(values.changes.len(), 1);
    assert_eq!(values.page.unwrap().next_cursor, "");
}

#[tokio::test]
async fn researcher_with_governance_scope_cannot_read_change_records() {
    let service = service(PlatformRole::Researcher, change());
    let response = service
        .get_foundation_change(Request::new(pb::GetFoundationChangeRequest {
            record_id: Some(proto_id('R')),
        }))
        .await
        .unwrap()
        .into_inner();
    let Some(pb::get_foundation_change_response::Result::Error(error)) = response.result else {
        panic!("scope without Platform Admin role must fail closed")
    };
    assert_eq!(error.code, pb::ErrorCode::Forbidden as i32);
}

fn service(role: PlatformRole, change: FoundationChangeRecord) -> FoundationChangeGrpcService {
    let identity = TrustedIdentity::implicit(
        "governance-test",
        id('A'),
        id('T'),
        vec![id('B')],
        role,
        ["governance:read"],
    )
    .unwrap();
    let application: Arc<dyn PlatformPort> = Arc::new(
        PlatformApplication::try_new(
            Arc::new(SystemClock),
            SessionPolicy::new(900, 60).unwrap(),
            KEY,
            vec![],
            Some(identity),
            vec![],
        )
        .unwrap(),
    );
    let cursor = Arc::new(
        AeadCursorCodec::new(
            CursorKey::new("governance-test", [7_u8; 32]).unwrap(),
            vec![],
        )
        .unwrap(),
    );
    FoundationChangeGrpcService::new(application, Arc::new(Repository { change }), cursor, KEY)
        .unwrap()
}

fn change() -> FoundationChangeRecord {
    FoundationChangeRecord::new(FoundationChangeRecordInput {
        record_id: id('R'),
        actor_id: id('A'),
        owner: OwnerRef::new(id('T'), id('B')),
        active_role: PlatformRole::PlatformAdmin,
        operation: FoundationChangeOperation::RegisterDataSource,
        resource: FoundationResourceRef::versioned(
            FoundationResourceKind::DataSource,
            VersionRef::new(id('D'), Version::new(1).unwrap()),
        ),
        before_hash: None,
        after_hash: ContentHash::digest(b"after"),
        change: ChangeJustification::new(
            "register governed source",
            vec![
                SourceDocumentRef::new("urn:test:source", ContentHash::digest(b"source")).unwrap(),
            ],
        )
        .unwrap(),
        request_fingerprint: ContentHash::digest(b"request"),
        occurred_at: MarketTime::new(
            Utc.with_ymd_and_hms(2026, 8, 13, 1, 2, 3).unwrap(),
            "UTC",
            NaiveDate::from_ymd_opt(2026, 8, 13).unwrap(),
        )
        .unwrap(),
        authorization_ref: None,
    })
    .unwrap()
}

fn proto_id(suffix: char) -> pb::Ulid {
    pb::Ulid {
        value: id(suffix).as_str().to_owned(),
    }
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F0{suffix}")).unwrap()
}
