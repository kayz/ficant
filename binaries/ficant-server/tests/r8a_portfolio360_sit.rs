use std::collections::BTreeMap;
use std::env;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use chrono::{NaiveDate, TimeZone, Utc};
use ficant_api::{GrpcWebServerConfig, build_production_routes, serve_production_routes};
use ficant_application::ports::{BondAnalyticsEngine, YieldCurveEngine};
use ficant_contracts::ficant::app::v1::GetCurrentSessionRequest;
use ficant_contracts::ficant::core::v1 as core;
use ficant_contracts::ficant::market::v1 as market;
use ficant_contracts::ficant::portfolio::v1 as portfolio;
use ficant_contracts::ficant::portfolio::v1::portfolio_aggregation_service_client::PortfolioAggregationServiceClient;
use ficant_contracts::ficant::portfolio::v1::portfolio_workbench_service_client::PortfolioWorkbenchServiceClient;
use ficant_contracts::ficant::research::v1 as research;
use ficant_contracts::ficant::research::v1::portfolio_risk_service_client::PortfolioRiskServiceClient;
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondTerms, BusinessDayConvention,
    CalendarBinding, CalendarRequirement, CouponFrequency, DayCountConvention, FixedDecimal,
};
use ficant_domain::curves::{
    YieldCurveBinding, YieldCurveInterpolation, YieldCurveNode, YieldCurveQuery,
};
use ficant_domain::primitives::{ContentHash, MarketTime, OwnerRef, Ulid, Version, VersionRef};
use ficant_domain::research::{SensitivityDirection, bond_position_key_rate_dv01};
use ficant_fixed_income_native::{NativeBondAnalyticsEngine, NativeYieldCurveEngine};
use ficant_server::{ServerSettings, build_production_grpc_services};
use prost::Message;
use prost_types::Timestamp;
use sqlx::postgres::PgPoolOptions;
use tonic::Request;

const KEY: &str = "3031323334353637383961626364656630313233343536373839616263646566";
const ALLOWED_ORIGIN: &str = "http://127.0.0.1:5173";
const TENANT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA1";
const OWNER_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA2";
const ACTOR_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE0";
const PORTFOLIO_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA9";
const BENCHMARK_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA7";
const INSTRUMENT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FB3";
const SELECTED_VALUATION_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAN";
const POSITION_SNAPSHOT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA5";
const CURVE_SNAPSHOT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAG";
const DV01_UNIT_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FBC";
const SCOPES: &str =
    "portfolio:read,positions:read,rates:analyze,facts:read,definitions:read,artifacts:read";
const SERVER_TEST_ENVIRONMENT: &str =
    "ficant.server.environment.v1\narch=amd64\nos=windows\nprofile=test";

