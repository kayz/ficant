#![allow(clippy::too_many_lines)]

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, NaiveTime, Timelike, Utc};
use chrono_tz::Tz;
use ficant_application::ApplicationError;
use ficant_application::ports::{
    AeadCursorCodec, AuthorizedPrincipal, BeginBlobStage, BlobStore, CURVE_POINT_SCHEMA, CursorKey,
    CurveSnapshotMetadataRepository, DefinitionRepository, DefinitionUseCase, DefinitionValue,
    FoundationChangeContext, GovernedAppendDefinitionVersion, GovernedAppendMarketFact,
    GovernedPublishCurveSnapshot, GovernedRegisterSubject, IdempotencyKey, InstrumentDefinition,
    InstrumentSubtype, MarketFact, MarketFactRulePackResolver, MarketFactUnitResolver,
    MarketFactUseCase, PortfolioAnalyticsAuthorityCandidate, PortfolioAnalyticsAuthorityQuery,
    PortfolioAnalyticsAuthorityRepository, PortfolioBondRatesAuthorityCandidate,
    PortfolioCatalogRepository, PortfolioCatalogTemporalScope, PortfolioImmutableSnapshotAuthority,
    PortfolioRatesUnitRole, PortfolioUnitAuthorityBinding, PortfolioValuationAuthorityBinding,
    SnapshotRepository, SnapshotValue, SubjectRepository, VerifyBlobStage,
    market_fact_content_hash, stored_definition_content_hash,
};
use ficant_application::use_cases::data_snapshot::{DataSnapshotPayloads, PublishDataSnapshot};
use ficant_application::use_cases::factor_topology::FactorTopologyUseCase;
use ficant_application::use_cases::position_views::{
    PositionSnapshotPayload, PublishPositionSnapshot,
};
use ficant_contracts::ficant::core::v1 as core_pb;
use ficant_contracts::ficant::market::v1 as market_pb;
use ficant_domain::ContentAddressed;
use ficant_domain::VersionedDefinition;
use ficant_domain::analytics::{
    AnalyticsMode, AnalyticsObjectRef, CalendarRequirement, FixedDecimal,
};
use ficant_domain::governance::{ChangeJustification, PlatformRole, SourceDocumentRef};
use ficant_domain::market::{
    ArtifactInputKind, Bond, BondBusinessDayConvention, BondCouponFrequency,
    BondDayCountConvention, BondPricingTerms, BondTaxAttributes, Calendar, CalendarInput,
    CalendarSession, CurveSnapshot, CurveSnapshotInput, FactSource, IncomeTaxStatus, Instrument,
    InstrumentInput, InstrumentKind, MarketRulePack, MarketRulePackInput, RulePackContent, Unit,
    UnitInput, Valuation, ValuationInput, ValuationValueRole, ValueAddedTaxStatus,
    VerificationStatus,
};
use ficant_domain::portfolio::{
    Benchmark, BenchmarkInput, BenchmarkRef, Book, BookInput, Portfolio, PortfolioDecimalRounding,
    PortfolioGroup, PortfolioGroupInput, PortfolioInput, PortfolioMetricConvention,
    PortfolioMetricConventionInput, PortfolioMetricConventionRef, PortfolioMetricWeighting,
    PortfolioSnapshotBinding, PortfolioStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{
    AccountingBook, AccountingClassification, AccountingClassificationState, CurveNodeDefinition,
    CurveNodeDefinitionInput, CurveNodeRef, CurveRebuildPolicy, DataSnapshot, DataSnapshotInput,
    FactorDefinition, FactorDefinitionInput, FactorTarget, FactorTargetBinding,
    InstrumentFactorTarget, Position, PositionHoldingForm, PositionInput, PositionSnapshot,
    PositionSnapshotInput, SecondOrderPolicy, SensitivityConvention, SensitivityDirection,
};
use ficant_domain::subject::{
    AccessSet, FundingTier, Subject, SubjectRecord, SubjectVersion, TaxTreatment,
};
use ficant_storage::postgres::PostgresRepository;
use ficant_storage::s3::S3BlobStore;
use ficant_tax_pack::{
    MARKET as TAX_MARKET, RULE_TYPE as TAX_RULE_TYPE, SOURCE as TAX_SOURCE,
    TYPE_URL_V2 as TAX_TYPE_URL,
};
use prost::Message;
use serde::Deserialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};

type AnyError = Box<dyn Error + Send + Sync>;

