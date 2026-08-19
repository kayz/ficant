use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ficant_api::{
    MarketDefinitionGrpcService, PlatformApplication, PlatformPort, SessionPolicy, SystemClock,
    TrustedIdentity,
};
use ficant_application::ports::{
    AccessScope, AeadCursorCodec, AppendDefinitionVersion, ApplicationResult, CursorKey,
    CursorPage, DefinitionIdentity, DefinitionRepository, DefinitionValue,
    GovernedAppendDefinitionVersion, PageRequest,
};
use ficant_application::{ApplicationError, ApplicationErrorCategory};
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as pb;
use ficant_contracts::ficant::market::v1::market_definition_service_server::MarketDefinitionService;
use ficant_domain::governance::{FoundationChangeOperation, PlatformRole};
use ficant_domain::primitives::{ContentHash, MarketTime, Ulid, Version};
use prost_types::{Any, Timestamp};
use tonic::Request;

const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct Repository {
    values: Mutex<Vec<DefinitionValue>>,
    governed_writes: AtomicUsize,
    legacy_writes: AtomicUsize,
}

#[async_trait]
impl DefinitionRepository for Repository {
    async fn append_complete(
        &self,
        command: GovernedAppendDefinitionVersion,
    ) -> ApplicationResult<DefinitionValue> {
        assert_eq!(
            command
                .change_record(None)
                .expect("governed append must materialize change evidence")
                .operation(),
            FoundationChangeOperation::AppendMarketDefinition,
        );
        self.governed_writes.fetch_add(1, Ordering::SeqCst);
        let mut values = self.values.lock().unwrap();
        if let Some(existing) = values.iter().find(|value| {
            value.identity() == command.value().identity()
                && value.version() == command.value().version()
        }) {
            return if existing == command.value() {
                Ok(existing.clone())
            } else {
                Err(ApplicationError::new(
                    ApplicationErrorCategory::ImmutableViolation,
                    false,
                ))
            };
        }
        values.push(command.value().clone());
        Ok(command.value().clone())
    }

    async fn create_identity(&self, _: DefinitionIdentity) -> ApplicationResult<()> {
        Err(not_used())
    }

    async fn append_version(
        &self,
        _: AppendDefinitionVersion,
    ) -> ApplicationResult<DefinitionValue> {
        self.legacy_writes.fetch_add(1, Ordering::SeqCst);
        Err(not_used())
    }

    async fn get_version(
        &self,
        scope: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        let result = self
            .values
            .lock()
            .unwrap()
            .iter()
            .find(|value| {
                value.identity() == definition_id.as_str() && value.version() == version.get()
            })
            .cloned();
        if let Some(value) = result.as_ref() {
            scope.authorize(value.owner())?;
        }
        Ok(result)
    }

    async fn resolve_as_of(
        &self,
        scope: &AccessScope,
        definition_id: Ulid,
        _: MarketTime,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        let result = self
            .values
            .lock()
            .unwrap()
            .iter()
            .filter(|value| value.identity() == definition_id.as_str())
            .max_by_key(|value| value.version())
            .cloned();
        if let Some(value) = result.as_ref() {
            scope.authorize(value.owner())?;
        }
        Ok(result)
    }

    async fn list_versions(
        &self,
        scope: &AccessScope,
        definition_id: Ulid,
        page: PageRequest,
    ) -> ApplicationResult<CursorPage<DefinitionValue>> {
        page.authorize_scope(scope)?;
        let mut values = self
            .values
            .lock()
            .unwrap()
            .iter()
            .filter(|value| value.identity() == definition_id.as_str())
            .cloned()
            .collect::<Vec<_>>();
        values.sort_by_key(DefinitionValue::version);
        for value in &values {
            scope.authorize(value.owner())?;
        }
        Ok(CursorPage::new(values, None))
    }
}