#[test]
#[allow(clippy::too_many_lines)]
fn synthetic_bond_terms_are_executable_by_the_native_engine() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/portfolio360/analytics-p0.json"
    ))
    .expect("analytics fixture is valid JSON");
    let owner = OwnerRef::new(
        Ulid::new(TENANT_ID).expect("tenant is valid"),
        Ulid::new(OWNER_ID).expect("owner is valid"),
    );
    let version = Version::new(1).expect("version is valid");
    let object = |id: &str, seed: &[u8]| {
        AnalyticsObjectRef::new(
            VersionRef::new(Ulid::new(id).expect("object id is valid"), version),
            ContentHash::digest(seed),
        )
    };
    let valuation_at = MarketTime::new(
        Utc.with_ymd_and_hms(2026, 8, 21, 2, 0, 0)
            .single()
            .expect("fixture instant is valid"),
        "Asia/Shanghai",
        NaiveDate::from_ymd_opt(2026, 8, 21).expect("fixture date is valid"),
    )
    .expect("fixture MarketTime is valid");
    let calendar = CalendarBinding::new(
        "r8a-synthetic-calendar",
        version,
        ContentHash::digest(b"r8a-synthetic-calendar"),
        NaiveDate::from_ymd_opt(2020, 1, 1).expect("coverage start is valid"),
        NaiveDate::from_ymd_opt(2040, 1, 1).expect("coverage end is valid"),
        Vec::new(),
        Vec::new(),
    )
    .expect("calendar binding is valid");
    let curve_reference = object(CURVE_SNAPSHOT_ID, b"curve");
    let curve = YieldCurveBinding::new(
        curve_reference.clone(),
        NaiveDate::from_ymd_opt(2026, 8, 21).expect("valuation date is valid"),
        YieldCurveInterpolation::LinearYield,
        [
            (2028, 8, 21, 17_600_000_000_i128),
            (2031, 8, 21, 18_900_000_000_i128),
            (2036, 8, 21, 20_700_000_000_i128),
        ]
        .into_iter()
        .map(|(year, month, day, value)| {
            YieldCurveNode::new(
                NaiveDate::from_ymd_opt(year, month, day).expect("curve node date is valid"),
                FixedDecimal::from_scaled(value),
            )
            .expect("curve node is valid")
        })
        .collect(),
    )
    .expect("curve binding is valid");
    for instrument in fixture["instruments"]
        .as_array()
        .expect("instruments are an array")
    {
        let id = instrument["id"].as_str().expect("instrument id is text");
        let date = |field: &str| {
            NaiveDate::parse_from_str(
                instrument[field]
                    .as_str()
                    .expect("fixture date field is text"),
                "%Y-%m-%d",
            )
            .expect("fixture date is valid")
        };
        let scaled = |field: &str| {
            FixedDecimal::from_scaled(
                instrument[field]
                    .as_str()
                    .expect("fixture decimal is text")
                    .parse::<i128>()
                    .expect("fixture decimal is i128"),
            )
        };
        let base_yields = [17_600_000_000_i128, 18_900_000_000, 20_700_000_000];
        let terms = BondTerms::new(
            date("first_issue_date"),
            date("maturity_date"),
            CouponFrequency::Annual,
            DayCountConvention::ActActBondIsma,
            BusinessDayConvention::Following,
            scaled("coupon_rate"),
            FixedDecimal::from_scaled(100_000_000_000_000),
        )
        .expect("fixture bond terms are valid");
        let price_for = |yields: [i128; 3]| {
            let bumped_curve = YieldCurveBinding::new(
                curve_reference.clone(),
                curve.valuation_date(),
                YieldCurveInterpolation::LinearYield,
                curve
                    .nodes()
                    .iter()
                    .zip(yields)
                    .map(|(node, value)| {
                        YieldCurveNode::new(node.maturity_date(), FixedDecimal::from_scaled(value))
                            .expect("bumped curve node is valid")
                    })
                    .collect(),
            )
            .expect("bumped curve is valid");
            let query = YieldCurveQuery::new(bumped_curve, date("maturity_date"))
                .expect("maturity is covered by the fixture curve");
            let interpolated = NativeYieldCurveEngine
                .interpolate(&query)
                .unwrap_or_else(|error| panic!("native curve for {id} failed: {error:?}"));
            let input = BondAnalyticsInput::new(
                owner.clone(),
                object(id, id.as_bytes()),
                object("01ARZ3NDEKTSV4RRFFQ69G5FBE", b"rule"),
                curve_reference.clone(),
                valuation_at.clone(),
                NaiveDate::from_ymd_opt(2026, 8, 21).expect("settlement is valid"),
                CalendarRequirement::ExactMarket,
                calendar.clone(),
                terms.clone(),
                AnalyticsMode::YieldIn,
                interpolated.yield_to_maturity(),
            )
            .expect("native input materializes");
            NativeBondAnalyticsEngine
                .calculate(&input)
                .unwrap_or_else(|error| panic!("native bond {id} failed: {error:?}"))
                .measures()
                .dirty_price()
        };
        let base = price_for(base_yields);
        for index in 0..base_yields.len() {
            let mut up = base_yields;
            up[index] += 100_000_000;
            let mut down = base_yields;
            down[index] -= 100_000_000;
            let up_price = price_for(up);
            let down_price = price_for(down);
            for position in fixture["snapshots"]
                .as_array()
                .expect("snapshots are an array")
                .iter()
                .flat_map(|snapshot| {
                    snapshot["positions"]
                        .as_array()
                        .expect("positions are an array")
                })
                .filter(|position| position["instrument"]["id"].as_str() == Some(id))
            {
                let quantity = position["quantity"]
                    .as_str()
                    .expect("position quantity is text")
                    .parse::<i128>()
                    .expect("position quantity is i128");
                bond_position_key_rate_dv01(
                    base,
                    up_price,
                    down_price,
                    FixedDecimal::from_scaled(1_000_000_000_000),
                    SensitivityDirection::Central,
                    FixedDecimal::from_scaled(quantity),
                    FixedDecimal::from_scaled(100_000_000_000_000),
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "frozen native Bond-position KRD {id} node {index} quantity {quantity}: {error:?}"
                    )
                });
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the explicit check.ps1 -IncludeIntegration environment"]
async fn production_p01_p03_p04_and_session_to_get_page_use_real_postgres_s3_and_routes() {
    let Some(environment) = IntegrationEnvironment::load() else {
        eprintln!("skipping R8A Portfolio360 production SIT: integration environment is absent");
        return;
    };
    reset_and_migrate(&environment.database_url).await;
    run_bootstrap(&environment).await;
    run_bootstrap(&environment).await;
    assert_server_evidence_bindings(&environment);

    let address = free_address();
    let server = RunningServer::start(address, &environment).await;
    let endpoint = format!("http://{address}");
    let normalized = assert_native_p01(&endpoint).await;
    assert_native_portfolio_risk(&endpoint).await;
    assert_native_portfolio_overview(&endpoint, normalized).await;
    assert_native_p03(&endpoint).await;
    assert_native_p04(&endpoint).await;
    assert_grpc_web_session_then_p01(address).await;
    assert_persisted_portfolio_formal_evidence(&environment.database_url).await;
    server.stop().await;
}

async fn assert_native_portfolio_risk(endpoint: &str) {
    let mut client = PortfolioRiskServiceClient::connect(endpoint.to_owned())
        .await
        .expect("PortfolioRisk native client connects");
    let response = client
        .calculate_key_rate_dv01(Request::new(research::CalculateKeyRateDv01Request {
            position_snapshot_id: Some(proto_id(POSITION_SNAPSHOT_ID)),
            knowledge_at: Some(proto_time(3)),
            valuation_at: Some(proto_time(2)),
            curve_snapshot_id: Some(proto_id(CURVE_SNAPSHOT_ID)),
            dv01_unit: Some(core::UnitRef {
                unit_id: Some(proto_id(DV01_UNIT_ID)),
                version: 1,
            }),
            futures_data_snapshot_id: None,
        }))
        .await
        .expect("exact production PortfolioRisk succeeds")
        .into_inner();
    let exposure = match response.result {
        Some(research::calculate_key_rate_dv01_response::Result::Exposure(exposure)) => exposure,
        Some(research::calculate_key_rate_dv01_response::Result::Error(error)) => panic!(
            "PortfolioRisk returned error code={} message={} retryable={}",
            error.code, error.message, error.retryable
        ),
        None => panic!("PortfolioRisk response omitted its result"),
    };
    assert_eq!(exposure.positions.len(), 2);
    assert_eq!(exposure.totals.len(), 3);
}

async fn assert_native_p01(endpoint: &str) -> portfolio::NormalizedPortfolioContext {
    let mut client = PortfolioWorkbenchServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Portfolio Workbench native client connects");
    let envelope = client
        .get_page(Request::new(page_request()))
        .await
        .expect("P01 native route succeeds")
        .into_inner();
    assert_eq!(envelope.schema_version, "portfolio-workbench.v1");
    assert_eq!(
        envelope.page_id,
        portfolio::PortfolioWorkbenchPageId::P01 as i32
    );
    assert!(envelope.typed_error.is_none());
    let provenance = envelope
        .provenance
        .as_ref()
        .expect("P01 provenance is present");
    assert!(provenance.formal_evidence.is_empty());
    assert_eq!(provenance.non_formal_reads.len(), 1);
    let normalized = envelope
        .normalized_context
        .clone()
        .expect("P01 normalized context is present");
    let Some(portfolio::portfolio_page_envelope::Projection::P01(projection)) = envelope.projection
    else {
        panic!("P01 must return its typed production projection")
    };
    let catalog = projection.catalog.expect("P01 catalog is present");
    assert_eq!(catalog.books.len(), 1);
    assert_eq!(catalog.groups.len(), 1);
    assert_eq!(catalog.portfolios.len(), 2);
    assert!(catalog.read_evidence.is_some());
    let structure = projection.structure.expect("P01 structure is present");
    assert_eq!(structure.book_count, 1);
    assert_eq!(structure.group_count, 1);
    assert_eq!(structure.portfolio_count, 2);
    normalized
}

async fn assert_native_portfolio_overview(
    endpoint: &str,
    context: portfolio::NormalizedPortfolioContext,
) {
    let mut client = PortfolioAggregationServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Portfolio Aggregation native client connects");
    let response = client
        .get_portfolio_overview(Request::new(portfolio::GetPortfolioOverviewRequest {
            context: Some(context),
        }))
        .await
        .expect("Portfolio Aggregation native route succeeds")
        .into_inner();
    match response.result {
        Some(portfolio::get_portfolio_overview_response::Result::Overview(overview)) => {
            assert!(!overview.members.is_empty());
        }
        Some(portfolio::get_portfolio_overview_response::Result::Error(error)) => panic!(
            "Portfolio Aggregation returned error code={} message={} retryable={}",
            error.code, error.message, error.retryable
        ),
        None => panic!("Portfolio Aggregation response omitted its result"),
    }
}

async fn assert_native_p03(endpoint: &str) {
    let envelope = native_page(endpoint, portfolio::PortfolioWorkbenchPageId::P03, None).await;
    assert_real_ready(&envelope, portfolio::PortfolioWorkbenchPageId::P03);
    let Some(portfolio::portfolio_page_envelope::Projection::P03(projection)) =
        &envelope.projection
    else {
        panic!("P03 must return its typed production projection")
    };
    assert_eq!(projection.position_views.len(), 1);
    assert_eq!(projection.position_views[0].positions.len(), 2);
    assert_eq!(projection.key_rate_exposures.len(), 1);
    assert_eq!(projection.key_rate_exposures[0].positions.len(), 2);
    assert_eq!(projection.key_rate_exposures[0].totals.len(), 3);
    let coverage = projection
        .coverage
        .as_ref()
        .expect("P03 coverage is present");
    let participation = coverage
        .participation
        .as_ref()
        .expect("P03 participation is present");
    assert_eq!(participation.imported_position_count, 2);
    assert_eq!(participation.participating_position_count, 2);
    assert!(coverage.missing_reasons.is_empty());
    assert_payload_has_no_placeholder(&envelope);
}

async fn assert_native_p04(endpoint: &str) {
    let envelope = native_page(
        endpoint,
        portfolio::PortfolioWorkbenchPageId::P04,
        Some(portfolio::PortfolioPageSelection {
            instrument: Some(core::VersionRef {
                id: Some(proto_id(INSTRUMENT_ID)),
                version: 1,
            }),
        }),
    )
    .await;
    assert_real_ready(&envelope, portfolio::PortfolioWorkbenchPageId::P04);
    let Some(portfolio::portfolio_page_envelope::Projection::P04(projection)) =
        &envelope.projection
    else {
        panic!("P04 must return its typed production projection")
    };
    let definition = projection
        .definition
        .as_ref()
        .expect("P04 definition is present");
    let Some(market::market_definition::Definition::Instrument(definition)) =
        &definition.definition
    else {
        panic!("P04 exact definition must remain an Instrument")
    };
    assert!(matches!(
        definition.subtype,
        Some(market::complete_instrument_definition::Subtype::Bond(_))
    ));
    let facts = projection.facts.as_ref().expect("P04 facts are present");
    assert!(!facts.facts.is_empty());
    let mut selected_typed_valuation = false;
    for fact in &facts.facts {
        if let Some(market::market_fact::Fact::Valuation(valuation)) = &fact.fact {
            assert_eq!(
                valuation.value_roles,
                vec![
                    market::ValuationValueRole::Yield as i32,
                    market::ValuationValueRole::RemainingYears as i32,
                ]
            );
            if valuation
                .valuation_id
                .as_ref()
                .is_some_and(|value| value.value == SELECTED_VALUATION_ID)
            {
                selected_typed_valuation = true;
            }
        }
    }
    assert!(selected_typed_valuation);
    let analysis = projection
        .analysis
        .as_ref()
        .expect("P04 exact bond analysis is present");
    assert!(!analysis.cashflows.is_empty());
    assert!(analysis.measures.is_some());
    let metadata = analysis.metadata.as_ref().expect("P04 metadata is present");
    assert!(!metadata.consumed_inputs.is_empty());
    assert!(metadata.request_fingerprint.is_some());
    assert!(metadata.formal_evidence.is_none());
    assert!(analysis.after_tax.is_none());
    let provenance = envelope
        .provenance
        .as_ref()
        .expect("P04 provenance is present");
    assert_eq!(provenance.non_formal_reads.len(), 2);
    let fact_read = provenance
        .non_formal_reads
        .iter()
        .find(|value| value.schema_id == "ficant.fact.v1.QueryInstrumentFacts")
        .expect("P04 Fact read evidence is present");
    assert!(!fact_read.consumed_inputs.is_empty());
    assert!(
        fact_read
            .consumed_inputs
            .iter()
            .all(|value| value.kind == core::FormalInputKind::Fact as i32)
    );
    assert_payload_has_no_placeholder(&envelope);
}

async fn native_page(
    endpoint: &str,
    page_id: portfolio::PortfolioWorkbenchPageId,
    selection: Option<portfolio::PortfolioPageSelection>,
) -> portfolio::PortfolioPageEnvelope {
    let mut client = PortfolioWorkbenchServiceClient::connect(endpoint.to_owned())
        .await
        .expect("Portfolio Workbench native client connects");
    client
        .get_page(Request::new(page_request_for(page_id, selection)))
        .await
        .expect("native Workbench route succeeds")
        .into_inner()
}

fn assert_real_ready(
    envelope: &portfolio::PortfolioPageEnvelope,
    page_id: portfolio::PortfolioWorkbenchPageId,
) {
    assert_eq!(envelope.schema_version, "portfolio-workbench.v1");
    assert_eq!(envelope.page_id, page_id as i32);
    assert_eq!(
        envelope.data_mode,
        portfolio::PortfolioPageDataMode::Real as i32,
        "page returned typed error: {:?}",
        envelope.typed_error.as_ref().map(|value| (
            value.code,
            value.safe_message.as_str(),
            value.retryable
        ))
    );
    assert_eq!(
        envelope.page_state,
        portfolio::PortfolioPageState::Ready as i32
    );
    assert!(envelope.typed_error.is_none());
    assert!(envelope.normalized_context.is_some());
    let coverage = envelope
        .coverage
        .as_ref()
        .expect("page coverage is present");
    assert!(coverage.participation.is_some());
    assert!(coverage.missing_reasons.is_empty());
    let provenance = envelope
        .provenance
        .as_ref()
        .expect("page provenance is present");
    assert!(!provenance.formal_evidence.is_empty());
    assert!(provenance.request_fingerprint.is_some());
    for evidence in &provenance.formal_evidence {
        assert_eq!(evidence.schema_id, "ficant.portfolio.v1.PortfolioOverview");
        assert!(!evidence.consumed_inputs.is_empty());
        assert!(!evidence.implementations.is_empty());
        assert!(evidence.code.is_some());
        assert!(evidence.runtime.is_some());
        assert!(evidence.result_hash.is_some());
        assert!(evidence.output_identity.is_some());
    }
}

fn assert_payload_has_no_placeholder(envelope: &portfolio::PortfolioPageEnvelope) {
    let encoded = String::from_utf8_lossy(&envelope.encode_to_vec()).to_ascii_lowercase();
    for marker in ["mock", "demo", "placeholder"] {
        assert!(
            !encoded.contains(marker),
            "production page contains forbidden marker {marker}"
        );
    }
}

async fn assert_persisted_portfolio_formal_evidence(database_url: &str) {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(database_url)
        .await
        .expect("formal-evidence database is reachable");
    let values: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT formal_evidence FROM analytics.formal_outputs
         WHERE schema_id='ficant.portfolio.v1.PortfolioOverview'
         ORDER BY output_identity",
    )
    .fetch_all(&pool)
    .await
    .expect("persisted PortfolioOverview evidence is readable");
    assert!(!values.is_empty());
    for bytes in values {
        let evidence = core::FormalOutputEvidence::decode(bytes.as_slice())
            .expect("persisted PortfolioOverview evidence is canonical protobuf");
        assert!(
            evidence
                .consumed_inputs
                .iter()
                .any(|binding| binding.kind == core::FormalInputKind::Fact as i32)
        );
        assert!(!evidence.implementations.is_empty());
    }
    pool.close().await;
}