const CATALOG_SCHEMA: &str = "ficant.portfolio360-catalog-fixture.v1";
const ANALYTICS_SCHEMA: &str = "ficant.portfolio360-analytics-fixture.v1";
const ADMIN_ACTOR_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
const SUBJECT_CHANGE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD1";
const POSITION_CHANGE_IDS: [&str; 3] = [
    "01ARZ3NDEKTSV4RRFFQ69G5FD2",
    "01ARZ3NDEKTSV4RRFFQ69G5FD3",
    "01ARZ3NDEKTSV4RRFFQ69G5FD4",
];
const UNIT_CHANGE_IDS: [&str; 9] = [
    "01ARZ3NDEKTSV4RRFFQ69G5FD5",
    "01ARZ3NDEKTSV4RRFFQ69G5FE0",
    "01ARZ3NDEKTSV4RRFFQ69G5FE1",
    "01ARZ3NDEKTSV4RRFFQ69G5FE2",
    "01ARZ3NDEKTSV4RRFFQ69G5FE3",
    "01ARZ3NDEKTSV4RRFFQ69G5FE4",
    "01ARZ3NDEKTSV4RRFFQ69G5FE5",
    "01ARZ3NDEKTSV4RRFFQ69G5FE6",
    "01ARZ3NDEKTSV4RRFFQ69G5FD6",
];
const CALENDAR_CHANGE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FD8";
const INSTRUMENT_CHANGE_IDS: [&str; 3] = [
    "01ARZ3NDEKTSV4RRFFQ69G5FD9",
    "01ARZ3NDEKTSV4RRFFQ69G5FDA",
    "01ARZ3NDEKTSV4RRFFQ69G5FDB",
];
const CURVE_RULE_CHANGE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE7";
const TAX_RULE_CHANGE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE8";
const CURVE_CHANGE_ID: &str = "01ARZ3NDEKTSV4RRFFQ69G5FE9";
const VALUATION_CHANGE_IDS: [&str; 6] = [
    "01ARZ3NDEKTSV4RRFFQ69G5FEA",
    "01ARZ3NDEKTSV4RRFFQ69G5FEB",
    "01ARZ3NDEKTSV4RRFFQ69G5FEC",
    "01ARZ3NDEKTSV4RRFFQ69G5FED",
    "01ARZ3NDEKTSV4RRFFQ69G5FEE",
    "01ARZ3NDEKTSV4RRFFQ69G5FEF",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFixtureSource {
    schema_id: String,
    tenant_id: String,
    owner_id: String,
    subject: VersionSource,
    effective_from: String,
    effective_to: String,
    visible_at: String,
    market_timezone: String,
    book: BookSource,
    group: GroupSource,
    benchmark: BenchmarkSource,
    metric_convention: ConventionSource,
    analytics_authority: CatalogAnalyticsAuthoritySource,
    portfolios: Vec<PortfolioSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogAnalyticsAuthoritySource {
    authority_set_id: String,
    curve_snapshot_id: String,
    data_snapshot_id: String,
    tax_rule_pack: VersionSource,
    valuation: CatalogValuationSource,
    unit_roles: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogValuationSource {
    id: String,
    source_revision: u64,
    scenario_value_index: u32,
    remaining_years_value_index: u32,
    mode: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionSource {
    id: String,
    version: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BookSource {
    id: String,
    version: u64,
    code: String,
    display_name: String,
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupSource {
    id: String,
    version: u64,
    code: String,
    display_name: String,
    status: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BenchmarkSource {
    id: String,
    version: u64,
    code: String,
    display_name: String,
    position_snapshot_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConventionSource {
    id: String,
    version: u64,
    schema_id: String,
    freshness_limit_seconds: u64,
    rounding: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PortfolioSource {
    id: String,
    version: u64,
    code: String,
    display_name: String,
    status: String,
    position_snapshot_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnalyticsFixtureSource {
    schema_id: String,
    currency_unit: VersionSource,
    price_per_100_unit: VersionSource,
    rate_unit: VersionSource,
    years_unit: VersionSource,
    years_squared_unit: VersionSource,
    dv01_per_100_unit: VersionSource,
    dv01_unit: VersionSource,
    dimensionless_unit: VersionSource,
    contract_count_unit: VersionSource,
    calendar: CalendarSource,
    instruments: Vec<InstrumentSource>,
    authority_set_ids: Vec<String>,
    curve_rule_pack: VersionSource,
    curve_family_id: String,
    curve_nodes: Vec<CurveNodeSource>,
    snapshots: Vec<SnapshotSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CalendarSource {
    id: String,
    version: u64,
    market: String,
    effective_from: String,
    effective_to: String,
    session_date: String,
    open_local_time: String,
    close_local_time: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstrumentSource {
    id: String,
    version: u64,
    symbol: String,
    synthetic_yield_to_maturity: String,
    first_issue_date: String,
    current_issue_date: String,
    maturity_date: String,
    coupon_rate: String,
    coupon_frequency: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CurveNodeSource {
    curve_node_id: String,
    factor_id: String,
    tenor: String,
    yield_to_maturity: String,
    bump: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotSource {
    id: String,
    positions: Vec<PositionSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PositionSource {
    position_id: String,
    instrument: VersionSource,
    valuation_id: String,
    yield_to_maturity: String,
    price: String,
    remaining_years: String,
    quantity: String,
    economic_value: String,
    economic_pnl: String,
    accounting_pnl: String,
    capital_requirement: String,
}

struct Fixture {
    owner: OwnerRef,
    subject: SubjectRecord,
    visible_at: MarketTime,
    book: Book,
    group: PortfolioGroup,
    benchmark: Benchmark,
    convention: PortfolioMetricConvention,
    portfolios: Vec<Portfolio>,
    snapshots: Vec<PositionSnapshot>,
    units: Vec<Unit>,
    calendar: Calendar,
    instruments: Vec<InstrumentDefinition>,
    curve_rule_pack: MarketRulePack,
    tax_rule_pack: MarketRulePack,
    factors: Vec<FactorDefinition>,
    curve_nodes: Vec<CurveNodeDefinition>,
    curve_points: Vec<u8>,
    curve_snapshot: CurveSnapshot,
    data_snapshot: DataSnapshot,
    data_parquet: Vec<u8>,
    data_manifest: Vec<u8>,
    valuations: Vec<Valuation>,
    analytics_authorities: Vec<PortfolioAnalyticsAuthorityCandidate>,
}

#[tokio::main]
async fn main() -> Result<(), AnyError> {
    let fixture_path = fixture_argument()?;
    let analytics_path = fixture_path
        .parent()
        .ok_or("catalog fixture must have a parent directory")?
        .join("analytics-p0.json");
    let catalog: CatalogFixtureSource = serde_json::from_slice(&fs::read(&fixture_path)?)?;
    let analytics: AnalyticsFixtureSource = serde_json::from_slice(&fs::read(&analytics_path)?)?;
    let fixture = build_fixture(catalog, &analytics)?;

    let database_url = required_environment("FICANT_BOOTSTRAP_DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;
    let cursor = Arc::new(app(AeadCursorCodec::new(
        app(CursorKey::new("r8a-bootstrap", [0x38; 32]))?,
        Vec::new(),
    ))?);
    let repository = Arc::new(PostgresRepository::new(pool.clone(), cursor));
    let blob_store = Arc::new(app(S3BlobStore::new(
        &required_environment("FICANT_BOOTSTRAP_S3_ENDPOINT")?,
        required_environment("FICANT_BOOTSTRAP_S3_BUCKET")?,
        &required_environment("FICANT_BOOTSTRAP_S3_ACCESS_KEY")?,
        &required_environment("FICANT_BOOTSTRAP_S3_SECRET_KEY")?,
        pool.clone(),
    ))?);

    let principal = administrator(&fixture.owner)?;
    stage(
        "publish governed Subject",
        publish_subject(repository.as_ref(), &principal, &fixture).await,
    )?;
    stage(
        "publish governed Units",
        publish_units(repository.as_ref(), &principal, &fixture).await,
    )?;
    stage(
        "publish governed Calendar",
        publish_calendar(repository.as_ref(), &principal, &fixture).await,
    )?;
    stage(
        "publish governed Bond instruments",
        publish_instruments(repository.as_ref(), &principal, &fixture).await,
    )?;
    stage(
        "publish governed Curve and Tax RulePacks",
        publish_rule_packs(repository.as_ref(), &principal, &fixture).await,
    )?;
    stage(
        "register exact Factor topology",
        publish_factor_topology(repository.as_ref(), &principal, &fixture).await,
    )?;
    stage(
        "publish governed PositionSnapshots",
        publish_position_snapshots(
            repository.as_ref(),
            blob_store.as_ref(),
            &principal,
            &fixture,
        )
        .await,
    )?;
    stage(
        "publish governed CurveSnapshot",
        publish_curve_snapshot(
            repository.as_ref(),
            blob_store.as_ref(),
            &principal,
            &fixture,
        )
        .await,
    )?;
    stage(
        "publish verified DataSnapshot",
        publish_data_snapshot(
            repository.as_ref(),
            blob_store.as_ref(),
            &principal,
            &fixture,
        )
        .await,
    )?;
    stage(
        "publish governed typed Yield Valuations",
        publish_valuations(repository.as_ref(), &principal, &fixture).await,
    )?;
    stage(
        "insert immutable Portfolio catalog and analytics authority",
        insert_catalog(&pool, &fixture).await,
    )?;
    stage(
        "verify exact Portfolio catalog read-back",
        verify_catalog(repository.as_ref(), &principal, &fixture).await,
    )?;
    stage(
        "verify exact Portfolio analytics authority read-back",
        verify_analytics_authorities(repository.as_ref(), &principal, &fixture).await,
    )?;
    println!(
        "Portfolio360 P0 fixture ready: owner={} portfolios={} snapshots={} factors={} valuations={} authorities={} curve={} data={}",
        fixture.owner.owner_id(),
        fixture.portfolios.len(),
        fixture.snapshots.len(),
        fixture.factors.len(),
        fixture.valuations.len(),
        fixture.analytics_authorities.len(),
        fixture.curve_snapshot.id(),
        fixture.data_snapshot.id()
    );
    Ok(())
}

fn stage<T>(name: &str, result: Result<T, AnyError>) -> Result<T, AnyError> {
    result.map_err(|error| std::io::Error::other(format!("{name}: {error}")).into())
}

fn fixture_argument() -> Result<PathBuf, AnyError> {
    let mut arguments = env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--fixture")) {
        return Err("expected --fixture <catalog-p0.json>".into());
    }
    let path = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("missing --fixture value")?;
    if arguments.next().is_some() || !path.is_file() {
        return Err("fixture must be one existing file and no extra arguments are allowed".into());
    }
    Ok(path)
}

fn required_environment(name: &str) -> Result<String, AnyError> {
    env::var(name).map_err(|_| format!("required bootstrap environment {name} is missing").into())
}

fn app<T>(result: Result<T, ApplicationError>) -> Result<T, AnyError> {
    result.map_err(|error| {
        std::io::Error::other(format!(
            "bootstrap application failure: {:?} (retryable={})",
            error.category(),
            error.retryable()
        ))
        .into()
    })
}

fn build_fixture(
    catalog: CatalogFixtureSource,
    analytics: &AnalyticsFixtureSource,
) -> Result<Fixture, AnyError> {
    if catalog.schema_id != CATALOG_SCHEMA
        || analytics.schema_id != ANALYTICS_SCHEMA
        || catalog.portfolios.len() != 2
        || analytics.snapshots.len() != 3
        || analytics.instruments.len() != 3
        || analytics.authority_set_ids.len() != 3
        || analytics.curve_nodes.len() != 3
    {
        return Err("Portfolio360 fixture schema/count invariant failed".into());
    }
    let expected_unit_roles = [
        "CURRENCY_AMOUNT",
        "PRICE_PER_100",
        "RATE",
        "YEARS",
        "YEARS_SQUARED",
        "DV01_PER_100",
        "DV01",
        "DIMENSIONLESS",
        "CONTRACT_COUNT",
    ];
    if catalog.analytics_authority.unit_roles != expected_unit_roles.map(str::to_owned)
        || catalog.analytics_authority.authority_set_id != analytics.authority_set_ids[0]
        || catalog.analytics_authority.valuation.id
            != analytics.snapshots[0].positions[0].valuation_id
        || catalog.analytics_authority.valuation.source_revision != 1
        || catalog.analytics_authority.valuation.scenario_value_index != 0
        || catalog
            .analytics_authority
            .valuation
            .remaining_years_value_index
            != 1
        || catalog.analytics_authority.valuation.mode != "YIELD_IN"
    {
        return Err("Portfolio analytics authority invariant failed".into());
    }
    let owner = OwnerRef::new(ulid(&catalog.tenant_id)?, ulid(&catalog.owner_id)?);
    let subject_ref = version_ref(&catalog.subject)?;
    let effective_from = market_time(&catalog.effective_from, &catalog.market_timezone)?;
    let effective_to = market_time(&catalog.effective_to, &catalog.market_timezone)?;
    let visible_at = market_time(&catalog.visible_at, &catalog.market_timezone)?;
    let observed_at = MarketTime::new(
        visible_at.instant() - chrono::Duration::hours(1),
        &catalog.market_timezone,
        (visible_at.instant() - chrono::Duration::hours(1))
            .with_timezone(&catalog.market_timezone.parse::<Tz>()?)
            .date_naive(),
    )?;
    let subject = SubjectRecord::new(
        Subject::new_owned(
            subject_ref.id().clone(),
            owner.clone(),
            "Portfolio360 P0 Research Subject",
        )?,
        SubjectVersion::new(
            subject_ref.clone(),
            AccessSet::new(["CGB", "CN"], ["bond-analytics", "portfolio360", "rates"])?,
            FundingTier::DrAvailable,
            TaxTreatment::new("cn-vat-general-taxpayer", "cn-cgb-interest-cit-exempt")?,
            "direct",
            "principal",
            None,
        )?,
    )?;
    let currency_unit = unit_ref(&analytics.currency_unit)?;
    let price_per_100_unit = unit_ref(&analytics.price_per_100_unit)?;
    let rate_unit = unit_ref(&analytics.rate_unit)?;
    let years_unit = unit_ref(&analytics.years_unit)?;
    let years_squared_unit = unit_ref(&analytics.years_squared_unit)?;
    let dv01_per_100_unit = unit_ref(&analytics.dv01_per_100_unit)?;
    let dv01_unit = unit_ref(&analytics.dv01_unit)?;
    let dimensionless_unit = unit_ref(&analytics.dimensionless_unit)?;
    let contract_count_unit = unit_ref(&analytics.contract_count_unit)?;
    let unit = |reference: &UnitRef,
                code: &str,
                dimension: &str,
                precision: u32|
     -> Result<Unit, AnyError> {
        Ok(Unit::new(UnitInput {
            unit_id: reference.unit_id().clone(),
            version: reference.version(),
            owner: owner.clone(),
            code: code.to_owned(),
            dimension: dimension.to_owned(),
            scale: 12,
            precision,
        })?)
    };
    let units = vec![
        unit(&currency_unit, "CNY", "currency_amount", 28)?,
        unit(&price_per_100_unit, "PRICE_PER_100", "price_per_100", 28)?,
        unit(&rate_unit, "RATE", "rate", 18)?,
        unit(&years_unit, "YEARS", "years", 28)?,
        unit(&years_squared_unit, "YEARS_SQUARED", "years_squared", 28)?,
        unit(&dv01_per_100_unit, "DV01_PER_100", "dv01_per_100", 28)?,
        unit(&dv01_unit, "DV01", "dv01", 28)?,
        unit(&dimensionless_unit, "DIMENSIONLESS", "dimensionless", 28)?,
        unit(&contract_count_unit, "CONTRACT_COUNT", "contract_count", 28)?,
    ];
    for position in analytics
        .snapshots
        .iter()
        .flat_map(|snapshot| &snapshot.positions)
    {
        let instrument_yield = analytics
            .instruments
            .iter()
            .find(|instrument| {
                instrument.id == position.instrument.id
                    && instrument.version == position.instrument.version
            })
            .map(|instrument| instrument.synthetic_yield_to_maturity.as_str())
            .ok_or("position instrument has no exact synthetic YTM fixture binding")?;
        if position.yield_to_maturity != instrument_yield {
            return Err("position synthetic YTM differs from its exact instrument fixture".into());
        }
        DecimalValue::new(&position.yield_to_maturity, 12, rate_unit.clone())?;
        DecimalValue::new(&position.price, 12, price_per_100_unit.clone())?;
        DecimalValue::new(&position.remaining_years, 12, years_unit.clone())?;
    }
    let calendar_ref = source_ref(&analytics.calendar.id, analytics.calendar.version)?;
    let calendar_effective_from =
        market_time(&analytics.calendar.effective_from, &catalog.market_timezone)?;
    let calendar_effective_to =
        market_time(&analytics.calendar.effective_to, &catalog.market_timezone)?;
    let calendar = Calendar::new(CalendarInput {
        calendar_id: calendar_ref.id().clone(),
        version: calendar_ref.version(),
        owner: owner.clone(),
        market: analytics.calendar.market.clone(),
        market_timezone: catalog.market_timezone.clone(),
        effective: EffectivePeriod::new(calendar_effective_from, calendar_effective_to)?,
        sessions: vec![CalendarSession::open(
            NaiveDate::parse_from_str(&analytics.calendar.session_date, "%Y-%m-%d")?,
            NaiveTime::parse_from_str(&analytics.calendar.open_local_time, "%H:%M:%S")?,
            NaiveTime::parse_from_str(&analytics.calendar.close_local_time, "%H:%M:%S")?,
        )?],
    })?;
    let instruments = analytics
        .instruments
        .iter()
        .map(|source| {
            let reference = source_ref(&source.id, source.version)?;
            let instrument = Instrument::new(InstrumentInput {
                instrument_id: reference.id().clone(),
                version: reference.version(),
                owner: owner.clone(),
                kind: InstrumentKind::Bond,
                market: analytics.calendar.market.clone(),
                symbol: source.symbol.clone(),
                currency: currency_unit.clone(),
                calendar: calendar_ref.clone(),
            })?;
            let frequency = match source.coupon_frequency.as_str() {
                "ANNUAL" => BondCouponFrequency::Annual,
                "SEMIANNUAL" => BondCouponFrequency::Semiannual,
                value => return Err(format!("unsupported coupon frequency {value}").into()),
            };
            let bond = Bond::with_issuance(
                &instrument,
                NaiveDate::parse_from_str(&source.first_issue_date, "%Y-%m-%d")?,
                NaiveDate::parse_from_str(&source.current_issue_date, "%Y-%m-%d")?,
                NaiveDate::parse_from_str(&source.maturity_date, "%Y-%m-%d")?,
                DecimalValue::new("100000000000000", 12, currency_unit.clone())?,
                BondTaxAttributes::new(ValueAddedTaxStatus::Exempt, IncomeTaxStatus::Exempt),
                DecimalValue::new("100000000000000", 12, currency_unit.clone())?,
            )?
            .with_pricing_terms(BondPricingTerms::new(
                DecimalValue::new(&source.coupon_rate, 12, rate_unit.clone())?,
                frequency,
                BondDayCountConvention::ActActBondIsma,
                BondBusinessDayConvention::Following,
            )?)?;
            app(InstrumentDefinition::new(
                instrument,
                Some(InstrumentSubtype::Bond(bond)),
            ))
        })
        .collect::<Result<Vec<_>, AnyError>>()?;
    let snapshots = analytics
        .snapshots
        .iter()
        .map(|source| {
            position_snapshot(
                source,
                &owner,
                &subject_ref,
                &currency_unit,
                &instruments,
                &observed_at,
                &visible_at,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let curve_rule_payload =
        b"ficant.portfolio360.curve-rule.v1\ncurve=cn.gov.cgb.ytm\ninterpolation=linear-zero\n"
            .to_vec();
    let curve_rule_pack = MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: ulid(&analytics.curve_rule_pack.id)?,
            version: Version::new(analytics.curve_rule_pack.version)?,
            owner: owner.clone(),
            market: analytics.calendar.market.clone(),
            rule_type: "yield-curve-construction".to_owned(),
            source: "ficant-authority/portfolio360-curve/v1".to_owned(),
            effective: EffectivePeriod::new(effective_from.clone(), effective_to.clone())?,
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(&curve_rule_payload),
        },
        RulePackContent::new(
            "type.googleapis.com/ficant.market.v1.CurveConstructionRulePackV1",
            curve_rule_payload,
        )?,
    )?;
    let tax_payload =
        include_bytes!("../../../domain-packs/cgb-interest-tax/cgb-interest-tax-v1.bin").to_vec();
    let tax_rule_pack = MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: ulid(&catalog.analytics_authority.tax_rule_pack.id)?,
            version: Version::new(catalog.analytics_authority.tax_rule_pack.version)?,
            owner: owner.clone(),
            market: TAX_MARKET.to_owned(),
            rule_type: TAX_RULE_TYPE.to_owned(),
            source: TAX_SOURCE.to_owned(),
            effective: EffectivePeriod::new(
                market_time("2026-01-01T00:00:00+08:00", &catalog.market_timezone)?,
                market_time("2028-01-01T00:00:00+08:00", &catalog.market_timezone)?,
            )?,
            verification_status: VerificationStatus::Verified,
            content_hash: ContentHash::digest(&tax_payload),
        },
        RulePackContent::new(TAX_TYPE_URL, tax_payload)?,
    )?;
    let factors = analytics
        .curve_nodes
        .iter()
        .map(|source| {
            let mut input = FactorDefinitionInput {
                factor_id: source.factor_id.clone(),
                factor_unit: rate_unit.clone(),
                convention: SensitivityConvention::new(
                    DecimalValue::new(&source.bump, 12, rate_unit.clone())?,
                    SensitivityDirection::Central,
                    CurveRebuildPolicy::Rebuild,
                    SecondOrderPolicy::Exclude,
                )?,
                content_hash: ContentHash::digest(b"pending"),
            };
            input.content_hash = FactorDefinition::content_hash_for(&input);
            FactorDefinition::new(input).map_err(|error| -> AnyError { Box::new(error) })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let curve_nodes = analytics
        .curve_nodes
        .iter()
        .map(|source| {
            let mut input = CurveNodeDefinitionInput {
                curve_node_id: source.curve_node_id.clone(),
                curve_family_id: analytics.curve_family_id.clone(),
                tenor: source.tenor.clone(),
                factor_unit: rate_unit.clone(),
                content_hash: ContentHash::digest(b"pending"),
            };
            input.content_hash = CurveNodeDefinition::content_hash_for(&input);
            CurveNodeDefinition::new(input).map_err(|error| -> AnyError { Box::new(error) })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let curve_points = market_pb::CurvePointSet {
        curve_family_id: analytics.curve_family_id.clone(),
        points: analytics
            .curve_nodes
            .iter()
            .zip(&curve_nodes)
            .map(|(source, node)| market_pb::CurvePoint {
                curve_node_id: node.curve_node_id().to_owned(),
                curve_node_content_hash: Some(core_pb::Sha256 {
                    value: node.content_hash().as_bytes().to_vec(),
                }),
                yield_to_maturity: Some(core_pb::DecimalValue {
                    coefficient: source.yield_to_maturity.clone(),
                    scale: 12,
                    unit: Some(core_pb::UnitRef {
                        unit_id: Some(core_pb::Ulid {
                            value: rate_unit.unit_id().as_str().to_owned(),
                        }),
                        version: rate_unit.version().get(),
                    }),
                }),
            })
            .collect(),
    }
    .encode_to_vec();
    let curve_snapshot = CurveSnapshot::new(CurveSnapshotInput {
        curve_snapshot_id: ulid(&catalog.analytics_authority.curve_snapshot_id)?,
        owner: owner.clone(),
        as_of: observed_at.clone(),
        currency: currency_unit.clone(),
        curve_kind: "YTM".to_owned(),
        calendar: calendar_ref.clone(),
        rule_pack: version_ref(&analytics.curve_rule_pack)?,
        point_schema: CURVE_POINT_SCHEMA.to_owned(),
        content_hash: ContentHash::digest(&curve_points),
        lineage: instruments
            .iter()
            .map(|definition| {
                LineageRef::versioned(
                    definition.instrument().version_ref().id().clone(),
                    definition.instrument().version_ref().version(),
                )
            })
            .collect(),
        input_kind: ArtifactInputKind::ExternalFixture,
    })?
    .with_knowledge_time(visible_at.clone(), analytics.curve_family_id.clone())?;
    let data_parquet =
        b"PAR1ficant.portfolio360.market-input.v1\nCGB-2030-01\nCGB-2032-02\nCGB-2035-03\nPAR1"
            .to_vec();
    let data_manifest = b"ficant.portfolio360.market-input-manifest.v1\nschema=prices-and-reference-data\nrows=3\nas_of=2026-08-21T02:00:00Z\n"
        .to_vec();
    let data_snapshot = DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: ulid(&catalog.analytics_authority.data_snapshot_id)?,
        owner: owner.clone(),
        visible_at: visible_at.clone(),
        as_of: observed_at.clone(),
        schema_hash: ContentHash::digest(b"ficant.portfolio360.market-input-schema.v1"),
        manifest_hash: ContentHash::digest(&data_manifest),
        blob_content_hash: ContentHash::digest(&data_parquet),
        lineage: instruments
            .iter()
            .map(|definition| {
                LineageRef::versioned(
                    definition.instrument().version_ref().id().clone(),
                    definition.instrument().version_ref().version(),
                )
            })
            .collect(),
    })?;
    for authority_set_id in &analytics.authority_set_ids {
        ulid(authority_set_id)?;
    }
    let (valuations, analytics_authorities) = build_analytics_authorities(
        &catalog,
        analytics,
        &owner,
        &subject_ref,
        &effective_from,
        &effective_to,
        &visible_at,
        &observed_at,
        &rate_unit,
        &years_unit,
        &units,
        &snapshots,
        &curve_rule_pack,
        &tax_rule_pack,
        &curve_snapshot,
        &data_snapshot,
    )?;
    let snapshot = |id: &str| {
        snapshots
            .iter()
            .find(|value| value.id().as_str() == id)
            .ok_or_else(|| format!("catalog snapshot {id} is absent from analytics fixture"))
    };

    let mut book_input = BookInput {
        book: source_ref(&catalog.book.id, catalog.book.version)?,
        owner: owner.clone(),
        subject_ref: subject_ref.clone(),
        code: catalog.book.code,
        display_name: catalog.book.display_name,
        status: status(&catalog.book.status)?,
        effective_from: effective_from.clone(),
        effective_to: effective_to.clone(),
        content_hash: ContentHash::digest(b"pending"),
    };
    book_input.content_hash = Book::content_hash_for(&book_input);
    let book = Book::new(book_input)?;
    let book_ref = exact_lineage(&book)?;

    let mut group_input = PortfolioGroupInput {
        group: source_ref(&catalog.group.id, catalog.group.version)?,
        owner: owner.clone(),
        subject_ref: subject_ref.clone(),
        book: book_ref.clone(),
        parent_group: None,
        code: catalog.group.code,
        display_name: catalog.group.display_name,
        status: status(&catalog.group.status)?,
        effective_from: effective_from.clone(),
        effective_to: effective_to.clone(),
        content_hash: ContentHash::digest(b"pending"),
    };
    group_input.content_hash = PortfolioGroup::content_hash_for(&group_input);
    let group = PortfolioGroup::new(group_input)?;
    let group_ref = exact_lineage(&group)?;

    let benchmark_snapshot = snapshot(&catalog.benchmark.position_snapshot_id)?;
    let mut benchmark_input = BenchmarkInput {
        benchmark: source_ref(&catalog.benchmark.id, catalog.benchmark.version)?,
        owner: owner.clone(),
        subject_ref: subject_ref.clone(),
        code: catalog.benchmark.code,
        display_name: catalog.benchmark.display_name,
        position_snapshot: snapshot_binding(benchmark_snapshot)?,
        effective_from: effective_from.clone(),
        effective_to: effective_to.clone(),
        content_hash: ContentHash::digest(b"pending"),
    };
    benchmark_input.content_hash = Benchmark::content_hash_for(&benchmark_input);
    let benchmark = Benchmark::new(benchmark_input)?;

    if catalog.metric_convention.rounding != "TIES_TO_EVEN" {
        return Err("R8A convention rounding must be TIES_TO_EVEN".into());
    }
    let mut convention_input = PortfolioMetricConventionInput {
        convention: source_ref(
            &catalog.metric_convention.id,
            catalog.metric_convention.version,
        )?,
        owner: owner.clone(),
        schema_id: catalog.metric_convention.schema_id,
        ytm_weighting: PortfolioMetricWeighting::MarketValueTimesModifiedDuration,
        duration_weighting: PortfolioMetricWeighting::MarketValue,
        convexity_weighting: PortfolioMetricWeighting::MarketValue,
        coupon_weighting: PortfolioMetricWeighting::Notional,
        remaining_life_weighting: PortfolioMetricWeighting::Notional,
        rounding: PortfolioDecimalRounding::TiesToEven,
        freshness_limit_seconds: catalog.metric_convention.freshness_limit_seconds,
        effective_from: effective_from.clone(),
        effective_to: effective_to.clone(),
        content_hash: ContentHash::digest(b"pending"),
    };
    convention_input.content_hash = PortfolioMetricConvention::content_hash_for(&convention_input);
    let convention = PortfolioMetricConvention::new(convention_input)?;

    let portfolios = catalog
        .portfolios
        .iter()
        .map(|source| {
            let position_snapshot = snapshot(&source.position_snapshot_id)?;
            let mut input = PortfolioInput {
                portfolio: source_ref(&source.id, source.version)?,
                owner: owner.clone(),
                subject_ref: subject_ref.clone(),
                book: book_ref.clone(),
                group: group_ref.clone(),
                code: source.code.clone(),
                display_name: source.display_name.clone(),
                status: status(&source.status)?,
                position_snapshot: snapshot_binding(position_snapshot)?,
                benchmark: BenchmarkRef::new(
                    benchmark.reference().clone(),
                    benchmark.content_hash().clone(),
                ),
                metric_convention: PortfolioMetricConventionRef::new(
                    convention.reference().clone(),
                    convention.content_hash().clone(),
                ),
                effective_from: effective_from.clone(),
                effective_to: effective_to.clone(),
                content_hash: ContentHash::digest(b"pending"),
            };
            input.content_hash = Portfolio::content_hash_for(&input);
            Portfolio::new(input).map_err(|error| -> AnyError { Box::new(error) })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Fixture {
        owner,
        subject,
        visible_at,
        book,
        group,
        benchmark,
        convention,
        portfolios,
        snapshots,
        units,
        calendar,
        instruments,
        curve_rule_pack,
        tax_rule_pack,
        factors,
        curve_nodes,
        curve_points,
        curve_snapshot,
        data_snapshot,
        data_parquet,
        data_manifest,
        valuations,
        analytics_authorities,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_analytics_authorities(
    catalog: &CatalogFixtureSource,
    analytics: &AnalyticsFixtureSource,
    owner: &OwnerRef,
    subject_ref: &VersionRef,
    effective_from: &MarketTime,
    effective_to: &MarketTime,
    visible_at: &MarketTime,
    valuation_at: &MarketTime,
    rate_unit: &UnitRef,
    years_unit: &UnitRef,
    units: &[Unit],
    snapshots: &[PositionSnapshot],
    curve_rule_pack: &MarketRulePack,
    tax_rule_pack: &MarketRulePack,
    curve_snapshot: &CurveSnapshot,
    data_snapshot: &DataSnapshot,
) -> Result<(Vec<Valuation>, Vec<PortfolioAnalyticsAuthorityCandidate>), AnyError> {
    let role_order = [
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
    if units.len() != role_order.len()
        || analytics.snapshots.len() != snapshots.len()
        || analytics.authority_set_ids.len() != snapshots.len()
    {
        return Err("analytics authority fixture cardinality differs".into());
    }
    let unit_bindings = role_order
        .into_iter()
        .zip(units)
        .map(|(role, unit)| PortfolioUnitAuthorityBinding {
            role,
            reference: UnitRef::new(
                Ulid::new(unit.identity().to_owned()).expect("validated Unit identity"),
                Version::new(unit.version()).expect("validated Unit version"),
            ),
            content_hash: stored_definition_content_hash(&DefinitionValue::Unit(unit.clone())),
        })
        .collect::<Vec<_>>();
    let tax_rule_pack = AnalyticsObjectRef::new(
        VersionRef::new(
            Ulid::new(tax_rule_pack.identity().to_owned()).expect("validated RulePack identity"),
            Version::new(tax_rule_pack.version()).expect("validated RulePack version"),
        ),
        stored_definition_content_hash(&DefinitionValue::MarketRulePack(tax_rule_pack.clone())),
    );
    let curve_rule_pack_ref = VersionRef::new(
        Ulid::new(curve_rule_pack.identity().to_owned()).expect("validated RulePack identity"),
        Version::new(curve_rule_pack.version()).expect("validated RulePack version"),
    );
    let mut valuations = Vec::new();
    let mut authorities = Vec::with_capacity(snapshots.len());
    for ((source, snapshot), authority_set_id) in analytics
        .snapshots
        .iter()
        .zip(snapshots)
        .zip(&analytics.authority_set_ids)
    {
        if source.id != snapshot.id().as_str()
            || source.positions.len() != snapshot.positions().len()
        {
            return Err("analytics authority PositionSnapshot binding differs".into());
        }
        let mut bond_rates = Vec::with_capacity(source.positions.len());
        for (source_position, position) in source.positions.iter().zip(snapshot.positions()) {
            if source_position.position_id != position.id().as_str()
                || version_ref(&source_position.instrument)? != *position.instrument_ref()
            {
                return Err("analytics authority Position binding differs".into());
            }
            let valuation = Valuation::new_with_value_roles(
                ValuationInput {
                    valuation_id: ulid(&source_position.valuation_id)?,
                    instrument: position.instrument_ref().clone(),
                    owner: owner.clone(),
                    source: FactSource::new(
                        "r8a-synthetic-fixture",
                        format!("portfolio360-yield-{}", source_position.valuation_id),
                        catalog.analytics_authority.valuation.source_revision,
                    )?,
                    valuation_at: valuation_at.clone(),
                    method: "ficant.r8a.synthetic-yield-fixture.v1".to_owned(),
                    rule_pack: curve_rule_pack_ref.clone(),
                    values: vec![
                        DecimalValue::new(
                            &source_position.yield_to_maturity,
                            12,
                            rate_unit.clone(),
                        )?,
                        DecimalValue::new(
                            &source_position.remaining_years,
                            12,
                            years_unit.clone(),
                        )?,
                    ],
                    supersedes_id: None,
                },
                vec![
                    ValuationValueRole::Yield,
                    ValuationValueRole::RemainingYears,
                ],
            )?;
            let fact = MarketFact::Valuation(valuation.clone());
            bond_rates.push(PortfolioBondRatesAuthorityCandidate {
                position_id: position.id().clone(),
                instrument_ref: position.instrument_ref().clone(),
                valuation: PortfolioValuationAuthorityBinding {
                    valuation_id: valuation.id().clone(),
                    source_revision: valuation.source().source_revision(),
                    content_hash: market_fact_content_hash(&fact),
                    value_index: catalog.analytics_authority.valuation.scenario_value_index,
                },
                remaining_years_value_index: catalog
                    .analytics_authority
                    .valuation
                    .remaining_years_value_index,
                mode: AnalyticsMode::YieldIn,
                input_value: FixedDecimal::from_scaled(
                    source_position.yield_to_maturity.parse::<i128>()?,
                ),
                remaining_years: FixedDecimal::from_scaled(
                    source_position.remaining_years.parse::<i128>()?,
                ),
                settlement_date: valuation_at.local_trading_date(),
                calendar_requirement: CalendarRequirement::ExactMarket,
            });
            valuations.push(valuation);
        }
        let mut candidate = PortfolioAnalyticsAuthorityCandidate {
            authority_set_id: ulid(authority_set_id)?,
            owner: owner.clone(),
            subject_ref: subject_ref.clone(),
            position_snapshot: PortfolioImmutableSnapshotAuthority {
                id: snapshot.id().clone(),
                content_hash: snapshot.content_hash().clone(),
            },
            curve_snapshot: PortfolioImmutableSnapshotAuthority {
                id: curve_snapshot.id().clone(),
                content_hash: curve_snapshot.content_hash().clone(),
            },
            data_snapshot: PortfolioImmutableSnapshotAuthority {
                id: data_snapshot.id().clone(),
                content_hash: data_snapshot.content_hash().clone(),
            },
            futures_data_snapshot: None,
            tax_rule_pack: tax_rule_pack.clone(),
            effective_from: effective_from.clone(),
            effective_to: effective_to.clone(),
            visible_at: visible_at.clone(),
            units: unit_bindings.clone(),
            bond_rates,
            content_hash: ContentHash::digest(b"pending-r8a-analytics-authority"),
        };
        candidate.content_hash = candidate.canonical_content_hash();
        authorities.push(candidate);
    }
    if valuations.len() != VALUATION_CHANGE_IDS.len() {
        return Err("R8A fixture must bind exactly six typed Valuations".into());
    }
    Ok((valuations, authorities))
}

#[allow(clippy::too_many_arguments)]
fn position_snapshot(
    source: &SnapshotSource,
    owner: &OwnerRef,
    subject_ref: &VersionRef,
    currency_unit: &UnitRef,
    instruments: &[InstrumentDefinition],
    observed_at: &MarketTime,
    visible_at: &MarketTime,
) -> Result<PositionSnapshot, AnyError> {
    let positions = source
        .positions
        .iter()
        .map(|value| {
            Position::new(PositionInput {
                position_id: ulid(&value.position_id)?,
                instrument_ref: version_ref(&value.instrument)?,
                quantity: DecimalValue::new(&value.quantity, 12, currency_unit.clone())?,
                economic_value: DecimalValue::new(
                    &value.economic_value,
                    12,
                    currency_unit.clone(),
                )?,
                economic_pnl: DecimalValue::new(&value.economic_pnl, 12, currency_unit.clone())?,
                accounting_pnl: DecimalValue::new(
                    &value.accounting_pnl,
                    12,
                    currency_unit.clone(),
                )?,
                capital_requirement: DecimalValue::new(
                    &value.capital_requirement,
                    12,
                    currency_unit.clone(),
                )?,
                accounting_classification: AccountingClassification::new(
                    AccountingClassificationState::Classified,
                    Some(AccountingBook::Fvtpl),
                )?,
                holding_form: PositionHoldingForm::Owned,
            })
            .map_err(|error| -> AnyError { Box::new(error) })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let lineage = positions
        .iter()
        .map(|position| {
            let _definition = instruments
                .iter()
                .find(|definition| {
                    definition.instrument().version_ref() == *position.instrument_ref()
                })
                .ok_or("position instrument is absent from the fixture definitions")?;
            Ok::<LineageRef, AnyError>(LineageRef::versioned(
                position.instrument_ref().id().clone(),
                position.instrument_ref().version(),
            ))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut input = PositionSnapshotInput {
        snapshot_id: ulid(&source.id)?,
        owner: owner.clone(),
        subject_ref: subject_ref.clone(),
        observed_at: observed_at.clone(),
        visible_at: visible_at.clone(),
        content_hash: ContentHash::digest(b"pending"),
        lineage,
        positions,
    };
    input.content_hash = PositionSnapshot::content_hash_for(&input);
    Ok(PositionSnapshot::new(input)?)
}

fn administrator(owner: &OwnerRef) -> Result<AuthorizedPrincipal, AnyError> {
    app(AuthorizedPrincipal::new(
        "portfolio360-bootstrap".to_owned(),
        ulid(ADMIN_ACTOR_ID)?,
        owner.tenant_id().clone(),
        vec![owner.owner_id().clone()],
        PlatformRole::PlatformAdmin,
        vec![
            "definitions:write".to_owned(),
            "facts:write".to_owned(),
            "positions:write".to_owned(),
            "registry:write".to_owned(),
        ],
        ContentHash::digest(b"portfolio360-bootstrap-local-principal"),
    ))
}

async fn publish_subject(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    if let Some(existing) = app(repository
        .get_subject(fixture.subject.version().reference().clone())
        .await)?
    {
        if existing != fixture.subject {
            return Err("existing Portfolio360 Subject differs from the fixture".into());
        }
        return Ok(());
    }
    let context = change_context(
        principal,
        SUBJECT_CHANGE_ID,
        "bootstrap Portfolio360 P0 Subject",
        &fixture.visible_at,
    )?;
    let command = app(GovernedRegisterSubject::new(
        context,
        fixture.subject.clone(),
        app(IdempotencyKey::new("r8a-portfolio360-subject-v1"))?,
    ))?;
    let stored = app(repository.register_governed_subject(command).await)?;
    if stored != fixture.subject {
        return Err("existing Portfolio360 Subject differs from the fixture".into());
    }
    Ok(())
}

async fn publish_units(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    for (index, unit) in fixture.units.iter().enumerate() {
        publish_definition(
            repository,
            principal,
            &fixture.visible_at,
            UNIT_CHANGE_IDS[index],
            &format!(
                "r8a-portfolio360-unit-{}-v{}",
                unit.identity(),
                unit.version()
            ),
            DefinitionValue::Unit(unit.clone()),
        )
        .await?;
    }
    Ok(())
}

async fn publish_calendar(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    publish_definition(
        repository,
        principal,
        &fixture.visible_at,
        CALENDAR_CHANGE_ID,
        "r8a-portfolio360-calendar-v1",
        DefinitionValue::Calendar(fixture.calendar.clone()),
    )
    .await
}

async fn publish_instruments(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    for (index, instrument) in fixture.instruments.iter().enumerate() {
        publish_definition(
            repository,
            principal,
            &fixture.visible_at,
            INSTRUMENT_CHANGE_IDS[index],
            &format!("r8a-portfolio360-instrument-{}", instrument.identity()),
            DefinitionValue::Instrument(instrument.clone()),
        )
        .await?;
    }
    Ok(())
}

async fn publish_rule_packs(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    publish_definition(
        repository,
        principal,
        &fixture.visible_at,
        CURVE_RULE_CHANGE_ID,
        "r8a-portfolio360-curve-rule-v1",
        DefinitionValue::MarketRulePack(fixture.curve_rule_pack.clone()),
    )
    .await?;
    publish_definition(
        repository,
        principal,
        &fixture.visible_at,
        TAX_RULE_CHANGE_ID,
        "r8a-portfolio360-tax-rule-v1",
        DefinitionValue::MarketRulePack(fixture.tax_rule_pack.clone()),
    )
    .await
}

async fn publish_factor_topology(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    let topology = FactorTopologyUseCase::new(repository);
    for factor in &fixture.factors {
        let stored = app(topology
            .register_factor_definition(
                principal.access_scope(),
                factor.clone(),
                app(IdempotencyKey::new(format!(
                    "r8a-portfolio360-factor-{}",
                    factor.factor_id()
                )))?,
            )
            .await)?;
        if stored != *factor {
            return Err(format!("existing Factor {} differs", factor.factor_id()).into());
        }
    }
    for node in &fixture.curve_nodes {
        let stored = app(topology
            .register_curve_node_definition(
                principal.access_scope(),
                node.clone(),
                app(IdempotencyKey::new(format!(
                    "r8a-portfolio360-curve-node-{}",
                    node.curve_node_id()
                )))?,
            )
            .await)?;
        if stored != *node {
            return Err(format!("existing CurveNode {} differs", node.curve_node_id()).into());
        }
    }
    for (factor, node) in fixture.factors.iter().zip(&fixture.curve_nodes) {
        let node_binding = FactorTargetBinding::new(
            factor.factor_id(),
            FactorTarget::CurveNode(CurveNodeRef::new(
                node.curve_node_id(),
                node.content_hash().clone(),
            )?),
        )?;
        bind_factor(&topology, principal, node_binding).await?;
        for definition in &fixture.instruments {
            let instrument_binding = FactorTargetBinding::new(
                factor.factor_id(),
                FactorTarget::Instrument(InstrumentFactorTarget::new(
                    fixture.owner.clone(),
                    definition.instrument().version_ref(),
                )),
            )?;
            bind_factor(&topology, principal, instrument_binding).await?;
        }
    }
    Ok(())
}

async fn bind_factor(
    topology: &FactorTopologyUseCase<'_>,
    principal: &AuthorizedPrincipal,
    binding: FactorTargetBinding,
) -> Result<(), AnyError> {
    let key = format!(
        "r8a-portfolio360-factor-binding-{}-{}",
        binding.factor_id(),
        hash_hex(binding.content_hash())
    );
    let stored = app(topology
        .bind_factor_target(
            principal.access_scope(),
            binding.clone(),
            app(IdempotencyKey::new(key))?,
        )
        .await)?;
    if stored != binding {
        return Err("existing Factor target binding differs".into());
    }
    Ok(())
}

async fn publish_curve_snapshot(
    repository: &PostgresRepository,
    blob_store: &S3BlobStore,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    let size = u64::try_from(fixture.curve_points.len())?;
    if let Some(existing) = app(repository
        .get_curve_snapshot_metadata(
            principal.access_scope(),
            fixture.curve_snapshot.id().clone(),
        )
        .await)?
    {
        if existing.snapshot() != &fixture.curve_snapshot || existing.blob_size() != size {
            return Err("existing CurveSnapshot differs from the fixture".into());
        }
        return Ok(());
    }
    let staged = app(blob_store
        .begin_stage(app(BeginBlobStage::new(
            principal.access_scope().clone(),
            fixture.owner.clone(),
            size,
            app(IdempotencyKey::new("r8a-portfolio360-curve-points-stage"))?,
        ))?)
        .await)?;
    app(blob_store
        .append_chunk(
            principal.access_scope(),
            &staged,
            fixture.curve_points.clone(),
        )
        .await)?;
    let verified = app(blob_store
        .verify_and_promote(app(VerifyBlobStage::new(
            principal.access_scope().clone(),
            staged,
            fixture.curve_snapshot.content_hash().clone(),
            size,
        ))?)
        .await)?;
    let command = app(GovernedPublishCurveSnapshot::new(
        change_context(
            principal,
            CURVE_CHANGE_ID,
            "bootstrap Portfolio360 P0 CurveSnapshot",
            &fixture.visible_at,
        )?,
        fixture.curve_snapshot.clone(),
        size,
        verified,
        app(IdempotencyKey::new("r8a-portfolio360-curve-snapshot-v1"))?,
    ))?;
    let stored = app(MarketFactUseCase::new(repository)
        .publish_curve_governed(command)
        .await)?;
    if stored != fixture.curve_snapshot {
        return Err("existing CurveSnapshot differs from the fixture".into());
    }
    Ok(())
}

async fn publish_data_snapshot(
    repository: &PostgresRepository,
    blob_store: &S3BlobStore,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    if let Some(existing) = app(repository
        .get_by_id(principal.access_scope(), fixture.data_snapshot.id().clone())
        .await)?
    {
        match existing {
            SnapshotValue::Data(existing) if existing == fixture.data_snapshot => return Ok(()),
            SnapshotValue::Data(_)
            | SnapshotValue::DataHealthThresholdProfile(_)
            | SnapshotValue::Position(_)
            | SnapshotValue::Universe(_) => {
                return Err("existing DataSnapshot differs from the fixture".into());
            }
        }
    }
    let payloads = app(DataSnapshotPayloads::new(
        fixture.data_snapshot.clone(),
        fixture.data_parquet.clone(),
        fixture.data_manifest.clone(),
        app(IdempotencyKey::new("r8a-portfolio360-data-snapshot-v1"))?,
    ))?;
    let stored = app(PublishDataSnapshot::new(blob_store, repository)
        .execute(principal.access_scope(), payloads)
        .await)?;
    if stored != fixture.data_snapshot {
        return Err("existing DataSnapshot differs from the fixture".into());
    }
    Ok(())
}

async fn publish_valuations(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    let unit_resolver = MarketFactUnitResolver::new(repository);
    let rule_resolver = MarketFactRulePackResolver::new(repository);
    let facts = MarketFactUseCase::new(repository);
    for (index, valuation) in fixture.valuations.iter().enumerate() {
        let fact = MarketFact::Valuation(valuation.clone());
        let unit_validated = app(unit_resolver
            .resolve(principal.access_scope(), fact.clone())
            .await)?;
        let fully_validated = app(rule_resolver
            .resolve(principal.access_scope(), unit_validated)
            .await)?;
        let command = app(GovernedAppendMarketFact::new(
            change_context(
                principal,
                VALUATION_CHANGE_IDS[index],
                "bootstrap R8A synthetic Yield Valuation fact",
                &fixture.visible_at,
            )?,
            fully_validated,
            app(IdempotencyKey::new(format!(
                "r8a-portfolio360-yield-valuation-{}-v{}",
                valuation.id(),
                valuation.source().source_revision()
            )))?,
        ))?;
        let stored = app(facts.append_governed(command).await)?;
        if stored != fact {
            return Err(format!("existing typed Valuation {} differs", valuation.id()).into());
        }
    }
    Ok(())
}

async fn publish_definition(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    visible_at: &MarketTime,
    change_id: &str,
    idempotency_key: &str,
    value: DefinitionValue,
) -> Result<(), AnyError> {
    if let Some(existing) = app(repository
        .get_version(
            principal.access_scope(),
            ulid(value.identity())?,
            Version::new(value.version())?,
        )
        .await)?
    {
        if existing != value {
            return Err(format!("existing definition {} differs", value.identity()).into());
        }
        return Ok(());
    }
    let use_case = DefinitionUseCase::new(repository);
    let context = change_context(
        principal,
        change_id,
        "bootstrap Portfolio360 P0 immutable market definition",
        visible_at,
    )?;
    let command = app(GovernedAppendDefinitionVersion::new(
        context,
        None,
        value.clone(),
        app(IdempotencyKey::new(idempotency_key))?,
    ))?;
    let stored = app(use_case.append(command).await)?;
    if stored != value {
        return Err(format!("existing definition {} differs", value.identity()).into());
    }
    Ok(())
}

async fn publish_position_snapshots(
    repository: &PostgresRepository,
    blob_store: &S3BlobStore,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    let snapshots: &dyn SnapshotRepository = repository;
    let blobs: &dyn BlobStore = blob_store;
    let publisher = PublishPositionSnapshot::new(blobs, snapshots);
    for (index, snapshot) in fixture.snapshots.iter().enumerate() {
        if let Some(existing) = app(repository
            .get_by_id(principal.access_scope(), snapshot.id().clone())
            .await)?
        {
            match existing {
                SnapshotValue::Position(existing) if existing == *snapshot => continue,
                SnapshotValue::Position(_)
                | SnapshotValue::Data(_)
                | SnapshotValue::DataHealthThresholdProfile(_)
                | SnapshotValue::Universe(_) => {
                    return Err(
                        format!("existing PositionSnapshot {} differs", snapshot.id()).into(),
                    );
                }
            }
        }
        let context = change_context(
            principal,
            POSITION_CHANGE_IDS[index],
            "bootstrap Portfolio360 P0 PositionSnapshot",
            &fixture.visible_at,
        )?;
        let payload = app(PositionSnapshotPayload::new(
            snapshot.clone(),
            app(IdempotencyKey::new(format!(
                "r8a-portfolio360-position-{}",
                snapshot.id()
            )))?,
        ))?;
        let stored = app(publisher.execute(context, payload).await)?;
        if stored != *snapshot {
            return Err(format!("existing PositionSnapshot {} differs", snapshot.id()).into());
        }
    }
    Ok(())
}

fn change_context(
    principal: &AuthorizedPrincipal,
    record_id: &str,
    reason: &str,
    occurred_at: &MarketTime,
) -> Result<FoundationChangeContext, AnyError> {
    app(FoundationChangeContext::administrator(
        principal.clone(),
        ChangeJustification::new(
            reason,
            vec![SourceDocumentRef::new(
                "urn:ficant:r8a:portfolio360-p0-fixture",
                ContentHash::digest(b"ficant.portfolio360.fixture.v1"),
            )?],
        )?,
        ulid(record_id)?,
        occurred_at.clone(),
    ))
}

async fn insert_catalog(pool: &PgPool, fixture: &Fixture) -> Result<(), AnyError> {
    let mut transaction = pool.begin().await?;
    insert_book(&mut transaction, fixture).await?;
    insert_group(&mut transaction, fixture).await?;
    insert_benchmark(&mut transaction, fixture).await?;
    insert_convention(&mut transaction, fixture).await?;
    for portfolio in &fixture.portfolios {
        insert_portfolio(&mut transaction, fixture, portfolio).await?;
    }
    for authority in &fixture.analytics_authorities {
        insert_analytics_authority(&mut transaction, authority).await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn insert_book(
    transaction: &mut Transaction<'_, Postgres>,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    let value = &fixture.book;
    let mut query = sqlx::query(
        "INSERT INTO portfolio.books
         (tenant_id,book_id,version,owner_id,subject_id,subject_version,code,display_name,status,
          effective_from,effective_from_nanos,effective_from_timezone,effective_from_local_date,
          effective_to,effective_to_nanos,effective_to_timezone,effective_to_local_date,
          visible_at,visible_at_nanos,visible_at_timezone,visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)
         ON CONFLICT DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get())?)
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get())?)
    .bind(value.code())
    .bind(value.display_name())
    .bind(status_name(value.status()));
    query = bind_time(query, value.effective_from())?;
    query = bind_time(query, value.effective_to())?;
    query = bind_time(query, &fixture.visible_at)?;
    query
        .bind(hash_hex(value.content_hash()))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn insert_group(
    transaction: &mut Transaction<'_, Postgres>,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    let value = &fixture.group;
    let mut query = sqlx::query(
        "INSERT INTO portfolio.groups
         (tenant_id,group_id,version,owner_id,subject_id,subject_version,
          book_id,book_version,book_hash,parent_group_id,parent_group_version,parent_group_hash,
          code,display_name,status,effective_from,effective_from_nanos,effective_from_timezone,
          effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
          effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
          visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                 $21,$22,$23,$24,$25,$26,$27,$28) ON CONFLICT DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get())?)
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get())?)
    .bind(value.book().object_id().as_str())
    .bind(i64::try_from(
        value.book().version().ok_or("missing book version")?.get(),
    )?)
    .bind(hash_hex(
        value.book().content_hash().ok_or("missing book hash")?,
    ))
    .bind(Option::<&str>::None)
    .bind(Option::<i64>::None)
    .bind(Option::<String>::None)
    .bind(value.code())
    .bind(value.display_name())
    .bind(status_name(value.status()));
    query = bind_time(query, value.effective_from())?;
    query = bind_time(query, value.effective_to())?;
    query = bind_time(query, &fixture.visible_at)?;
    query
        .bind(hash_hex(value.content_hash()))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn insert_benchmark(
    transaction: &mut Transaction<'_, Postgres>,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    let value = &fixture.benchmark;
    let snapshot = value.position_snapshot();
    let mut query = sqlx::query(
        "INSERT INTO portfolio.benchmarks
         (tenant_id,benchmark_id,version,owner_id,subject_id,subject_version,code,display_name,
          snapshot_id,snapshot_hash,snapshot_observed_at,snapshot_observed_at_nanos,
          snapshot_observed_at_timezone,snapshot_observed_at_local_date,snapshot_visible_at,
          snapshot_visible_at_nanos,snapshot_visible_at_timezone,snapshot_visible_at_local_date,
          effective_from,effective_from_nanos,effective_from_timezone,effective_from_local_date,
          effective_to,effective_to_nanos,effective_to_timezone,effective_to_local_date,
          visible_at,visible_at_nanos,visible_at_timezone,visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                 $21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31) ON CONFLICT DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get())?)
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get())?)
    .bind(value.code())
    .bind(value.display_name())
    .bind(snapshot.snapshot_id().as_str())
    .bind(hash_hex(snapshot.content_hash()));
    query = bind_time(query, snapshot.observed_at())?;
    query = bind_time(query, snapshot.visible_at())?;
    query = bind_time(query, value.effective_from())?;
    query = bind_time(query, value.effective_to())?;
    query = bind_time(query, &fixture.visible_at)?;
    query
        .bind(hash_hex(value.content_hash()))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn insert_convention(
    transaction: &mut Transaction<'_, Postgres>,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    let value = &fixture.convention;
    let mut query = sqlx::query(
        "INSERT INTO portfolio.metric_conventions
         (tenant_id,convention_id,version,owner_id,schema_id,ytm_weighting,duration_weighting,
          convexity_weighting,coupon_weighting,remaining_life_weighting,rounding,
          freshness_limit_seconds,effective_from,effective_from_nanos,effective_from_timezone,
          effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
          effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
          visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,'MARKET_VALUE_TIMES_MODIFIED_DURATION','MARKET_VALUE',
                 'MARKET_VALUE','NOTIONAL','NOTIONAL','TIES_TO_EVEN',$6,$7,$8,$9,$10,$11,$12,
                 $13,$14,$15,$16,$17,$18,$19) ON CONFLICT DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get())?)
    .bind(value.owner().owner_id().as_str())
    .bind(value.schema_id())
    .bind(i64::try_from(value.freshness_limit_seconds())?);
    query = bind_time(query, value.effective_from())?;
    query = bind_time(query, value.effective_to())?;
    query = bind_time(query, &fixture.visible_at)?;
    query
        .bind(hash_hex(value.content_hash()))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn insert_portfolio(
    transaction: &mut Transaction<'_, Postgres>,
    fixture: &Fixture,
    value: &Portfolio,
) -> Result<(), AnyError> {
    let snapshot = value.position_snapshot();
    let mut query = sqlx::query(
        "INSERT INTO portfolio.portfolios
         (tenant_id,portfolio_id,version,owner_id,subject_id,subject_version,
          book_id,book_version,book_hash,group_id,group_version,group_hash,code,display_name,status,
          snapshot_id,snapshot_hash,snapshot_observed_at,snapshot_observed_at_nanos,
          snapshot_observed_at_timezone,snapshot_observed_at_local_date,snapshot_visible_at,
          snapshot_visible_at_nanos,snapshot_visible_at_timezone,snapshot_visible_at_local_date,
          benchmark_id,benchmark_version,benchmark_hash,convention_id,convention_version,
          convention_hash,effective_from,effective_from_nanos,effective_from_timezone,
          effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
          effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
          visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                 $21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34,$35,$36,$37,$38,
                 $39,$40,$41,$42,$43,$44) ON CONFLICT DO NOTHING",
    )
    .bind(value.owner().tenant_id().as_str())
    .bind(value.reference().id().as_str())
    .bind(i64::try_from(value.reference().version().get())?)
    .bind(value.owner().owner_id().as_str())
    .bind(value.subject_ref().id().as_str())
    .bind(i64::try_from(value.subject_ref().version().get())?)
    .bind(value.book().object_id().as_str())
    .bind(i64::try_from(
        value.book().version().ok_or("missing book version")?.get(),
    )?)
    .bind(hash_hex(
        value.book().content_hash().ok_or("missing book hash")?,
    ))
    .bind(value.group().object_id().as_str())
    .bind(i64::try_from(
        value
            .group()
            .version()
            .ok_or("missing group version")?
            .get(),
    )?)
    .bind(hash_hex(
        value.group().content_hash().ok_or("missing group hash")?,
    ))
    .bind(value.code())
    .bind(value.display_name())
    .bind(status_name(value.status()))
    .bind(snapshot.snapshot_id().as_str())
    .bind(hash_hex(snapshot.content_hash()));
    query = bind_time(query, snapshot.observed_at())?;
    query = bind_time(query, snapshot.visible_at())?;
    query = query
        .bind(value.benchmark().reference().id().as_str())
        .bind(i64::try_from(
            value.benchmark().reference().version().get(),
        )?)
        .bind(hash_hex(value.benchmark().content_hash()))
        .bind(value.metric_convention().reference().id().as_str())
        .bind(i64::try_from(
            value.metric_convention().reference().version().get(),
        )?)
        .bind(hash_hex(value.metric_convention().content_hash()));
    query = bind_time(query, value.effective_from())?;
    query = bind_time(query, value.effective_to())?;
    query = bind_time(query, &fixture.visible_at)?;
    query
        .bind(hash_hex(value.content_hash()))
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn insert_analytics_authority(
    transaction: &mut Transaction<'_, Postgres>,
    candidate: &PortfolioAnalyticsAuthorityCandidate,
) -> Result<(), AnyError> {
    let mut query = sqlx::query(
        "INSERT INTO portfolio.analytics_authority_sets
         (tenant_id,authority_set_id,owner_id,subject_id,subject_version,
          position_snapshot_id,position_snapshot_hash,curve_snapshot_id,curve_snapshot_hash,
          data_snapshot_id,data_snapshot_hash,tax_rule_pack_id,tax_rule_pack_version,
          tax_rule_pack_hash,effective_from,effective_from_nanos,effective_from_timezone,
          effective_from_local_date,effective_to,effective_to_nanos,effective_to_timezone,
          effective_to_local_date,visible_at,visible_at_nanos,visible_at_timezone,
          visible_at_local_date,content_hash)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,
                 $21,$22,$23,$24,$25,$26,$27) ON CONFLICT DO NOTHING",
    )
    .bind(candidate.owner.tenant_id().as_str())
    .bind(candidate.authority_set_id.as_str())
    .bind(candidate.owner.owner_id().as_str())
    .bind(candidate.subject_ref.id().as_str())
    .bind(i64::try_from(candidate.subject_ref.version().get())?)
    .bind(candidate.position_snapshot.id.as_str())
    .bind(hash_hex(&candidate.position_snapshot.content_hash))
    .bind(candidate.curve_snapshot.id.as_str())
    .bind(hash_hex(&candidate.curve_snapshot.content_hash))
    .bind(candidate.data_snapshot.id.as_str())
    .bind(hash_hex(&candidate.data_snapshot.content_hash))
    .bind(candidate.tax_rule_pack.version_ref().id().as_str())
    .bind(i64::try_from(
        candidate.tax_rule_pack.version_ref().version().get(),
    )?)
    .bind(hash_hex(candidate.tax_rule_pack.content_hash()));
    query = bind_time(query, &candidate.effective_from)?;
    query = bind_time(query, &candidate.effective_to)?;
    query = bind_time(query, &candidate.visible_at)?;
    query
        .bind(hash_hex(&candidate.content_hash))
        .execute(&mut **transaction)
        .await?;
    for unit in &candidate.units {
        sqlx::query(
            "INSERT INTO portfolio.analytics_authority_units
             (tenant_id,authority_set_id,role,unit_id,unit_version,unit_hash)
             VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING",
        )
        .bind(candidate.owner.tenant_id().as_str())
        .bind(candidate.authority_set_id.as_str())
        .bind(unit_role_name(unit.role))
        .bind(unit.reference.unit_id().as_str())
        .bind(i64::try_from(unit.reference.version().get())?)
        .bind(hash_hex(&unit.content_hash))
        .execute(&mut **transaction)
        .await?;
    }
    for bond in &candidate.bond_rates {
        sqlx::query(
            "INSERT INTO portfolio.bond_rates_authorities
             (tenant_id,authority_set_id,position_id,instrument_id,instrument_version,
              valuation_id,valuation_source_revision,valuation_hash,valuation_value_index,
              remaining_years_value_index,mode,input_coefficient,input_scale,
              remaining_years_coefficient,remaining_years_scale,settlement_date,
              calendar_requirement)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12::numeric,12,$13::numeric,12,$14,$15)
             ON CONFLICT DO NOTHING",
        )
        .bind(candidate.owner.tenant_id().as_str())
        .bind(candidate.authority_set_id.as_str())
        .bind(bond.position_id.as_str())
        .bind(bond.instrument_ref.id().as_str())
        .bind(i64::try_from(bond.instrument_ref.version().get())?)
        .bind(bond.valuation.valuation_id.as_str())
        .bind(i64::try_from(bond.valuation.source_revision)?)
        .bind(hash_hex(&bond.valuation.content_hash))
        .bind(i32::try_from(bond.valuation.value_index)?)
        .bind(i32::try_from(bond.remaining_years_value_index)?)
        .bind(analytics_mode_name(bond.mode))
        .bind(bond.input_value.scaled().to_string())
        .bind(bond.remaining_years.scaled().to_string())
        .bind(bond.settlement_date)
        .bind(calendar_requirement_name(bond.calendar_requirement))
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

async fn verify_catalog(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    let temporal = app(PortfolioCatalogTemporalScope::new(
        fixture.owner.clone(),
        fixture.subject.version().reference().clone(),
        fixture.visible_at.clone(),
        fixture.visible_at.clone(),
    ))?;
    let stored = app(repository
        .read_catalog_snapshot(principal.access_scope(), &temporal)
        .await)?;
    if stored.books().len() != 1
        || stored.groups().len() != 1
        || stored.benchmarks().len() != 1
        || stored.metric_conventions().len() != 1
        || stored.portfolios().len() != fixture.portfolios.len()
        || stored.books()[0].value() != &fixture.book
        || stored.groups()[0].value() != &fixture.group
        || stored.benchmarks()[0].value() != &fixture.benchmark
        || stored.metric_conventions()[0].value() != &fixture.convention
        || stored
            .portfolios()
            .iter()
            .zip(&fixture.portfolios)
            .any(|(record, expected)| record.value() != expected)
    {
        return Err("catalog conflict/replay read-back differs from fixture".into());
    }
    Ok(())
}

async fn verify_analytics_authorities(
    repository: &PostgresRepository,
    principal: &AuthorizedPrincipal,
    fixture: &Fixture,
) -> Result<(), AnyError> {
    for (snapshot, expected) in fixture.snapshots.iter().zip(&fixture.analytics_authorities) {
        let query = app(PortfolioAnalyticsAuthorityQuery::new(
            fixture.owner.clone(),
            fixture.subject.version().reference().clone(),
            snapshot_binding(snapshot)?,
            snapshot.observed_at().clone(),
            fixture.visible_at.clone(),
        ))?;
        let stored = app(repository
            .read_candidates(principal.access_scope(), &query)
            .await)?;
        if stored.as_slice() != std::slice::from_ref(expected) {
            return Err(format!(
                "analytics authority conflict/replay differs for snapshot {}",
                snapshot.id()
            )
            .into());
        }
        for binding in &expected.bond_rates {
            let valuation = app(repository
                .read_valuation_exact(principal.access_scope(), &fixture.owner, &binding.valuation)
                .await)?
            .ok_or("typed Valuation is absent after governed publication")?;
            if valuation.value_roles()
                != [
                    ValuationValueRole::Yield,
                    ValuationValueRole::RemainingYears,
                ]
            {
                return Err("typed Valuation role read-back differs".into());
            }
        }
    }
    Ok(())
}

fn bind_time<'q>(
    query: sqlx::query::Query<'q, Postgres, sqlx::postgres::PgArguments>,
    value: &MarketTime,
) -> Result<sqlx::query::Query<'q, Postgres, sqlx::postgres::PgArguments>, AnyError> {
    Ok(query
        .bind(
            value
                .instant()
                .with_nanosecond(0)
                .ok_or("invalid market-time nanos")?,
        )
        .bind(i32::try_from(value.instant().timestamp_subsec_nanos())?)
        .bind(value.market_timezone().to_owned())
        .bind(value.local_trading_date()))
}

fn market_time(value: &str, timezone: &str) -> Result<MarketTime, AnyError> {
    let instant = DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc);
    let timezone = timezone.parse::<Tz>()?;
    Ok(MarketTime::new(
        instant,
        timezone.name(),
        instant.with_timezone(&timezone).date_naive(),
    )?)
}

fn status(value: &str) -> Result<PortfolioStatus, AnyError> {
    match value {
        "ACTIVE" => Ok(PortfolioStatus::Active),
        "SUSPENDED" => Ok(PortfolioStatus::Suspended),
        "CLOSED" => Ok(PortfolioStatus::Closed),
        _ => Err(format!("unknown portfolio status {value}").into()),
    }
}

const fn unit_role_name(value: PortfolioRatesUnitRole) -> &'static str {
    match value {
        PortfolioRatesUnitRole::CurrencyAmount => "CURRENCY_AMOUNT",
        PortfolioRatesUnitRole::PricePer100 => "PRICE_PER_100",
        PortfolioRatesUnitRole::Rate => "RATE",
        PortfolioRatesUnitRole::Years => "YEARS",
        PortfolioRatesUnitRole::YearsSquared => "YEARS_SQUARED",
        PortfolioRatesUnitRole::Dv01Per100 => "DV01_PER_100",
        PortfolioRatesUnitRole::Dv01 => "DV01",
        PortfolioRatesUnitRole::Dimensionless => "DIMENSIONLESS",
        PortfolioRatesUnitRole::ContractCount => "CONTRACT_COUNT",
    }
}

const fn analytics_mode_name(value: AnalyticsMode) -> &'static str {
    match value {
        AnalyticsMode::YieldIn => "YIELD_IN",
        AnalyticsMode::PriceIn => "PRICE_IN",
    }
}

const fn calendar_requirement_name(value: CalendarRequirement) -> &'static str {
    match value {
        CalendarRequirement::ReferenceReplay => "REFERENCE_REPLAY",
        CalendarRequirement::ExactMarket => "EXACT_MARKET",
    }
}

const fn status_name(value: PortfolioStatus) -> &'static str {
    match value {
        PortfolioStatus::Active => "ACTIVE",
        PortfolioStatus::Suspended => "SUSPENDED",
        PortfolioStatus::Closed => "CLOSED",
    }
}

fn snapshot_binding(snapshot: &PositionSnapshot) -> Result<PortfolioSnapshotBinding, AnyError> {
    Ok(PortfolioSnapshotBinding::new(
        snapshot.id().clone(),
        snapshot.content_hash().clone(),
        snapshot.observed_at().clone(),
        snapshot.visible_at().clone(),
    )?)
}

fn exact_lineage<T>(value: &T) -> Result<LineageRef, AnyError>
where
    T: ContentAddressed + VersionedDefinition,
{
    Ok(LineageRef::new(
        ulid(value.identity())?,
        Some(Version::new(value.version())?),
        Some(value.content_hash().clone()),
    )?)
}

fn version_ref(value: &VersionSource) -> Result<VersionRef, AnyError> {
    source_ref(&value.id, value.version)
}

fn unit_ref(value: &VersionSource) -> Result<UnitRef, AnyError> {
    Ok(UnitRef::new(ulid(&value.id)?, Version::new(value.version)?))
}

fn source_ref(id: &str, version: u64) -> Result<VersionRef, AnyError> {
    Ok(VersionRef::new(ulid(id)?, Version::new(version)?))
}

fn ulid(value: &str) -> Result<Ulid, AnyError> {
    Ok(Ulid::new(value.to_owned())?)
}

fn hash_hex(value: &ContentHash) -> String {
    value.as_bytes().iter().fold(
        String::with_capacity(value.as_bytes().len() * 2),
        |mut text, byte| {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
            text
        },
    )
}

#[allow(dead_code)]
fn _fixture_directory(path: &Path) -> Option<&Path> {
    path.parent()
}