#[tokio::test]
async fn admin_round_trips_complete_definitions_and_both_roles_read_all_query_shapes() {
    fn assert_service<T: MarketDefinitionService>() {}
    assert_service::<MarketDefinitionGrpcService>();

    let repository = Arc::new(Repository::default());
    let admin = service(repository.clone(), PlatformRole::PlatformAdmin, id('B'));
    let researcher = service(repository.clone(), PlatformRole::Researcher, id('B'));
    let fixtures = [
        bond_definition('B'),
        futures_definition('F'),
        calendar_definition('C'),
        unit_definition('D', 1, 2),
        rule_pack_definition('P'),
    ];
    for (index, fixture) in fixtures.iter().enumerate() {
        let stored = append(&admin, fixture.clone(), 0, &format!("definition-{index}"))
            .await
            .expect("Platform Admin must append a complete Definition");
        assert_eq!(&stored, fixture, "every complete variant must round-trip");
    }
    let unit_v2 = unit_definition('D', 2, 3);
    assert_eq!(
        append(&admin, unit_v2.clone(), 1, "definition-unit-v2")
            .await
            .unwrap(),
        unit_v2,
    );

    for reader in [&admin, &researcher] {
        let exact = reader
            .get_definition_version(Request::new(pb::GetDefinitionVersionRequest {
                definition_id: Some(proto_id('B')),
                version: 1,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            exact.result,
            Some(pb::get_definition_version_response::Result::Definition(_))
        ));

        let resolved = reader
            .resolve_definition_as_of(Request::new(pb::ResolveDefinitionAsOfRequest {
                definition_id: Some(proto_id('C')),
                as_of: Some(timestamp(12)),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(matches!(
            resolved.result,
            Some(pb::resolve_definition_as_of_response::Result::Definition(_))
        ));

        let listed = reader
            .list_definition_versions(Request::new(pb::ListDefinitionVersionsRequest {
                definition_id: Some(proto_id('D')),
                page: Some(core::PageRequest {
                    page_size: 20,
                    cursor: String::new(),
                }),
            }))
            .await
            .unwrap()
            .into_inner();
        let Some(pb::list_definition_versions_response::Result::Versions(listed)) = listed.result
        else {
            panic!("both active roles with read scope must list Definition versions");
        };
        assert_eq!(listed.definitions.len(), 2);
        assert_eq!(listed.page.unwrap().next_cursor, "");
    }

    assert_eq!(repository.governed_writes.load(Ordering::SeqCst), 6);
    assert_eq!(
        repository.legacy_writes.load(Ordering::SeqCst),
        0,
        "the public service must only reach governed append_complete",
    );
}

#[tokio::test]
async fn researcher_scope_and_incomplete_change_evidence_fail_before_any_definition_write() {
    let repository = Arc::new(Repository::default());
    let researcher = service(repository.clone(), PlatformRole::Researcher, id('B'));
    let response = researcher
        .append_definition(Request::new(pb::AppendDefinitionRequest {
            idempotency_key: "researcher-direct-write".to_owned(),
            expected_latest_version: 0,
            definition: Some(unit_definition('D', 1, 2)),
            change: Some(change()),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_error(response.result, core::ErrorCode::Forbidden);

    let admin = service(repository.clone(), PlatformRole::PlatformAdmin, id('B'));
    let response = admin
        .append_definition(Request::new(pb::AppendDefinitionRequest {
            idempotency_key: "missing-source-evidence".to_owned(),
            expected_latest_version: 0,
            definition: Some(unit_definition('V', 1, 2)),
            change: Some(core::ChangeJustification {
                reason: "reason alone is not evidence".to_owned(),
                sources: vec![],
            }),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_error(response.result, core::ErrorCode::ValidationFailed);

    let wrong_owner = service(repository.clone(), PlatformRole::PlatformAdmin, id('Z'));
    let response = wrong_owner
        .append_definition(Request::new(pb::AppendDefinitionRequest {
            idempotency_key: "wrong-owner".to_owned(),
            expected_latest_version: 0,
            definition: Some(unit_definition('W', 1, 2)),
            change: Some(change()),
        }))
        .await
        .unwrap()
        .into_inner();
    assert_error(response.result, core::ErrorCode::Forbidden);

    assert_eq!(repository.governed_writes.load(Ordering::SeqCst), 0);
    assert_eq!(repository.legacy_writes.load(Ordering::SeqCst), 0);
}

async fn append(
    service: &MarketDefinitionGrpcService,
    definition: pb::MarketDefinition,
    expected_latest_version: u64,
    key: &str,
) -> Option<pb::MarketDefinition> {
    let response = service
        .append_definition(Request::new(pb::AppendDefinitionRequest {
            idempotency_key: key.to_owned(),
            expected_latest_version,
            definition: Some(definition),
            change: Some(change()),
        }))
        .await
        .unwrap()
        .into_inner();
    match response.result {
        Some(pb::append_definition_response::Result::Definition(value)) => Some(value),
        Some(pb::append_definition_response::Result::Error(error)) => {
            panic!("unexpected Definition error for {key}: {}", error.code)
        }
        None => None,
    }
}

fn service(
    repository: Arc<Repository>,
    active_role: PlatformRole,
    allowed_owner: Ulid,
) -> MarketDefinitionGrpcService {
    let identity = TrustedIdentity::implicit(
        "definition-test",
        id('A'),
        id('T'),
        vec![allowed_owner],
        active_role,
        ["definitions:read", "definitions:write"],
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
        AeadCursorCodec::new(CursorKey::new("definition", [7_u8; 32]).unwrap(), vec![]).unwrap(),
    );
    MarketDefinitionGrpcService::new(application, repository, cursor, KEY).unwrap()
}

fn bond_definition(suffix: char) -> pb::MarketDefinition {
    let instrument = instrument(suffix, pb::InstrumentKind::Bond);
    pb::MarketDefinition {
        definition: Some(pb::market_definition::Definition::Instrument(
            pb::CompleteInstrumentDefinition {
                instrument: Some(instrument.clone()),
                subtype: Some(pb::complete_instrument_definition::Subtype::Bond(
                    pb::Bond {
                        instrument: Some(version_ref(suffix, 1)),
                        maturity_date: "2036-01-01".to_owned(),
                        face_value: Some(decimal("100001", 2)),
                        first_issue_date: "2026-01-01".to_owned(),
                        current_issue_date: "2026-01-01".to_owned(),
                        cumulative_issued_amount: Some(decimal("100001", 2)),
                        tax_attributes: Some(pb::BondTaxAttributes {
                            value_added_tax_status: pb::ValueAddedTaxStatus::Exempt as i32,
                            income_tax_status: pb::IncomeTaxStatus::Exempt as i32,
                        }),
                        coupon_rate: Some(decimal("225", 4)),
                        coupon_frequency: pb::BondCouponFrequency::Annual as i32,
                        day_count: pb::BondDayCountConvention::ActActBondIsma as i32,
                        business_day: pb::BondBusinessDayConvention::Following as i32,
                    },
                )),
            },
        )),
    }
}

fn futures_definition(suffix: char) -> pb::MarketDefinition {
    pb::MarketDefinition {
        definition: Some(pb::market_definition::Definition::Instrument(
            pb::CompleteInstrumentDefinition {
                instrument: Some(instrument(suffix, pb::InstrumentKind::Futures)),
                subtype: Some(
                    pb::complete_instrument_definition::Subtype::FuturesContract(
                        pb::FuturesContract {
                            instrument: Some(version_ref(suffix, 1)),
                            last_trade_time: Some(market_time(9)),
                            expiry_time: Some(market_time(10)),
                            settlement_time: Some(market_time(11)),
                            multiplier: Some(decimal("1000001", 2)),
                            rule_pack: Some(version_ref('R', 1)),
                            product_code: "T".to_owned(),
                            price_unit: Some(unit_ref('Q', 1)),
                        },
                    ),
                ),
            },
        )),
    }
}

fn calendar_definition(suffix: char) -> pb::MarketDefinition {
    pb::MarketDefinition {
        definition: Some(pb::market_definition::Definition::Calendar(pb::Calendar {
            calendar_id: Some(proto_id(suffix)),
            version: 1,
            owner: Some(owner()),
            market: "XSHG".to_owned(),
            market_timezone: "Asia/Shanghai".to_owned(),
            effective_from: Some(market_time(0)),
            effective_to: Some(market_time_next_day(0)),
            sessions: vec![pb::CalendarSession {
                local_date: "2026-08-13".to_owned(),
                open_local_time: "09:00:00".to_owned(),
                close_local_time: "15:00:00".to_owned(),
                closed: false,
            }],
        })),
    }
}

fn unit_definition(suffix: char, version: u64, precision: u32) -> pb::MarketDefinition {
    pb::MarketDefinition {
        definition: Some(pb::market_definition::Definition::Unit(pb::Unit {
            unit_id: Some(proto_id(suffix)),
            version,
            owner: Some(owner()),
            code: "CNY".to_owned(),
            dimension: "currency".to_owned(),
            scale: 2,
            precision,
        })),
    }
}

fn rule_pack_definition(suffix: char) -> pb::MarketDefinition {
    let payload = b"rule-pack-payload".to_vec();
    pb::MarketDefinition {
        definition: Some(pb::market_definition::Definition::MarketRulePack(
            pb::MarketRulePack {
                rule_pack_id: Some(proto_id(suffix)),
                version: 1,
                owner: Some(owner()),
                market: "XSHG".to_owned(),
                rule_type: "test.rule.v1".to_owned(),
                source: "urn:test:rule".to_owned(),
                effective_from: Some(market_time(0)),
                effective_to: Some(market_time_next_day(0)),
                verification_status: pb::VerificationStatus::Verified as i32,
                content_hash: Some(hash(&payload)),
                content: Some(Any {
                    type_url: "type.googleapis.com/test.Rule".to_owned(),
                    value: payload,
                }),
            },
        )),
    }
}

fn instrument(suffix: char, kind: pb::InstrumentKind) -> pb::Instrument {
    pb::Instrument {
        instrument_id: Some(proto_id(suffix)),
        version: 1,
        owner: Some(owner()),
        kind: kind as i32,
        market: "XSHG".to_owned(),
        symbol: format!("TEST-{suffix}"),
        currency: Some(unit_ref('M', 1)),
        calendar: Some(version_ref('C', 1)),
    }
}

fn change() -> core::ChangeJustification {
    core::ChangeJustification {
        reason: "publish complete immutable Definition".to_owned(),
        sources: vec![core::SourceDocumentRef {
            uri: "urn:test:definition-source".to_owned(),
            sha256: Some(hash(b"definition-source")),
        }],
    }
}

fn assert_error(result: Option<pb::append_definition_response::Result>, expected: core::ErrorCode) {
    let Some(pb::append_definition_response::Result::Error(error)) = result else {
        panic!("request must fail with a business error");
    };
    assert_eq!(error.code, expected as i32);
}

fn decimal(coefficient: &str, scale: u32) -> core::DecimalValue {
    core::DecimalValue {
        coefficient: coefficient.to_owned(),
        scale,
        unit: Some(unit_ref('M', 1)),
    }
}

fn market_time(hour: i64) -> core::MarketTime {
    core::MarketTime {
        instant: Some(timestamp(hour)),
        market_timezone: "Asia/Shanghai".to_owned(),
        local_trading_date: "2026-08-13".to_owned(),
    }
}

fn market_time_next_day(hour: i64) -> core::MarketTime {
    core::MarketTime {
        instant: Some(Timestamp {
            seconds: timestamp(hour).seconds + 86_400,
            nanos: 0,
        }),
        market_timezone: "Asia/Shanghai".to_owned(),
        local_trading_date: "2026-08-14".to_owned(),
    }
}

fn timestamp(hour: i64) -> Timestamp {
    Timestamp {
        seconds: 1_786_550_400 + hour * 3_600,
        nanos: 0,
    }
}

fn hash(value: &[u8]) -> core::Sha256 {
    core::Sha256 {
        value: ContentHash::digest(value).as_bytes().to_vec(),
    }
}

fn owner() -> core::OwnerRef {
    core::OwnerRef {
        tenant_id: Some(proto_id('T')),
        owner_id: Some(proto_id('B')),
    }
}

fn version_ref(suffix: char, version: u64) -> core::VersionRef {
    core::VersionRef {
        id: Some(proto_id(suffix)),
        version,
    }
}

fn unit_ref(suffix: char, version: u64) -> core::UnitRef {
    core::UnitRef {
        unit_id: Some(proto_id(suffix)),
        version,
    }
}

fn proto_id(suffix: char) -> core::Ulid {
    core::Ulid {
        value: id(suffix).as_str().to_owned(),
    }
}

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5F0{suffix}")).unwrap()
}

fn not_used() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StateConflict, false)
}