async fn assert_grpc_web_session_then_p01(address: SocketAddr) {
    let session = grpc_web_exchange(
        address,
        "/ficant.app.v1.PlatformService/GetCurrentSession",
        GetCurrentSessionRequest {}.encode_to_vec(),
    )
    .await;
    assert_grpc_web_headers(&session);
    assert!(
        session
            .windows("portfolio360-researcher".len())
            .any(|value| value == b"portfolio360-researcher"),
        "gRPC-Web session must use the real configured Researcher"
    );

    let page = grpc_web_exchange(
        address,
        "/ficant.portfolio.v1.PortfolioWorkbenchService/GetPage",
        page_request().encode_to_vec(),
    )
    .await;
    assert_grpc_web_headers(&page);
    assert!(
        page.windows("portfolio-workbench.v1".len())
            .any(|value| value == b"portfolio-workbench.v1"),
        "gRPC-Web GetPage must return the real Workbench envelope"
    );
    assert!(
        page.windows("BOOK-CGB".len())
            .any(|value| value == b"BOOK-CGB"),
        "gRPC-Web P01 must contain the bootstrapped catalog, not a mock"
    );
}

fn assert_grpc_web_headers(response: &[u8]) {
    let header_end = response
        .windows(4)
        .position(|value| value == b"\r\n\r\n")
        .expect("gRPC-Web response contains HTTP headers");
    let headers = String::from_utf8_lossy(&response[..header_end]).to_ascii_lowercase();
    assert!(headers.starts_with("http/1.1 200 ok\r\n"));
    assert!(headers.contains("content-type: application/grpc-web+proto\r\n"));
    assert!(headers.contains("access-control-allow-origin: http://127.0.0.1:5173\r\n"));
}

async fn grpc_web_exchange(address: SocketAddr, path: &str, payload: Vec<u8>) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.push(0);
    frame.extend_from_slice(
        &u32::try_from(payload.len())
            .expect("protobuf payload length fits u32")
            .to_be_bytes(),
    );
    frame.extend_from_slice(&payload);
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nOrigin: {ALLOWED_ORIGIN}\r\nContent-Type: application/grpc-web+proto\r\nX-Grpc-Web: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        frame.len()
    )
    .into_bytes();
    request.extend_from_slice(&frame);
    tokio::task::spawn_blocking(move || {
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))
            .expect("gRPC-Web client connects");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("gRPC-Web read timeout configures");
        stream.write_all(&request).expect("gRPC-Web request writes");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .expect("gRPC-Web response reads");
        response
    })
    .await
    .expect("blocking gRPC-Web exchange joins")
}

fn page_request() -> portfolio::GetPortfolioPageRequest {
    page_request_for(portfolio::PortfolioWorkbenchPageId::P01, None)
}

fn page_request_for(
    page_id: portfolio::PortfolioWorkbenchPageId,
    selection: Option<portfolio::PortfolioPageSelection>,
) -> portfolio::GetPortfolioPageRequest {
    portfolio::GetPortfolioPageRequest {
        page_id: page_id as i32,
        context: Some(portfolio::PortfolioContextInput {
            scope: Some(portfolio::PortfolioScopeSelector {
                scope: Some(portfolio::portfolio_scope_selector::Scope::PortfolioId(
                    proto_id(PORTFOLIO_ID),
                )),
            }),
            valuation_at: Some(proto_time(2)),
            knowledge_at: Some(proto_time(3)),
            currency: portfolio::PortfolioCurrencyMode::Cny as i32,
            look_through: portfolio::PortfolioLookThroughMode::None as i32,
            benchmark_id: Some(proto_id(BENCHMARK_ID)),
            period: portfolio::PortfolioPeriodPreset::OneDay as i32,
        }),
        selection,
    }
}

fn proto_id(value: &str) -> core::Ulid {
    core::Ulid {
        value: value.to_owned(),
    }
}

fn proto_time(hour: u32) -> core::MarketTime {
    let instant = Utc
        .with_ymd_and_hms(2026, 8, 21, hour, 0, 0)
        .single()
        .expect("fixture time is valid");
    core::MarketTime {
        instant: Some(Timestamp {
            seconds: instant.timestamp(),
            nanos: 0,
        }),
        market_timezone: "Asia/Shanghai".to_owned(),
        local_trading_date: NaiveDate::from_ymd_opt(2026, 8, 21)
            .expect("fixture date is valid")
            .to_string(),
    }
}

async fn run_bootstrap(environment: &IntegrationEnvironment) {
    let repository = repository_root();
    let script = repository.join("scripts/bootstrap-portfolio360-p0.ps1");
    let output = tokio::task::spawn_blocking({
        let environment = environment.clone();
        move || {
            Command::new("pwsh")
                .args(["-NoProfile", "-NonInteractive", "-File"])
                .arg(script)
                .current_dir(repository)
                .env("FICANT_EXPERIMENT_DATABASE_URL", &environment.database_url)
                .env("FICANT_EXPERIMENT_S3_ENDPOINT", &environment.s3_endpoint)
                .env("FICANT_EXPERIMENT_S3_BUCKET", &environment.s3_bucket)
                .env(
                    "FICANT_EXPERIMENT_S3_ACCESS_KEY",
                    &environment.s3_access_key,
                )
                .env(
                    "FICANT_EXPERIMENT_S3_SECRET_KEY",
                    &environment.s3_secret_key,
                )
                .output()
                .expect("Portfolio360 bootstrap process starts")
        }
    })
    .await
    .expect("Portfolio360 bootstrap process joins");
    assert!(
        output.status.success(),
        "Portfolio360 bootstrap failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[derive(Clone)]
struct IntegrationEnvironment {
    database_url: String,
    s3_endpoint: String,
    s3_bucket: String,
    s3_access_key: String,
    s3_secret_key: String,
    runtime_digest: String,
}

impl IntegrationEnvironment {
    fn load() -> Option<Self> {
        Some(Self {
            database_url: env::var("FICANT_TEST_DATABASE_URL").ok()?,
            s3_endpoint: env::var("FICANT_TEST_S3_ENDPOINT").ok()?,
            s3_bucket: env::var("FICANT_TEST_S3_BUCKET").ok()?,
            s3_access_key: env::var("FICANT_TEST_S3_ACCESS_KEY").ok()?,
            s3_secret_key: env::var("FICANT_TEST_S3_SECRET_KEY").ok()?,
            runtime_digest: env::var("FICANT_TEST_RUNTIME_IMAGE_DIGEST").ok()?,
        })
    }
}

struct RunningServer {
    handle: tokio::task::JoinHandle<Result<(), ficant_api::GrpcWebServeError>>,
}

impl RunningServer {
    async fn start(address: SocketAddr, environment: &IntegrationEnvironment) -> Self {
        let settings = ServerSettings::try_from_values(&server_values(address, environment))
            .expect("R8A Portfolio360 SIT settings are valid");
        let services =
            build_production_grpc_services(&settings).expect("production services compose");
        let routes = build_production_routes(services).expect("production routes are unique");
        let handle = tokio::spawn(serve_production_routes(
            GrpcWebServerConfig {
                bind: address,
                allowed_origins: vec![ALLOWED_ORIGIN.to_owned()],
            },
            routes,
        ));
        wait_until_listening(address).await;
        Self { handle }
    }

    async fn stop(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

async fn reset_and_migrate(database_url: &str) {
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(database_url)
        .await
        .expect("integration PostgreSQL is reachable");
    sqlx::raw_sql(
        "DROP SCHEMA IF EXISTS portfolio CASCADE;
         DROP SCHEMA IF EXISTS analytics CASCADE;
         DROP SCHEMA IF EXISTS data CASCADE;
         DROP SCHEMA IF EXISTS storage CASCADE;
         DROP SCHEMA IF EXISTS research CASCADE;
         DROP SCHEMA IF EXISTS market CASCADE;
         DROP SCHEMA IF EXISTS core CASCADE;
         DROP TABLE IF EXISTS public._sqlx_migrations;",
    )
    .execute(&pool)
    .await
    .expect("integration database reset succeeds");
    sqlx::migrate::Migrator::new(repository_root().join("migrations/postgresql"))
        .await
        .expect("migration directory is readable")
        .run(&pool)
        .await
        .expect("R8A migrations apply");
    pool.close().await;
}

#[allow(clippy::too_many_lines)]
fn server_values(
    address: SocketAddr,
    environment: &IntegrationEnvironment,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("FICANT_GRPC_BIND".to_owned(), address.to_string()),
        (
            "FICANT_GRPC_WEB_ALLOWED_ORIGINS".to_owned(),
            ALLOWED_ORIGIN.to_owned(),
        ),
        ("FICANT_PLATFORM_SIGNING_KEY_HEX".to_owned(), KEY.to_owned()),
        ("FICANT_PLATFORM_TRACE_KEY_HEX".to_owned(), KEY.to_owned()),
        (
            "FICANT_CODE_COMMIT_SHA".to_owned(),
            ficant_server::compiled_git_commit_sha().to_owned(),
        ),
        (
            "FICANT_CODE_TREE_SHA".to_owned(),
            ficant_server::compiled_git_tree_sha().to_owned(),
        ),
        (
            "FICANT_SERVER_RUNTIME_IMAGE_DIGEST".to_owned(),
            environment.runtime_digest.clone(),
        ),
        (
            "FICANT_SERVER_ENVIRONMENT_ATTESTATION".to_owned(),
            content_digest(SERVER_TEST_ENVIRONMENT),
        ),
        (
            "FICANT_EXPERIMENT_DATABASE_URL".to_owned(),
            environment.database_url.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_ENDPOINT".to_owned(),
            environment.s3_endpoint.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_BUCKET".to_owned(),
            environment.s3_bucket.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_ACCESS_KEY".to_owned(),
            environment.s3_access_key.clone(),
        ),
        (
            "FICANT_EXPERIMENT_S3_SECRET_KEY".to_owned(),
            environment.s3_secret_key.clone(),
        ),
        (
            "FICANT_EXPERIMENT_CURSOR_KEY_HEX".to_owned(),
            KEY.to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_TENANT_ID".to_owned(),
            TENANT_ID.to_owned(),
        ),
        ("FICANT_EXPERIMENT_OWNER_ID".to_owned(), OWNER_ID.to_owned()),
        ("FICANT_EXPERIMENT_ACTOR_ID".to_owned(), ACTOR_ID.to_owned()),
        (
            "FICANT_EXPERIMENT_RUNTIME_IMAGE_DIGEST".to_owned(),
            environment.runtime_digest.clone(),
        ),
        (
            "FICANT_EXPERIMENT_ENVIRONMENT_ATTESTATION".to_owned(),
            "ficant.worker.environment.v1\narch=amd64\nos=linux\nprofile=test".to_owned(),
        ),
        (
            "FICANT_EXPERIMENT_NATIVE_SOURCE_DIGEST".to_owned(),
            ficant_native_nodes::native_node_source_digest_attestation(),
        ),
        (
            "FICANT_INPUT_FILE_NDJSON_ROOT".to_owned(),
            env::temp_dir()
                .join("ficant-r8a-unused-input")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "FICANT_INPUT_FILE_CONNECTION_BINDING".to_owned(),
            "r8a-unused-file".to_owned(),
        ),
        (
            "FICANT_INPUT_POSTGRES_CONNECTION_BINDING".to_owned(),
            "r8a-unused-postgres".to_owned(),
        ),
        (
            "FICANT_LOOPBACK_SUBJECT".to_owned(),
            "portfolio360-researcher".to_owned(),
        ),
        ("FICANT_LOOPBACK_ACTOR_ID".to_owned(), ACTOR_ID.to_owned()),
        ("FICANT_LOOPBACK_TENANT_ID".to_owned(), TENANT_ID.to_owned()),
        (
            "FICANT_LOOPBACK_ALLOWED_OWNER_IDS".to_owned(),
            OWNER_ID.to_owned(),
        ),
        (
            "FICANT_LOOPBACK_ACTIVE_ROLE".to_owned(),
            "RESEARCHER".to_owned(),
        ),
        ("FICANT_LOOPBACK_SCOPES".to_owned(), SCOPES.to_owned()),
    ])
}

fn assert_server_evidence_bindings(environment: &IntegrationEnvironment) {
    let values = server_values(
        "127.0.0.1:0".parse().expect("fixture address is valid"),
        environment,
    );
    let runtime = values
        .get("FICANT_SERVER_RUNTIME_IMAGE_DIGEST")
        .expect("server runtime binding is configured");
    let attestation = values
        .get("FICANT_SERVER_ENVIRONMENT_ATTESTATION")
        .expect("server environment binding is configured");
    assert_eq!(runtime, &environment.runtime_digest);
    assert_eq!(attestation, &content_digest(SERVER_TEST_ENVIRONMENT));
    assert_ne!(runtime, &format!("sha256:{}", "ab".repeat(32)));
    assert_ne!(attestation, &format!("sha256:{}", "cd".repeat(32)));
}

fn content_digest(value: &str) -> String {
    let digest = ContentHash::digest(value.as_bytes());
    let mut encoded = String::with_capacity(71);
    encoded.push_str("sha256:");
    for byte in digest.as_bytes() {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("String writes cannot fail");
    }
    encoded
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("server remains two levels below repository root")
        .to_path_buf()
}

fn free_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral listener binds");
    let address = listener.local_addr().expect("ephemeral address is known");
    drop(listener);
    address
}

async fn wait_until_listening(address: SocketAddr) {
    for _ in 0..100 {
        if TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("production server did not listen at {address}");
}
