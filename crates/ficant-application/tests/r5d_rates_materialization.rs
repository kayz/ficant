use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use ficant_application::ports::{
    AccessScope, AppendDefinitionVersion, ApplicationResult, ArtifactRepository,
    BondAnalyticsArtifactCodec, BondAnalyticsArtifactFacts, CanonicalQuote,
    CanonicalSnapshotDecoder, CouponTaxRate, CurvePointSetDecoder, CurveSnapshotMetadata,
    CurveSnapshotMetadataRepository, DataSourceRepository, DecodedCanonicalQuotes,
    DecodedCurvePoint, DecodedCurvePointSet, DefinitionIdentity, DefinitionRepository,
    DefinitionValue, EncodedBondAnalyticsArtifact, EncodedFuturesDeliveryArtifact,
    FactorTopologyRepository, FundingRate, FundingRulePackParser,
    FuturesDeliveryArtifactCandidateFacts, FuturesDeliveryArtifactCodec,
    FuturesDeliveryArtifactFacts, FuturesDeliveryRuleParser, IdempotencyKey, IntegrityEvent,
    IntegrityEventSink, PublishArtifact, RegisterDataSource, RequiredVerifiedBlobRead,
    SafeTraceContext, SnapshotVerifiedReadMetadata, SnapshotVerifiedReadMetadataRepository,
    SubjectRepository, TaxRulePackParser, VerifiedBlobPayload, VerifiedBlobReader,
    VerifiedBlobRole,
};
use ficant_application::{
    ApplicationError, ApplicationErrorCategory, BOND_ANALYTICS_MEDIA_TYPE, BondRatesCommand,
    CarryRatesCommand, CurveRatesCommand, DeliveryRatesCommand, FUTURES_DELIVERY_MEDIA_TYPE,
    HedgeRatesCommand, ImmutableArtifactBinding, ImmutableSnapshotBinding,
    MaterializeBondRatesInput, MaterializeCarryRatesInput, MaterializeCurveRatesInput,
    MaterializeDeliveryRatesInput, MaterializeHedgeRatesInput, RatesEvidenceBinding,
    RatesInputEvidence, RatesInputRole, RatesRequestEvidence, RatesUnitRequirement,
    rates_data_source_content_hash,
};
use ficant_domain::analytics::{
    AnalyticsError, AnalyticsMode, AnalyticsObjectRef, BondAnalyticsInput, BondAnalyticsResult,
    CalendarRequirement, FixedDecimal,
};
use ficant_domain::futures_delivery::{
    CgbFuturesProduct, FuturesDeliverableInput, FuturesDeliveryBasketResult, FuturesDeliveryRule,
    FuturesDeliveryRuleInput,
};
use ficant_domain::market::{
    ArtifactInputKind, Bond, BondBusinessDayConvention, BondCouponFrequency,
    BondDayCountConvention, BondPricingTerms, BondTaxAttributes, Calendar, CalendarInput,
    CalendarSession, CurveSnapshot, CurveSnapshotInput, DataSource, DataSourceInput,
    DataSourceKind, FuturesContract, IncomeTaxStatus, Instrument, InstrumentInput, InstrumentKind,
    MarketRulePack, MarketRulePackInput, PriceSourceType, RulePackContent, Unit, UnitInput,
    ValueAddedTaxStatus, VerificationStatus,
};
use ficant_domain::primitives::{
    ContentHash, DecimalValue, EffectivePeriod, LineageRef, MarketTime, OwnerRef, Ulid, UnitRef,
    Version, VersionRef,
};
use ficant_domain::research::{
    Artifact, ArtifactKind, CurveNodeDefinition, CurveNodeDefinitionInput, DataSnapshot,
    DataSnapshotInput, FactorDefinition, FactorTarget, FactorTargetBinding,
};
use ficant_domain::subject::{
    AccessSet, FundingTier, Subject, SubjectRecord, SubjectStateSnapshot, SubjectVersion,
    TaxTreatment,
};
use ficant_domain::{ContentAddressed, VersionedDefinition};

const CURVE_BYTES: &[u8] = b"r5d-canonical-curve";
const PARQUET_BYTES: &[u8] = b"r5d-canonical-quotes";
const MANIFEST_BYTES: &[u8] = b"r5d-canonical-manifest";
const TARGET_BYTES: &[u8] = b"r5d-target-risk-artifact";
const DELIVERY_ARTIFACT_BYTES: &[u8] = b"r5d-delivery-artifact";
const CTD_BYTES: &[u8] = b"r5d-ctd-analytics-artifact";
const TAX_BYTES: &[u8] = b"r5d-tax-rules";
const FUNDING_BYTES: &[u8] = b"r5d-funding-rules";
const DELIVERY_RULE_BYTES: &[u8] = b"r5d-delivery-rules";
const CURVE_RULE_BYTES: &[u8] = b"r5d-curve-rules";
const TAX_TYPE: &str = "type.googleapis.com/ficant.market.v1.TaxRulePack";
const FUNDING_TYPE: &str = "type.googleapis.com/ficant.market.v1.FundingRulePack";
const DELIVERY_TYPE: &str = "type.googleapis.com/ficant.market.v1.CgbFuturesDeliveryRulePack";
const CURVE_TYPE: &str = "type.googleapis.com/ficant.market.v1.CurveRulePack";

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn all_five_materializers_return_stable_sorted_complete_evidence() {
    let fixture = Fixture::new();

    let bond =
        MaterializeBondRatesInput::new(&fixture, &fixture, &fixture, &fixture, &fixture, &fixture);
    let bond_first = counted_handoff(
        bond.execute(&fixture.scope, fixture.bond_command(fixed_rate(3)), trace()),
        &AtomicUsize::new(0),
    )
    .await
    .expect("exact Bond inputs materialize");
    let bond_second = bond
        .execute(&fixture.scope, fixture.bond_command(fixed_rate(3)), trace())
        .await
        .expect("the same Bond request rematerializes");
    assert_stable_evidence(bond_first.evidence(), bond_second.evidence());
    assert_complete(
        bond_first.evidence(),
        &[
            (RatesInputRole::Subject, 'E'),
            (RatesInputRole::Unit, 'C'),
            (RatesInputRole::Unit, 'P'),
            (RatesInputRole::Unit, 'Q'),
            (RatesInputRole::Unit, 'R'),
            (RatesInputRole::Unit, 'V'),
            (RatesInputRole::Unit, 'W'),
            (RatesInputRole::Unit, 'J'),
            (RatesInputRole::Unit, 'N'),
            (RatesInputRole::Unit, 'H'),
            (RatesInputRole::Bond, 'B'),
            (RatesInputRole::Calendar, 'K'),
            (RatesInputRole::DataSnapshot, 'S'),
            (RatesInputRole::TaxRulePack, 'X'),
        ],
        &[],
    );

    let curve = MaterializeCurveRatesInput::new(
        &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture,
    );
    let curve_first = curve
        .execute(&fixture.scope, fixture.curve_command(), trace())
        .await
        .expect("exact Curve inputs materialize");
    let curve_second = curve
        .execute(&fixture.scope, fixture.curve_command(), trace())
        .await
        .expect("the same Curve request rematerializes");
    assert_stable_evidence(curve_first.evidence(), curve_second.evidence());
    assert_complete(
        curve_first.evidence(),
        &[
            (RatesInputRole::Subject, 'E'),
            (RatesInputRole::Unit, 'C'),
            (RatesInputRole::Unit, 'P'),
            (RatesInputRole::Unit, 'R'),
            (RatesInputRole::Unit, 'V'),
            (RatesInputRole::Unit, 'W'),
            (RatesInputRole::Unit, 'Q'),
            (RatesInputRole::Unit, 'J'),
            (RatesInputRole::Unit, 'N'),
            (RatesInputRole::Unit, 'H'),
            (RatesInputRole::Calendar, 'K'),
            (RatesInputRole::CurveSnapshot, '4'),
            (RatesInputRole::DataSnapshot, 'S'),
            (RatesInputRole::DataSource, '8'),
            (RatesInputRole::CurveRulePack, 'Z'),
        ],
        &[
            "cn.gov.yield-curve.02y",
            "cn.gov.yield-curve.05y",
            "cn.gov.yield-curve.10y",
        ],
    );

    let carry = MaterializeCarryRatesInput::new(
        &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture,
    );
    let carry_first = carry
        .execute(&fixture.scope, fixture.carry_command(), trace())
        .await
        .expect("exact Carry inputs materialize");
    let carry_second = carry
        .execute(&fixture.scope, fixture.carry_command(), trace())
        .await
        .expect("the same Carry request rematerializes");
    assert_stable_evidence(carry_first.evidence(), carry_second.evidence());
    assert_complete(
        carry_first.evidence(),
        &[
            (RatesInputRole::Subject, 'E'),
            (RatesInputRole::Unit, 'C'),
            (RatesInputRole::Unit, 'P'),
            (RatesInputRole::Unit, 'R'),
            (RatesInputRole::Unit, 'V'),
            (RatesInputRole::Unit, 'W'),
            (RatesInputRole::Unit, 'Q'),
            (RatesInputRole::Unit, 'J'),
            (RatesInputRole::Unit, 'N'),
            (RatesInputRole::Unit, 'H'),
            (RatesInputRole::Bond, 'B'),
            (RatesInputRole::Calendar, 'K'),
            (RatesInputRole::CurveSnapshot, '4'),
            (RatesInputRole::DataSnapshot, 'S'),
            (RatesInputRole::DataSource, '8'),
            (RatesInputRole::CurveRulePack, 'Z'),
        ],
        &[
            "cn.gov.yield-curve.02y",
            "cn.gov.yield-curve.05y",
            "cn.gov.yield-curve.10y",
        ],
    );

    let delivery = MaterializeDeliveryRatesInput::new(
        &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture,
        &fixture,
    );
    let delivery_first = delivery
        .execute(&fixture.scope, fixture.delivery_command(), trace())
        .await
        .expect("exact Delivery inputs materialize");
    let delivery_second = delivery
        .execute(&fixture.scope, fixture.delivery_command(), trace())
        .await
        .expect("the same Delivery request rematerializes");
    assert_stable_evidence(delivery_first.evidence(), delivery_second.evidence());
    assert_complete(
        delivery_first.evidence(),
        &[
            (RatesInputRole::Subject, 'E'),
            (RatesInputRole::Unit, 'C'),
            (RatesInputRole::Unit, 'N'),
            (RatesInputRole::Unit, 'P'),
            (RatesInputRole::Unit, 'R'),
            (RatesInputRole::Unit, 'V'),
            (RatesInputRole::Unit, 'W'),
            (RatesInputRole::Unit, 'Q'),
            (RatesInputRole::Unit, 'J'),
            (RatesInputRole::Unit, 'H'),
            (RatesInputRole::Bond, 'B'),
            (RatesInputRole::DataSnapshot, 'S'),
            (RatesInputRole::DataSource, '8'),
            (RatesInputRole::TaxRulePack, 'X'),
            (RatesInputRole::FundingRulePack, 'M'),
            (RatesInputRole::DeliveryRulePack, 'D'),
            (RatesInputRole::FuturesContract, 'F'),
        ],
        &[],
    );

    let hedge = MaterializeHedgeRatesInput::new(
        &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture,
    );
    let hedge_first = hedge
        .execute(&fixture.scope, fixture.hedge_command(), trace())
        .await
        .expect("exact Hedge artifacts materialize");
    let hedge_second = hedge
        .execute(&fixture.scope, fixture.hedge_command(), trace())
        .await
        .expect("the same Hedge request rematerializes");
    assert_stable_evidence(hedge_first.evidence(), hedge_second.evidence());
    assert_complete(
        hedge_first.evidence(),
        &[
            (RatesInputRole::Subject, 'E'),
            (RatesInputRole::Unit, 'C'),
            (RatesInputRole::Unit, 'P'),
            (RatesInputRole::Unit, 'R'),
            (RatesInputRole::Unit, 'V'),
            (RatesInputRole::Unit, 'W'),
            (RatesInputRole::Unit, 'Q'),
            (RatesInputRole::Unit, 'H'),
            (RatesInputRole::Unit, 'J'),
            (RatesInputRole::Unit, 'N'),
            (RatesInputRole::Bond, 'B'),
            (RatesInputRole::DeliveryRulePack, 'D'),
            (RatesInputRole::FuturesContract, 'F'),
            (RatesInputRole::TargetRiskArtifact, '5'),
            (RatesInputRole::DeliveryArtifact, '6'),
            (RatesInputRole::CtdAnalyticsArtifact, '7'),
        ],
        &[],
    );

    assert_eq!(bond_first.input().rule_pack().version_ref().id(), &id('X'));
    assert_eq!(
        delivery_first.inputs()[0].rule_pack().version_ref().id(),
        &id('D')
    );
    assert_eq!(hedge_first.input().rule_pack().version_ref().id(), &id('D'));
    assert_ne!(id('X'), id('D'), "Tax and Delivery packs stay distinct");
}

#[tokio::test]
async fn bond_scenario_scalar_changes_parameter_and_request_hash_not_snapshot_identity() {
    let fixture = Fixture::new();
    let use_case =
        MaterializeBondRatesInput::new(&fixture, &fixture, &fixture, &fixture, &fixture, &fixture);
    let low = use_case
        .execute(&fixture.scope, fixture.bond_command(fixed_rate(2)), trace())
        .await
        .unwrap();
    let high = use_case
        .execute(&fixture.scope, fixture.bond_command(fixed_rate(4)), trace())
        .await
        .unwrap();

    assert_eq!(
        low.evidence().consumed_inputs(),
        high.evidence().consumed_inputs()
    );
    assert_eq!(low.input().snapshot(), high.input().snapshot());
    assert_eq!(low.input().snapshot().version_ref().id(), &id('S'));
    assert_ne!(
        low.evidence().canonical_parameters_sha256(),
        high.evidence().canonical_parameters_sha256()
    );
    assert_ne!(
        low.evidence().request_fingerprint(),
        high.evidence().request_fingerprint()
    );
}

#[tokio::test]
async fn bond_identity_version_hash_and_time_drifts_fail_before_numerical_handoff() {
    let fixture = Fixture::new();
    let use_case =
        MaterializeBondRatesInput::new(&fixture, &fixture, &fixture, &fixture, &fixture, &fixture);

    let mut wrong_identity = fixture.bond_command(fixed_rate(3));
    wrong_identity.bond = AnalyticsObjectRef::new(
        VersionRef::new(id('9'), version()),
        wrong_identity.bond.content_hash().clone(),
    );
    assert_fail_closed(use_case.execute(&fixture.scope, wrong_identity, trace())).await;

    let mut wrong_version = fixture.bond_command(fixed_rate(3));
    wrong_version.bond = AnalyticsObjectRef::new(
        VersionRef::new(id('B'), Version::new(2).unwrap()),
        wrong_version.bond.content_hash().clone(),
    );
    assert_fail_closed(use_case.execute(&fixture.scope, wrong_version, trace())).await;

    let mut wrong_hash = fixture.bond_command(fixed_rate(3));
    wrong_hash.bond = AnalyticsObjectRef::new(
        wrong_hash.bond.version_ref().clone(),
        ContentHash::digest(b"wrong-bond-definition"),
    );
    assert_fail_closed(use_case.execute(&fixture.scope, wrong_hash, trace())).await;

    let mut stale_knowledge = fixture.bond_command(fixed_rate(3));
    stale_knowledge.knowledge_at = time(1);
    assert_fail_closed(use_case.execute(&fixture.scope, stale_knowledge, trace())).await;

    let mut wrong_valuation = fixture.bond_command(fixed_rate(3));
    wrong_valuation.valuation_at = time(0);
    assert_fail_closed(use_case.execute(&fixture.scope, wrong_valuation, trace())).await;

    let mut as_of_drift = fixture.clone();
    as_of_drift.data = data_snapshot(time(0), time(2), ContentHash::digest(PARQUET_BYTES));
    let use_case = MaterializeBondRatesInput::new(
        &as_of_drift,
        &as_of_drift,
        &as_of_drift,
        &as_of_drift,
        &as_of_drift,
        &as_of_drift,
    );
    assert_fail_closed(use_case.execute(
        &as_of_drift.scope,
        as_of_drift.bond_command(fixed_rate(3)),
        trace(),
    ))
    .await;

    let mut visible_drift = fixture.clone();
    visible_drift.data = data_snapshot(time(1), time(4), ContentHash::digest(PARQUET_BYTES));
    let use_case = MaterializeBondRatesInput::new(
        &visible_drift,
        &visible_drift,
        &visible_drift,
        &visible_drift,
        &visible_drift,
        &visible_drift,
    );
    assert_fail_closed(use_case.execute(
        &visible_drift.scope,
        visible_drift.bond_command(fixed_rate(3)),
        trace(),
    ))
    .await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn effective_and_payload_content_drifts_fail_before_numerical_handoff() {
    let mut ineffective_calendar = Fixture::new();
    ineffective_calendar.replace_definition(DefinitionValue::Calendar(calendar_with_effective(
        time(2),
        time(15),
    )));
    let use_case = MaterializeBondRatesInput::new(
        &ineffective_calendar,
        &ineffective_calendar,
        &ineffective_calendar,
        &ineffective_calendar,
        &ineffective_calendar,
        &ineffective_calendar,
    );
    assert_fail_closed(use_case.execute(
        &ineffective_calendar.scope,
        ineffective_calendar.bond_command(fixed_rate(3)),
        trace(),
    ))
    .await;

    let mut ineffective = Fixture::new();
    ineffective.replace_definition(DefinitionValue::MarketRulePack(rule_pack(
        'X',
        "CN",
        "tax",
        TAX_TYPE,
        TAX_BYTES,
        time(2),
        time(15),
    )));
    let use_case = MaterializeBondRatesInput::new(
        &ineffective,
        &ineffective,
        &ineffective,
        &ineffective,
        &ineffective,
        &ineffective,
    );
    assert_fail_closed(use_case.execute(
        &ineffective.scope,
        ineffective.bond_command(fixed_rate(3)),
        trace(),
    ))
    .await;

    let mut content_drift = Fixture::new();
    content_drift.replace_definition(DefinitionValue::MarketRulePack(rule_pack(
        'X',
        "CN",
        "tax",
        TAX_TYPE,
        b"r5d-tax-rules-drift",
        time(0),
        time(15),
    )));
    let use_case = MaterializeBondRatesInput::new(
        &content_drift,
        &content_drift,
        &content_drift,
        &content_drift,
        &content_drift,
        &content_drift,
    );
    assert_fail_closed(use_case.execute(
        &content_drift.scope,
        content_drift.bond_command(fixed_rate(3)),
        trace(),
    ))
    .await;

    let mut funding_drift = Fixture::new();
    funding_drift.replace_definition(DefinitionValue::MarketRulePack(rule_pack(
        'M',
        "CN",
        "funding",
        FUNDING_TYPE,
        b"r5d-funding-rules-drift",
        time(0),
        time(15),
    )));
    let use_case = MaterializeDeliveryRatesInput::new(
        &funding_drift,
        &funding_drift,
        &funding_drift,
        &funding_drift,
        &funding_drift,
        &funding_drift,
        &funding_drift,
        &funding_drift,
        &funding_drift,
        &funding_drift,
    );
    Box::pin(assert_fail_closed(use_case.execute(
        &funding_drift.scope,
        funding_drift.delivery_command(),
        trace(),
    )))
    .await;

    let mut unverified_tax = Fixture::new();
    unverified_tax.replace_definition(DefinitionValue::MarketRulePack(rule_pack_with_status(
        'X',
        "CN",
        "tax",
        TAX_TYPE,
        TAX_BYTES,
        time(0),
        time(15),
        VerificationStatus::Unverified,
    )));
    let use_case = MaterializeBondRatesInput::new(
        &unverified_tax,
        &unverified_tax,
        &unverified_tax,
        &unverified_tax,
        &unverified_tax,
        &unverified_tax,
    );
    assert_fail_closed(use_case.execute(
        &unverified_tax.scope,
        unverified_tax.bond_command(fixed_rate(3)),
        trace(),
    ))
    .await;
}

#[tokio::test]
async fn exact_bond_definition_units_are_checked_before_numerical_handoff() {
    let mut coupon_unit_drift = Fixture::new();
    coupon_unit_drift.replace_definition(bond_definition_with_units(
        'B',
        unit_ref('C'),
        unit_ref('N'),
    ));
    let bond = MaterializeBondRatesInput::new(
        &coupon_unit_drift,
        &coupon_unit_drift,
        &coupon_unit_drift,
        &coupon_unit_drift,
        &coupon_unit_drift,
        &coupon_unit_drift,
    );
    assert_fail_closed(bond.execute(
        &coupon_unit_drift.scope,
        coupon_unit_drift.bond_command(fixed_rate(3)),
        trace(),
    ))
    .await;

    let mut currency_unit_drift = Fixture::new();
    currency_unit_drift.replace_definition(bond_definition_with_units(
        'B',
        unit_ref('N'),
        unit_ref('R'),
    ));
    let carry = MaterializeCarryRatesInput::new(
        &currency_unit_drift,
        &currency_unit_drift,
        &currency_unit_drift,
        &currency_unit_drift,
        &currency_unit_drift,
        &currency_unit_drift,
        &currency_unit_drift,
        &currency_unit_drift,
        &currency_unit_drift,
    );
    assert_fail_closed(Box::pin(carry.execute(
        &currency_unit_drift.scope,
        currency_unit_drift.carry_command(),
        trace(),
    )))
    .await;
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn curve_carry_delivery_and_hedge_drift_never_reaches_numerical_handoff() {
    let fixture = Fixture::new();

    let curve = MaterializeCurveRatesInput::new(
        &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture,
    );
    let mut wrong_curve = fixture.curve_command();
    wrong_curve.curve = ImmutableSnapshotBinding::new(
        wrong_curve.curve.id().clone(),
        ContentHash::digest(b"wrong-curve-hash"),
    );
    assert_fail_closed(curve.execute(&fixture.scope, wrong_curve, trace())).await;

    let mut point_drift = fixture.clone();
    point_drift.curve_points = DecodedCurvePointSet::new(
        "cn.gov.yield-curve",
        vec![
            DecodedCurvePoint::new(
                "cn.gov.yield-curve.02y",
                ContentHash::digest(b"wrong-node-content"),
                decimal("25", 3, unit_ref('R')),
            )
            .unwrap(),
            DecodedCurvePoint::new(
                "cn.gov.yield-curve.05y",
                curve_node("cn.gov.yield-curve.05y", "P5Y")
                    .content_hash()
                    .clone(),
                decimal("30", 3, unit_ref('R')),
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let curve = MaterializeCurveRatesInput::new(
        &point_drift,
        &point_drift,
        &point_drift,
        &point_drift,
        &point_drift,
        &point_drift,
        &point_drift,
        &point_drift,
        &point_drift,
    );
    assert_fail_closed(curve.execute(&point_drift.scope, point_drift.curve_command(), trace()))
        .await;

    let carry = MaterializeCarryRatesInput::new(
        &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture,
    );
    let mut valuation_drift = fixture.carry_command();
    valuation_drift.valuation_at = time(0);
    Box::pin(assert_fail_closed(carry.execute(
        &fixture.scope,
        valuation_drift,
        trace(),
    )))
    .await;

    let delivery = MaterializeDeliveryRatesInput::new(
        &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture,
        &fixture,
    );
    let mut snapshot_hash_drift = fixture.delivery_command();
    snapshot_hash_drift.data_snapshot = ImmutableSnapshotBinding::new(
        snapshot_hash_drift.data_snapshot.id().clone(),
        ContentHash::digest(b"wrong-delivery-snapshot"),
    );
    Box::pin(assert_fail_closed(delivery.execute(
        &fixture.scope,
        snapshot_hash_drift,
        trace(),
    )))
    .await;

    let mut source_lineage_drift = fixture.clone();
    source_lineage_drift.data = data_snapshot_with_source_hash(
        time(1),
        time(2),
        ContentHash::digest(PARQUET_BYTES),
        ContentHash::digest(b"stale-delivery-source-hash"),
    );
    let delivery = MaterializeDeliveryRatesInput::new(
        &source_lineage_drift,
        &source_lineage_drift,
        &source_lineage_drift,
        &source_lineage_drift,
        &source_lineage_drift,
        &source_lineage_drift,
        &source_lineage_drift,
        &source_lineage_drift,
        &source_lineage_drift,
        &source_lineage_drift,
    );
    Box::pin(assert_fail_closed(delivery.execute(
        &source_lineage_drift.scope,
        source_lineage_drift.delivery_command(),
        trace(),
    )))
    .await;

    let hedge = MaterializeHedgeRatesInput::new(
        &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture,
    );
    let mut artifact_hash_drift = fixture.hedge_command();
    artifact_hash_drift.ctd_analytics_artifact = ImmutableArtifactBinding::new(
        artifact_hash_drift.ctd_analytics_artifact.id().clone(),
        ContentHash::digest(b"wrong-ctd-artifact"),
    );
    assert_fail_closed(hedge.execute(&fixture.scope, artifact_hash_drift, trace())).await;

    let mut artifact_content_drift = fixture.clone();
    artifact_content_drift.ctd_facts = BondAnalyticsArtifactFacts::new(
        time(0),
        fixture.ctd_facts.bond().clone(),
        fixture.ctd_facts.rule_pack().clone(),
        fixture.ctd_facts.snapshot().clone(),
        fixture.ctd_facts.dv01(),
    );
    let hedge = MaterializeHedgeRatesInput::new(
        &artifact_content_drift,
        &artifact_content_drift,
        &artifact_content_drift,
        &artifact_content_drift,
        &artifact_content_drift,
        &artifact_content_drift,
        &artifact_content_drift,
        &artifact_content_drift,
    );
    assert_fail_closed(hedge.execute(
        &artifact_content_drift.scope,
        artifact_content_drift.hedge_command(),
        trace(),
    ))
    .await;

    let mut target_lineage_drift = fixture.clone();
    target_lineage_drift.target_facts = BondAnalyticsArtifactFacts::new(
        time(1),
        fixture.target_facts.bond().clone(),
        fixture.target_facts.rule_pack().clone(),
        AnalyticsObjectRef::new(
            VersionRef::new(id('9'), version()),
            fixture.target_facts.snapshot().content_hash().clone(),
        ),
        fixture.target_facts.dv01(),
    );
    let target_artifact = bond_artifact('5', TARGET_BYTES, &target_lineage_drift.target_facts);
    target_lineage_drift
        .artifacts
        .insert(id('5'), target_artifact);
    let hedge = MaterializeHedgeRatesInput::new(
        &target_lineage_drift,
        &target_lineage_drift,
        &target_lineage_drift,
        &target_lineage_drift,
        &target_lineage_drift,
        &target_lineage_drift,
        &target_lineage_drift,
        &target_lineage_drift,
    );
    assert_fail_closed(hedge.execute(
        &target_lineage_drift.scope,
        target_lineage_drift.hedge_command(),
        trace(),
    ))
    .await;
}

#[tokio::test]
async fn sub_microsecond_knowledge_drift_changes_bond_request_fingerprint() {
    let fixture = Fixture::new();
    let use_case =
        MaterializeBondRatesInput::new(&fixture, &fixture, &fixture, &fixture, &fixture, &fixture);
    let mut first = fixture.bond_command(fixed_rate(3));
    first.knowledge_at = time_with_nanos(3, 100);
    let mut second = fixture.bond_command(fixed_rate(3));
    second.knowledge_at = time_with_nanos(3, 200);

    let first = use_case
        .execute(&fixture.scope, first, trace())
        .await
        .unwrap();
    let second = use_case
        .execute(&fixture.scope, second, trace())
        .await
        .unwrap();
    assert_eq!(
        first.evidence().consumed_inputs(),
        second.evidence().consumed_inputs()
    );
    assert_ne!(
        first.evidence().canonical_parameters_sha256(),
        second.evidence().canonical_parameters_sha256()
    );
    assert_ne!(
        first.evidence().request_fingerprint(),
        second.evidence().request_fingerprint()
    );
}

#[tokio::test]
async fn hedge_rejects_future_valuation_and_drifted_delivery_rule_authority() {
    let fixture = Fixture::new();
    let hedge = MaterializeHedgeRatesInput::new(
        &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture, &fixture,
    );
    let mut future_valuation = fixture.hedge_command();
    future_valuation.knowledge_at = time(0);
    assert_fail_closed(hedge.execute(&fixture.scope, future_valuation, trace())).await;

    let mut rule_drift = Fixture::new();
    rule_drift.replace_definition(DefinitionValue::MarketRulePack(rule_pack(
        'D',
        "CFFEX",
        "cgb-futures",
        DELIVERY_TYPE,
        b"r5d-delivery-rules-drift",
        time(0),
        time(15),
    )));
    let hedge = MaterializeHedgeRatesInput::new(
        &rule_drift,
        &rule_drift,
        &rule_drift,
        &rule_drift,
        &rule_drift,
        &rule_drift,
        &rule_drift,
        &rule_drift,
    );
    assert_fail_closed(hedge.execute(&rule_drift.scope, rule_drift.hedge_command(), trace())).await;
}

#[tokio::test]
async fn curve_lineage_and_definition_effective_windows_are_authoritative() {
    let mut lineage_drift = Fixture::new();
    lineage_drift.curve = curve_snapshot(vec![
        LineageRef::new(
            id('8'),
            Some(version()),
            Some(ContentHash::digest(b"invented-data-source-hash")),
        )
        .unwrap(),
        LineageRef::content_addressed(id('S'), lineage_drift.data.content_hash().clone()),
    ]);
    let curve = MaterializeCurveRatesInput::new(
        &lineage_drift,
        &lineage_drift,
        &lineage_drift,
        &lineage_drift,
        &lineage_drift,
        &lineage_drift,
        &lineage_drift,
        &lineage_drift,
        &lineage_drift,
    );
    assert_fail_closed(curve.execute(&lineage_drift.scope, lineage_drift.curve_command(), trace()))
        .await;

    let mut nested_source_hash_drift = Fixture::new();
    nested_source_hash_drift.data = data_snapshot_with_source_hash(
        time(1),
        time(2),
        ContentHash::digest(PARQUET_BYTES),
        ContentHash::digest(b"stale-data-source-hash"),
    );
    let curve = MaterializeCurveRatesInput::new(
        &nested_source_hash_drift,
        &nested_source_hash_drift,
        &nested_source_hash_drift,
        &nested_source_hash_drift,
        &nested_source_hash_drift,
        &nested_source_hash_drift,
        &nested_source_hash_drift,
        &nested_source_hash_drift,
        &nested_source_hash_drift,
    );
    assert_fail_closed(curve.execute(
        &nested_source_hash_drift.scope,
        nested_source_hash_drift.curve_command(),
        trace(),
    ))
    .await;

    let mut calendar_drift = Fixture::new();
    calendar_drift.replace_definition(DefinitionValue::Calendar(calendar_with_effective(
        time(2),
        time(15),
    )));
    let curve = MaterializeCurveRatesInput::new(
        &calendar_drift,
        &calendar_drift,
        &calendar_drift,
        &calendar_drift,
        &calendar_drift,
        &calendar_drift,
        &calendar_drift,
        &calendar_drift,
        &calendar_drift,
    );
    assert_fail_closed(curve.execute(
        &calendar_drift.scope,
        calendar_drift.curve_command(),
        trace(),
    ))
    .await;

    let mut rule_effective_drift = Fixture::new();
    rule_effective_drift.replace_definition(DefinitionValue::MarketRulePack(rule_pack(
        'Z',
        "CN",
        "yield-curve",
        CURVE_TYPE,
        CURVE_RULE_BYTES,
        time(2),
        time(15),
    )));
    let curve = MaterializeCurveRatesInput::new(
        &rule_effective_drift,
        &rule_effective_drift,
        &rule_effective_drift,
        &rule_effective_drift,
        &rule_effective_drift,
        &rule_effective_drift,
        &rule_effective_drift,
        &rule_effective_drift,
        &rule_effective_drift,
    );
    assert_fail_closed(curve.execute(
        &rule_effective_drift.scope,
        rule_effective_drift.curve_command(),
        trace(),
    ))
    .await;
}

#[tokio::test]
async fn curve_kind_and_rule_semantics_are_authoritative() {
    let mut kind_drift = Fixture::new();
    kind_drift.curve = curve_snapshot_with_kind(
        vec![
            LineageRef::new(
                id('8'),
                Some(version()),
                Some(rates_data_source_content_hash(&kind_drift.data_source)),
            )
            .unwrap(),
            LineageRef::content_addressed(id('S'), kind_drift.data.content_hash().clone()),
        ],
        "ZERO",
    );
    let curve = MaterializeCurveRatesInput::new(
        &kind_drift,
        &kind_drift,
        &kind_drift,
        &kind_drift,
        &kind_drift,
        &kind_drift,
        &kind_drift,
        &kind_drift,
        &kind_drift,
    );
    assert_fail_closed(curve.execute(&kind_drift.scope, kind_drift.curve_command(), trace())).await;

    let mut rule_type_drift = Fixture::new();
    rule_type_drift.replace_definition(DefinitionValue::MarketRulePack(rule_pack(
        'Z',
        "CN",
        "tax",
        TAX_TYPE,
        CURVE_RULE_BYTES,
        time(0),
        time(15),
    )));
    let curve = MaterializeCurveRatesInput::new(
        &rule_type_drift,
        &rule_type_drift,
        &rule_type_drift,
        &rule_type_drift,
        &rule_type_drift,
        &rule_type_drift,
        &rule_type_drift,
        &rule_type_drift,
        &rule_type_drift,
    );
    assert_fail_closed(curve.execute(
        &rule_type_drift.scope,
        rule_type_drift.curve_command(),
        trace(),
    ))
    .await;

    let mut unverified_rule = Fixture::new();
    unverified_rule.replace_definition(DefinitionValue::MarketRulePack(rule_pack_with_status(
        'Z',
        "CN",
        "yield-curve",
        CURVE_TYPE,
        CURVE_RULE_BYTES,
        time(0),
        time(15),
        VerificationStatus::Unverified,
    )));
    let curve = MaterializeCurveRatesInput::new(
        &unverified_rule,
        &unverified_rule,
        &unverified_rule,
        &unverified_rule,
        &unverified_rule,
        &unverified_rule,
        &unverified_rule,
        &unverified_rule,
        &unverified_rule,
    );
    assert_fail_closed(curve.execute(
        &unverified_rule.scope,
        unverified_rule.curve_command(),
        trace(),
    ))
    .await;
}

#[derive(Clone)]
struct Fixture {
    scope: AccessScope,
    subject: SubjectRecord,
    definitions: Vec<DefinitionValue>,
    curve: CurveSnapshot,
    curve_points: DecodedCurvePointSet,
    data: DataSnapshot,
    data_source: DataSource,
    artifacts: BTreeMap<Ulid, Artifact>,
    artifact_payloads: BTreeMap<Ulid, Vec<u8>>,
    target_facts: BondAnalyticsArtifactFacts,
    ctd_facts: BondAnalyticsArtifactFacts,
    delivery_facts: FuturesDeliveryArtifactFacts,
}

impl Fixture {
    #[allow(clippy::too_many_lines)]
    fn new() -> Self {
        let units = vec![
            unit('C', "CNY", "currency_amount", 2),
            unit('P', "CNY100", "price_per_100", 12),
            unit('R', "RATE", "rate", 12),
            unit('V', "YEARS", "years", 12),
            unit('W', "YEARS2", "years_squared", 12),
            unit('Q', "DV01_PER_100", "dv01_per_100", 12),
            unit('J', "DV01", "dv01", 12),
            unit('N', "ONE", "dimensionless", 12),
            unit('H', "CONTRACT", "contract_count", 0),
        ];
        let calendar = calendar();
        let tax_pack = rule_pack('X', "CN", "tax", TAX_TYPE, TAX_BYTES, time(0), time(15));
        let funding_pack = rule_pack(
            'M',
            "CN",
            "funding",
            FUNDING_TYPE,
            FUNDING_BYTES,
            time(0),
            time(15),
        );
        let delivery_pack = rule_pack(
            'D',
            "CFFEX",
            "cgb-futures",
            DELIVERY_TYPE,
            DELIVERY_RULE_BYTES,
            time(0),
            time(15),
        );
        let curve_pack = rule_pack(
            'Z',
            "CN",
            "yield-curve",
            CURVE_TYPE,
            CURVE_RULE_BYTES,
            time(0),
            time(15),
        );
        let bond_instrument = instrument('B', InstrumentKind::Bond);
        let target_instrument = instrument('G', InstrumentKind::Bond);
        let future_instrument = instrument('F', InstrumentKind::Futures);
        let bond_value = DefinitionValue::Instrument(
            ficant_application::ports::InstrumentDefinition::new(
                bond_instrument.clone(),
                Some(ficant_application::ports::InstrumentSubtype::Bond(bond(
                    &bond_instrument,
                ))),
            )
            .unwrap(),
        );
        let target_value = DefinitionValue::Instrument(
            ficant_application::ports::InstrumentDefinition::new(
                target_instrument.clone(),
                Some(ficant_application::ports::InstrumentSubtype::Bond(bond(
                    &target_instrument,
                ))),
            )
            .unwrap(),
        );
        let future = FuturesContract::new(
            &future_instrument,
            time_for(2026, 9, 11, 11),
            time_for(2026, 9, 11, 15),
            time_for(2026, 9, 18, 8),
            decimal("1", 0, unit_ref('C')),
            VersionRef::new(id('D'), version()),
        )
        .unwrap()
        .with_risk_terms("T", unit_ref('P'))
        .unwrap();
        let future_value = DefinitionValue::Instrument(
            ficant_application::ports::InstrumentDefinition::new(
                future_instrument,
                Some(ficant_application::ports::InstrumentSubtype::FuturesContract(future)),
            )
            .unwrap(),
        );
        let mut definitions = units
            .into_iter()
            .map(DefinitionValue::Unit)
            .collect::<Vec<_>>();
        definitions.extend([
            DefinitionValue::Calendar(calendar),
            DefinitionValue::MarketRulePack(tax_pack.clone()),
            DefinitionValue::MarketRulePack(funding_pack),
            DefinitionValue::MarketRulePack(delivery_pack.clone()),
            DefinitionValue::MarketRulePack(curve_pack),
            bond_value.clone(),
            target_value.clone(),
            future_value.clone(),
        ]);

        let curve_nodes = [
            curve_node("cn.gov.yield-curve.02y", "P2Y"),
            curve_node("cn.gov.yield-curve.05y", "P5Y"),
            curve_node("cn.gov.yield-curve.10y", "P10Y"),
        ];
        let curve_points = DecodedCurvePointSet::new(
            "cn.gov.yield-curve",
            curve_nodes
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    DecodedCurvePoint::new(
                        node.curve_node_id(),
                        node.content_hash().clone(),
                        decimal(&(25 + index * 5).to_string(), 3, unit_ref('R')),
                    )
                    .unwrap()
                })
                .collect(),
        )
        .unwrap();
        let data = data_snapshot(time(1), time(2), ContentHash::digest(PARQUET_BYTES));
        let data_source = data_source();
        let curve = curve_snapshot(vec![
            LineageRef::new(
                id('8'),
                Some(version()),
                Some(rates_data_source_content_hash(&data_source)),
            )
            .unwrap(),
            LineageRef::content_addressed(id('S'), data.content_hash().clone()),
        ]);
        let tax_binding = AnalyticsObjectRef::new(
            VersionRef::new(id('X'), version()),
            definition_hash(&DefinitionValue::MarketRulePack(tax_pack)),
        );
        let delivery_payload_binding = AnalyticsObjectRef::new(
            VersionRef::new(id('D'), version()),
            delivery_pack.content_hash().clone(),
        );
        let snapshot_ref = AnalyticsObjectRef::new(
            VersionRef::new(id('S'), version()),
            data.content_hash().clone(),
        );
        let bond_ref = AnalyticsObjectRef::new(
            VersionRef::new(id('B'), version()),
            definition_hash(&bond_value),
        );
        let target_ref = AnalyticsObjectRef::new(
            VersionRef::new(id('G'), version()),
            definition_hash(&target_value),
        );
        let future_ref = AnalyticsObjectRef::new(
            VersionRef::new(id('F'), version()),
            definition_hash(&future_value),
        );
        let target_facts = BondAnalyticsArtifactFacts::new(
            time(1),
            target_ref.clone(),
            tax_binding.clone(),
            snapshot_ref.clone(),
            fixed_int(20),
        );
        let ctd_facts = BondAnalyticsArtifactFacts::new(
            time(1),
            bond_ref.clone(),
            tax_binding.clone(),
            snapshot_ref.clone(),
            fixed_int(8),
        );
        let delivery_facts = FuturesDeliveryArtifactFacts::new(
            time(1),
            future_ref,
            delivery_payload_binding.clone(),
            snapshot_ref,
            CgbFuturesProduct::TenYear,
            vec![FuturesDeliveryArtifactCandidateFacts::new(
                bond_ref,
                fixed_rate(95),
            )],
            0,
        );
        let target_artifact = bond_artifact('5', TARGET_BYTES, &target_facts);
        let delivery_artifact = delivery_artifact('6', DELIVERY_ARTIFACT_BYTES, &delivery_facts);
        let ctd_artifact = bond_artifact('7', CTD_BYTES, &ctd_facts);
        let artifacts = [target_artifact, delivery_artifact, ctd_artifact]
            .into_iter()
            .map(|value| (value.id().clone(), value))
            .collect();
        let artifact_payloads = [
            (id('5'), TARGET_BYTES.to_vec()),
            (id('6'), DELIVERY_ARTIFACT_BYTES.to_vec()),
            (id('7'), CTD_BYTES.to_vec()),
        ]
        .into_iter()
        .collect();
        let subject = subject();
        Self {
            scope: AccessScope::new(id('T'), id('A'), vec![id('Y')]).unwrap(),
            subject,
            definitions,
            curve,
            curve_points,
            data,
            data_source,
            artifacts,
            artifact_payloads,
            target_facts,
            ctd_facts,
            delivery_facts,
        }
    }

    fn definition(&self, suffix: char) -> &DefinitionValue {
        self.definitions
            .iter()
            .find(|value| value.identity() == id(suffix).as_str())
            .expect("fixture definition exists")
    }

    fn replace_definition(&mut self, replacement: DefinitionValue) {
        let index = self
            .definitions
            .iter()
            .position(|value| value.identity() == replacement.identity())
            .expect("replacement identity exists in fixture");
        self.definitions[index] = replacement;
    }

    fn object_binding(&self, suffix: char) -> AnalyticsObjectRef {
        let value = self.definition(suffix);
        AnalyticsObjectRef::new(
            VersionRef::new(id(suffix), Version::new(value.version()).unwrap()),
            definition_hash(value),
        )
    }

    fn bond_command(&self, input_value: FixedDecimal) -> BondRatesCommand {
        BondRatesCommand {
            owner: owner(),
            subject_ref: VersionRef::new(id('E'), version()),
            units: unit_requirements(),
            currency_unit: unit_ref('C'),
            rate_unit: unit_ref('R'),
            knowledge_at: time(3),
            bond: self.object_binding('B'),
            calendar: self.object_binding('K'),
            data_snapshot: ImmutableSnapshotBinding::new(
                self.data.id().clone(),
                self.data.content_hash().clone(),
            ),
            tax_rule_pack: self.object_binding('X'),
            valuation_at: time(1),
            settlement_date: date(2026, 8, 4),
            calendar_requirement: CalendarRequirement::ExactMarket,
            mode: AnalyticsMode::YieldIn,
            input_value,
        }
    }

    fn curve_command(&self) -> CurveRatesCommand {
        CurveRatesCommand {
            owner: owner(),
            subject_ref: VersionRef::new(id('E'), version()),
            units: unit_requirements(),
            currency_unit: unit_ref('C'),
            rate_unit: unit_ref('R'),
            knowledge_at: time(3),
            curve: ImmutableSnapshotBinding::new(
                self.curve.id().clone(),
                self.curve.content_hash().clone(),
            ),
            query_date: date(2029, 8, 3),
        }
    }

    fn carry_command(&self) -> CarryRatesCommand {
        CarryRatesCommand {
            owner: owner(),
            subject_ref: VersionRef::new(id('E'), version()),
            units: unit_requirements(),
            currency_unit: unit_ref('C'),
            rate_unit: unit_ref('R'),
            knowledge_at: time(3),
            bond: self.object_binding('B'),
            curve: ImmutableSnapshotBinding::new(
                self.curve.id().clone(),
                self.curve.content_hash().clone(),
            ),
            valuation_at: time(1),
            initial_settlement: date(2026, 8, 4),
            horizon_settlement: date(2026, 11, 4),
            calendar_requirement: CalendarRequirement::ExactMarket,
        }
    }

    fn delivery_command(&self) -> DeliveryRatesCommand {
        DeliveryRatesCommand {
            owner: owner(),
            subject_ref: VersionRef::new(id('E'), version()),
            units: unit_requirements(),
            currency_unit: unit_ref('C'),
            price_unit: unit_ref('P'),
            rate_unit: unit_ref('R'),
            knowledge_at: time(3),
            futures_contract: self.object_binding('F'),
            data_snapshot: ImmutableSnapshotBinding::new(
                self.data.id().clone(),
                self.data.content_hash().clone(),
            ),
            funding_rule_pack: self.object_binding('M'),
            tax_rule_pack: self.object_binding('X'),
            valuation_at: time(1),
            purchase_date: date(2026, 8, 3),
        }
    }

    fn hedge_command(&self) -> HedgeRatesCommand {
        HedgeRatesCommand {
            owner: owner(),
            subject_ref: VersionRef::new(id('E'), version()),
            units: unit_requirements(),
            knowledge_at: time(3),
            target_risk_artifact: artifact_binding(&self.artifacts[&id('5')]),
            delivery_artifact: artifact_binding(&self.artifacts[&id('6')]),
            ctd_analytics_artifact: artifact_binding(&self.artifacts[&id('7')]),
            futures_contract: self.object_binding('F'),
            valuation_at: time(1),
        }
    }
}

#[async_trait]
impl DefinitionRepository for Fixture {
    async fn create_identity(&self, _: DefinitionIdentity) -> ApplicationResult<()> {
        Err(unavailable())
    }

    async fn append_version(
        &self,
        _: AppendDefinitionVersion,
    ) -> ApplicationResult<DefinitionValue> {
        Err(unavailable())
    }

    async fn get_version(
        &self,
        _: &AccessScope,
        definition_id: Ulid,
        version: Version,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        Ok(self
            .definitions
            .iter()
            .find(|value| {
                value.identity() == definition_id.as_str() && value.version() == version.get()
            })
            .cloned())
    }

    async fn resolve_as_of(
        &self,
        _: &AccessScope,
        _: Ulid,
        _: MarketTime,
    ) -> ApplicationResult<Option<DefinitionValue>> {
        Err(unavailable())
    }
}

#[async_trait]
impl SubjectRepository for Fixture {
    async fn register_subject(&self, _: SubjectRecord) -> ApplicationResult<SubjectRecord> {
        Err(unavailable())
    }

    async fn get_subject(&self, reference: VersionRef) -> ApplicationResult<Option<SubjectRecord>> {
        Ok((self.subject.version().reference() == &reference).then(|| self.subject.clone()))
    }

    async fn register_subject_state(
        &self,
        _: SubjectStateSnapshot,
    ) -> ApplicationResult<SubjectStateSnapshot> {
        Err(unavailable())
    }

    async fn get_subject_state(
        &self,
        _: Ulid,
        _: chrono::DateTime<Utc>,
    ) -> ApplicationResult<Option<SubjectStateSnapshot>> {
        Err(unavailable())
    }
}

#[async_trait]
impl CurveSnapshotMetadataRepository for Fixture {
    async fn get_curve_snapshot_metadata(
        &self,
        _: &AccessScope,
        curve_snapshot_id: Ulid,
    ) -> ApplicationResult<Option<CurveSnapshotMetadata>> {
        Ok((curve_snapshot_id == *self.curve.id()).then(|| {
            CurveSnapshotMetadata::new(self.curve.clone(), CURVE_BYTES.len() as u64).unwrap()
        }))
    }
}

#[async_trait]
impl SnapshotVerifiedReadMetadataRepository for Fixture {
    async fn get_verified_read_metadata(
        &self,
        _: &AccessScope,
        snapshot_id: Ulid,
    ) -> ApplicationResult<Option<SnapshotVerifiedReadMetadata>> {
        if snapshot_id != *self.data.id() {
            return Ok(None);
        }
        SnapshotVerifiedReadMetadata::data(
            self.data.clone(),
            PARQUET_BYTES.len() as u64,
            MANIFEST_BYTES.len() as u64,
        )
        .map(Some)
    }
}

#[async_trait]
impl DataSourceRepository for Fixture {
    async fn register(&self, _: RegisterDataSource) -> ApplicationResult<DataSource> {
        Err(unavailable())
    }

    async fn get_exact(
        &self,
        _: &AccessScope,
        reference: VersionRef,
    ) -> ApplicationResult<Option<DataSource>> {
        Ok((self.data_source.id() == reference.id()
            && self.data_source.version() == reference.version().get())
        .then(|| self.data_source.clone()))
    }
}

#[async_trait]
impl FactorTopologyRepository for Fixture {
    async fn register_factor_definition(
        &self,
        _: &AccessScope,
        _: FactorDefinition,
        _: IdempotencyKey,
    ) -> ApplicationResult<FactorDefinition> {
        Err(unavailable())
    }

    async fn register_curve_node_definition(
        &self,
        _: &AccessScope,
        _: CurveNodeDefinition,
        _: IdempotencyKey,
    ) -> ApplicationResult<CurveNodeDefinition> {
        Err(unavailable())
    }

    async fn bind_factor_target(
        &self,
        _: &AccessScope,
        _: FactorTargetBinding,
        _: IdempotencyKey,
    ) -> ApplicationResult<FactorTargetBinding> {
        Err(unavailable())
    }

    async fn get_factor_definition(&self, _: &str) -> ApplicationResult<Option<FactorDefinition>> {
        Ok(None)
    }

    async fn get_curve_node_definition(
        &self,
        curve_node_id: &str,
    ) -> ApplicationResult<Option<CurveNodeDefinition>> {
        Ok([
            ("cn.gov.yield-curve.02y", "P2Y"),
            ("cn.gov.yield-curve.05y", "P5Y"),
            ("cn.gov.yield-curve.10y", "P10Y"),
        ]
        .into_iter()
        .find(|(id, _)| *id == curve_node_id)
        .map(|(id, tenor)| curve_node(id, tenor)))
    }

    async fn get_factor_targets(
        &self,
        _: &AccessScope,
        _: &str,
    ) -> ApplicationResult<Vec<FactorTargetBinding>> {
        Err(unavailable())
    }

    async fn get_target_factors(
        &self,
        _: &AccessScope,
        _: &FactorTarget,
    ) -> ApplicationResult<Vec<FactorDefinition>> {
        Err(unavailable())
    }

    async fn exact_target_exists(&self, _: &FactorTarget) -> ApplicationResult<bool> {
        Ok(true)
    }
}

impl CurvePointSetDecoder for Fixture {
    fn decode_canonical(&self, bytes: &[u8]) -> ApplicationResult<DecodedCurvePointSet> {
        if bytes != CURVE_BYTES {
            return Err(validation());
        }
        Ok(self.curve_points.clone())
    }
}

#[async_trait]
impl CanonicalSnapshotDecoder for Fixture {
    async fn decode_quotes(
        &self,
        snapshot: &DataSnapshot,
        parquet: &[u8],
        manifest: &[u8],
    ) -> ApplicationResult<DecodedCanonicalQuotes> {
        if snapshot != &self.data || parquet != PARQUET_BYTES || manifest != MANIFEST_BYTES {
            return Err(validation());
        }
        DecodedCanonicalQuotes::new(
            VersionRef::new(id('8'), version()),
            vec![
                quote('B', fixed_int(100), fixed_int(102)),
                quote('F', fixed_int(99), fixed_int(101)),
            ],
        )
    }
}

#[async_trait]
impl VerifiedBlobReader for Fixture {
    async fn read_required(
        &self,
        request: &RequiredVerifiedBlobRead,
        sink: &dyn IntegrityEventSink,
    ) -> ApplicationResult<VerifiedBlobPayload> {
        let bytes = match request.blob_role() {
            VerifiedBlobRole::CurvePoints => CURVE_BYTES.to_vec(),
            VerifiedBlobRole::DataParquet => PARQUET_BYTES.to_vec(),
            VerifiedBlobRole::DataManifest => MANIFEST_BYTES.to_vec(),
            VerifiedBlobRole::ArtifactPayload => self
                .artifact_payloads
                .get(request.resource_id())
                .cloned()
                .ok_or_else(unavailable)?,
            _ => return Err(unavailable()),
        };
        request.verify_bytes(sink, bytes).await
    }
}

#[async_trait]
impl IntegrityEventSink for Fixture {
    async fn emit(&self, _: IntegrityEvent) -> ApplicationResult<()> {
        Ok(())
    }
}

#[async_trait]
impl ArtifactRepository for Fixture {
    async fn publish_verified_blob(&self, _: PublishArtifact) -> ApplicationResult<Artifact> {
        Err(unavailable())
    }

    async fn get_metadata(
        &self,
        _: &AccessScope,
        artifact_id: Ulid,
    ) -> ApplicationResult<Option<Artifact>> {
        Ok(self.artifacts.get(&artifact_id).cloned())
    }
}

impl TaxRulePackParser for Fixture {
    fn market(&self) -> &'static str {
        "CN"
    }

    fn rule_type(&self) -> &'static str {
        "tax"
    }

    fn type_url(&self) -> &'static str {
        TAX_TYPE
    }

    fn parse(
        &self,
        content: &RulePackContent,
        _: NaiveDate,
        _: BondTaxAttributes,
        _: &TaxTreatment,
    ) -> ApplicationResult<CouponTaxRate> {
        if content.value() != TAX_BYTES {
            return Err(validation());
        }
        Ok(CouponTaxRate::new(
            FixedDecimal::from_scaled(130_000_000_000),
            unit_ref('R'),
        ))
    }
}

impl FundingRulePackParser for Fixture {
    fn market(&self) -> &'static str {
        "CN"
    }

    fn rule_type(&self) -> &'static str {
        "funding"
    }

    fn type_url(&self) -> &'static str {
        FUNDING_TYPE
    }

    fn parse(&self, content: &RulePackContent, _: FundingTier) -> ApplicationResult<FundingRate> {
        if content.value() != FUNDING_BYTES {
            return Err(validation());
        }
        Ok(FundingRate::new(
            FixedDecimal::from_scaled(18_000_000_000),
            unit_ref('R'),
        ))
    }
}

impl FuturesDeliveryRuleParser for Fixture {
    fn market(&self) -> &'static str {
        "CFFEX"
    }

    fn rule_type(&self) -> &'static str {
        "cgb-futures"
    }

    fn type_url(&self) -> &'static str {
        DELIVERY_TYPE
    }

    fn parse_product_code(&self, product_code: &str) -> ApplicationResult<CgbFuturesProduct> {
        (product_code == "T")
            .then_some(CgbFuturesProduct::TenYear)
            .ok_or_else(validation)
    }

    fn parse(
        &self,
        content: &RulePackContent,
        _: CgbFuturesProduct,
    ) -> ApplicationResult<FuturesDeliveryRule> {
        if content.value() != DELIVERY_RULE_BYTES {
            return Err(validation());
        }
        delivery_rule()
    }
}

impl BondAnalyticsArtifactCodec for Fixture {
    fn encode(
        &self,
        _: &BondAnalyticsResult,
    ) -> Result<EncodedBondAnalyticsArtifact, AnalyticsError> {
        Err(AnalyticsError::Internal)
    }

    fn decode(
        &self,
        _: &[u8],
        _: &BondAnalyticsInput,
    ) -> Result<BondAnalyticsResult, AnalyticsError> {
        Err(AnalyticsError::Internal)
    }

    fn decode_facts(&self, bytes: &[u8]) -> Result<BondAnalyticsArtifactFacts, AnalyticsError> {
        match bytes {
            TARGET_BYTES => Ok(self.target_facts.clone()),
            CTD_BYTES => Ok(self.ctd_facts.clone()),
            _ => Err(AnalyticsError::InvalidInput),
        }
    }
}

impl FuturesDeliveryArtifactCodec for Fixture {
    fn encode(
        &self,
        _: &FuturesDeliveryBasketResult,
    ) -> Result<EncodedFuturesDeliveryArtifact, AnalyticsError> {
        Err(AnalyticsError::Internal)
    }

    fn encode_self_describing(
        &self,
        _: &FuturesDeliveryBasketResult,
    ) -> Result<EncodedFuturesDeliveryArtifact, AnalyticsError> {
        Err(AnalyticsError::Internal)
    }

    fn decode(
        &self,
        _: &[u8],
        _: &[FuturesDeliverableInput],
    ) -> Result<FuturesDeliveryBasketResult, AnalyticsError> {
        Err(AnalyticsError::Internal)
    }

    fn decode_facts(&self, bytes: &[u8]) -> Result<FuturesDeliveryArtifactFacts, AnalyticsError> {
        if bytes == DELIVERY_ARTIFACT_BYTES {
            Ok(self.delivery_facts.clone())
        } else {
            Err(AnalyticsError::InvalidInput)
        }
    }
}

async fn counted_handoff<F, T>(future: F, calls: &AtomicUsize) -> ApplicationResult<T>
where
    F: std::future::Future<Output = ApplicationResult<T>>,
{
    let materialized = future.await?;
    calls.fetch_add(1, Ordering::SeqCst);
    Ok(materialized)
}

async fn assert_fail_closed<F, T>(future: F) -> ApplicationError
where
    F: std::future::Future<Output = ApplicationResult<T>>,
{
    let calls = AtomicUsize::new(0);
    let Err(error) = counted_handoff(future, &calls).await else {
        panic!("drifted input unexpectedly reached the numerical handoff");
    };
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "materialization must fail before a numerical engine could be called"
    );
    assert!(!error.retryable(), "binding drift is never retryable");
    error
}

fn assert_stable_evidence(first: &RatesRequestEvidence, second: &RatesRequestEvidence) {
    assert_eq!(first, second);
    assert_ne!(
        first.canonical_parameters_sha256(),
        &ContentHash::digest(&[])
    );
    assert_ne!(first.request_fingerprint(), &ContentHash::digest(&[]));
    assert!(
        first
            .consumed_inputs()
            .windows(2)
            .all(|pair| { evidence_key(&pair[0]) <= evidence_key(&pair[1]) && pair[0] != pair[1] })
    );
}

fn assert_complete(
    evidence: &RatesRequestEvidence,
    expected: &[(RatesInputRole, char)],
    expected_factors: &[&str],
) {
    let actual = evidence
        .consumed_inputs()
        .iter()
        .map(|value| (value.role(), evidence_id(value)))
        .collect::<BTreeSet<_>>();
    let mut expected = expected
        .iter()
        .map(|(role, suffix)| (*role, id(*suffix).as_str().to_owned()))
        .collect::<BTreeSet<_>>();
    expected.extend(expected_factors.iter().map(|curve_node_id| {
        (
            RatesInputRole::CurveNodeDefinition,
            (*curve_node_id).to_owned(),
        )
    }));
    assert_eq!(actual, expected);
    assert_eq!(evidence.consumed_inputs().len(), expected.len());
}

fn evidence_key(value: &RatesInputEvidence) -> (RatesInputRole, String, u64, ContentHash) {
    match value.binding() {
        RatesEvidenceBinding::Object(binding) => (
            value.role(),
            binding.version_ref().id().as_str().to_owned(),
            binding.version_ref().version().get(),
            binding.content_hash().clone(),
        ),
        RatesEvidenceBinding::Snapshot(binding) => (
            value.role(),
            binding.id().as_str().to_owned(),
            0,
            binding.content_hash().clone(),
        ),
        RatesEvidenceBinding::Artifact(binding) => (
            value.role(),
            binding.id().as_str().to_owned(),
            0,
            binding.content_hash().clone(),
        ),
        RatesEvidenceBinding::CurveNode(binding) => (
            value.role(),
            binding.curve_node_id().to_owned(),
            0,
            binding.content_hash().clone(),
        ),
    }
}

fn evidence_id(value: &RatesInputEvidence) -> String {
    match value.binding() {
        RatesEvidenceBinding::Object(binding) => binding.version_ref().id().as_str().to_owned(),
        RatesEvidenceBinding::Snapshot(binding) => binding.id().as_str().to_owned(),
        RatesEvidenceBinding::Artifact(binding) => binding.id().as_str().to_owned(),
        RatesEvidenceBinding::CurveNode(binding) => binding.curve_node_id().to_owned(),
    }
}

fn artifact_binding(value: &Artifact) -> ImmutableArtifactBinding {
    ImmutableArtifactBinding::new(value.id().clone(), value.content_hash().clone())
}

fn bond_artifact(suffix: char, bytes: &[u8], facts: &BondAnalyticsArtifactFacts) -> Artifact {
    Artifact::new(
        id(suffix),
        owner(),
        ArtifactKind::Generic,
        BOND_ANALYTICS_MEDIA_TYPE,
        ContentHash::digest(bytes),
        bytes.len() as u64,
        vec![
            LineageRef::versioned(
                facts.bond().version_ref().id().clone(),
                facts.bond().version_ref().version(),
            ),
            LineageRef::new(
                facts.rule_pack().version_ref().id().clone(),
                Some(facts.rule_pack().version_ref().version()),
                Some(facts.rule_pack().content_hash().clone()),
            )
            .unwrap(),
            LineageRef::content_addressed(
                facts.snapshot().version_ref().id().clone(),
                facts.snapshot().content_hash().clone(),
            ),
        ],
    )
    .unwrap()
}

fn delivery_artifact(suffix: char, bytes: &[u8], facts: &FuturesDeliveryArtifactFacts) -> Artifact {
    let mut lineage = vec![LineageRef::versioned(
        facts.futures_contract().version_ref().id().clone(),
        facts.futures_contract().version_ref().version(),
    )];
    lineage.extend(facts.candidates().iter().map(|candidate| {
        LineageRef::versioned(
            candidate.bond().version_ref().id().clone(),
            candidate.bond().version_ref().version(),
        )
    }));
    lineage.extend([
        LineageRef::new(
            facts.rule_pack().version_ref().id().clone(),
            Some(facts.rule_pack().version_ref().version()),
            Some(facts.rule_pack().content_hash().clone()),
        )
        .unwrap(),
        LineageRef::content_addressed(
            facts.snapshot().version_ref().id().clone(),
            facts.snapshot().content_hash().clone(),
        ),
    ]);
    Artifact::new(
        id(suffix),
        owner(),
        ArtifactKind::Generic,
        FUTURES_DELIVERY_MEDIA_TYPE,
        ContentHash::digest(bytes),
        bytes.len() as u64,
        lineage,
    )
    .unwrap()
}

fn quote(suffix: char, bid: FixedDecimal, ask: FixedDecimal) -> CanonicalQuote {
    CanonicalQuote::new(
        VersionRef::new(id(suffix), version()),
        time(1),
        time(2),
        date(2026, 8, 3),
        Some(bid),
        Some(ask),
        unit_ref('P'),
    )
}

fn delivery_rule() -> ApplicationResult<FuturesDeliveryRule> {
    FuturesDeliveryRule::new(FuturesDeliveryRuleInput {
        original_term_max_months: 120,
        residual_min_months: 78,
        residual_max_months: None,
        delivery_months: vec![3, 6, 9, 12],
        nominal_coupon: fixed_rate(3),
        face_quote_basis: fixed_int(100),
        accrued_interest_day_count: 1,
        conversion_factor_rounding_places: 4,
        accrued_interest_rounding_places: 7,
        annual_day_basis: 365,
    })
    .and_then(|value| value.with_contract_size_in_quote_units(10_000))
    .map_err(ficant_application::map_domain_error)
}

fn subject() -> SubjectRecord {
    let subject = Subject::new(id('E'), "R5D exact rates Subject").unwrap();
    let subject_version = SubjectVersion::new(
        VersionRef::new(id('E'), version()),
        AccessSet::new(
            ["CFFEX", "CN"],
            [
                "bond-analytics",
                "carry-roll",
                "futures-delivery",
                "futures-hedge",
                "yield-curve",
            ],
        )
        .unwrap(),
        FundingTier::DrAvailable,
        TaxTreatment::new("r5d-vat", "r5d-income").unwrap(),
        "r5d-assessment",
        "r5d-liability",
        None,
    )
    .unwrap();
    SubjectRecord::new(subject, subject_version).unwrap()
}

fn unit(suffix: char, code: &str, dimension: &str, scale: u32) -> Unit {
    Unit::new(UnitInput {
        unit_id: id(suffix),
        version: version(),
        owner: owner(),
        code: code.to_owned(),
        dimension: dimension.to_owned(),
        scale,
        precision: 28,
    })
    .unwrap()
}

fn calendar() -> Calendar {
    calendar_with_effective(time_for(2020, 1, 1, 0), time_for(2040, 1, 1, 0))
}

fn calendar_with_effective(from: MarketTime, to: MarketTime) -> Calendar {
    Calendar::new(CalendarInput {
        calendar_id: id('K'),
        version: version(),
        owner: owner(),
        market: "CN".to_owned(),
        market_timezone: "Asia/Shanghai".to_owned(),
        effective: EffectivePeriod::new(from, to).unwrap(),
        sessions: vec![
            CalendarSession::open(
                date(2026, 8, 3),
                NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                NaiveTime::from_hms_opt(17, 0, 0).unwrap(),
            )
            .unwrap(),
        ],
    })
    .unwrap()
}

fn curve_snapshot(lineage: Vec<LineageRef>) -> CurveSnapshot {
    curve_snapshot_with_kind(lineage, "YTM")
}

fn curve_snapshot_with_kind(lineage: Vec<LineageRef>, curve_kind: &str) -> CurveSnapshot {
    CurveSnapshot::new(CurveSnapshotInput {
        curve_snapshot_id: id('4'),
        owner: owner(),
        as_of: time(1),
        currency: unit_ref('C'),
        curve_kind: curve_kind.to_owned(),
        calendar: VersionRef::new(id('K'), version()),
        rule_pack: VersionRef::new(id('Z'), version()),
        point_schema: ficant_application::ports::CURVE_POINT_SCHEMA.to_owned(),
        content_hash: ContentHash::digest(CURVE_BYTES),
        lineage,
        input_kind: ArtifactInputKind::ExternalFixture,
    })
    .unwrap()
    .with_knowledge_time(time(2), "cn.gov.yield-curve")
    .unwrap()
}

fn rule_pack(
    suffix: char,
    market: &str,
    rule_type: &str,
    type_url: &str,
    bytes: &[u8],
    from: MarketTime,
    to: MarketTime,
) -> MarketRulePack {
    rule_pack_with_status(
        suffix,
        market,
        rule_type,
        type_url,
        bytes,
        from,
        to,
        VerificationStatus::Verified,
    )
}

#[allow(clippy::too_many_arguments)]
fn rule_pack_with_status(
    suffix: char,
    market: &str,
    rule_type: &str,
    type_url: &str,
    bytes: &[u8],
    from: MarketTime,
    to: MarketTime,
    verification_status: VerificationStatus,
) -> MarketRulePack {
    let content = RulePackContent::new(type_url, bytes.to_vec()).unwrap();
    MarketRulePack::new_with_content(
        MarketRulePackInput {
            rule_pack_id: id(suffix),
            version: version(),
            owner: owner(),
            market: market.to_owned(),
            rule_type: rule_type.to_owned(),
            source: "r5d-test-fixture".to_owned(),
            effective: EffectivePeriod::new(from, to).unwrap(),
            verification_status,
            content_hash: ContentHash::digest(bytes),
        },
        content,
    )
    .unwrap()
}

fn instrument(suffix: char, kind: InstrumentKind) -> Instrument {
    Instrument::new(InstrumentInput {
        instrument_id: id(suffix),
        version: version(),
        owner: owner(),
        kind,
        market: "CN".to_owned(),
        symbol: format!("R5D-{suffix}"),
        currency: unit_ref('C'),
        calendar: VersionRef::new(id('K'), version()),
    })
    .unwrap()
}

fn bond_definition_with_units(
    suffix: char,
    currency_unit: UnitRef,
    rate_unit: UnitRef,
) -> DefinitionValue {
    let instrument = Instrument::new(InstrumentInput {
        instrument_id: id(suffix),
        version: version(),
        owner: owner(),
        kind: InstrumentKind::Bond,
        market: "CN".to_owned(),
        symbol: format!("R5D-{suffix}"),
        currency: currency_unit.clone(),
        calendar: VersionRef::new(id('K'), version()),
    })
    .unwrap();
    let bond = Bond::with_issuance(
        &instrument,
        date(2026, 8, 3),
        date(2026, 8, 3),
        date(2036, 8, 3),
        decimal("100000000", 0, currency_unit.clone()),
        BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Taxable),
        decimal("100", 0, currency_unit),
    )
    .unwrap()
    .with_pricing_terms(
        BondPricingTerms::new(
            decimal("25", 3, rate_unit),
            BondCouponFrequency::Semiannual,
            BondDayCountConvention::ActActBondIsma,
            BondBusinessDayConvention::Following,
        )
        .unwrap(),
    )
    .unwrap();
    DefinitionValue::Instrument(
        ficant_application::ports::InstrumentDefinition::new(
            instrument,
            Some(ficant_application::ports::InstrumentSubtype::Bond(bond)),
        )
        .unwrap(),
    )
}

fn bond(instrument: &Instrument) -> Bond {
    Bond::with_issuance(
        instrument,
        date(2026, 8, 3),
        date(2026, 8, 3),
        date(2036, 8, 3),
        decimal("100000000", 0, unit_ref('C')),
        BondTaxAttributes::new(ValueAddedTaxStatus::Taxable, IncomeTaxStatus::Taxable),
        decimal("100", 0, unit_ref('C')),
    )
    .unwrap()
    .with_pricing_terms(
        BondPricingTerms::new(
            decimal("25", 3, unit_ref('R')),
            BondCouponFrequency::Semiannual,
            BondDayCountConvention::ActActBondIsma,
            BondBusinessDayConvention::Following,
        )
        .unwrap(),
    )
    .unwrap()
}

fn curve_node(node_id: &str, tenor: &str) -> CurveNodeDefinition {
    let mut input = CurveNodeDefinitionInput {
        curve_node_id: node_id.to_owned(),
        curve_family_id: "cn.gov.yield-curve".to_owned(),
        tenor: tenor.to_owned(),
        factor_unit: unit_ref('R'),
        content_hash: ContentHash::digest(b"placeholder"),
    };
    input.content_hash = CurveNodeDefinition::content_hash_for(&input);
    CurveNodeDefinition::new(input).unwrap()
}

fn data_snapshot(as_of: MarketTime, visible_at: MarketTime, hash: ContentHash) -> DataSnapshot {
    data_snapshot_with_source_hash(
        as_of,
        visible_at,
        hash,
        rates_data_source_content_hash(&data_source()),
    )
}

fn data_source() -> DataSource {
    DataSource::new(DataSourceInput {
        data_source_id: id('8'),
        version: version(),
        owner: owner(),
        kind: DataSourceKind::FileNdjson,
        name: "R5D active quote fixture".to_owned(),
        connection_binding: "r5d-quotes".to_owned(),
        dataset: "r5d_quotes".to_owned(),
        canonical_schema_id: "ficant.market.quote.canonical.v1".to_owned(),
        canonical_schema_hash: ContentHash::digest(b"r5d-schema"),
    })
    .unwrap()
    .with_price_source_type(PriceSourceType::ActiveQuote)
    .unwrap()
}

fn data_snapshot_with_source_hash(
    as_of: MarketTime,
    visible_at: MarketTime,
    hash: ContentHash,
    source_hash: ContentHash,
) -> DataSnapshot {
    DataSnapshot::new(DataSnapshotInput {
        data_snapshot_id: id('S'),
        owner: owner(),
        visible_at,
        as_of,
        schema_hash: ContentHash::digest(b"r5d-schema"),
        manifest_hash: ContentHash::digest(MANIFEST_BYTES),
        blob_content_hash: hash,
        lineage: vec![LineageRef::new(id('8'), Some(version()), Some(source_hash)).unwrap()],
    })
    .unwrap()
}

fn definition_hash(value: &DefinitionValue) -> ContentHash {
    ContentHash::digest(&definition_bytes(value))
}

fn definition_bytes(value: &DefinitionValue) -> Vec<u8> {
    match value {
        DefinitionValue::Instrument(value) => {
            let mut encoded = Canonical::new("definition/instrument-aggregate/v1");
            encoded.field(2, &instrument_bytes(value.instrument()));
            match value.subtype() {
                None => {
                    encoded.field(3, &[0]);
                }
                Some(ficant_application::ports::InstrumentSubtype::Bond(bond)) => {
                    encoded.field(3, &[1]);
                    encoded.field(4, &bond_bytes(bond));
                }
                Some(ficant_application::ports::InstrumentSubtype::FuturesContract(contract)) => {
                    encoded.field(3, &[2]);
                    encoded.field(4, &futures_bytes(contract));
                }
            }
            encoded.finish()
        }
        DefinitionValue::Calendar(value) => calendar_bytes(value),
        DefinitionValue::Unit(value) => unit_bytes(value),
        DefinitionValue::MarketRulePack(value) => rule_pack_bytes(value),
    }
}

fn instrument_bytes(value: &Instrument) -> Vec<u8> {
    let mut encoded = Canonical::new("definition/instrument/v1");
    encoded.field(2, value.id().as_str().as_bytes());
    encoded.u64(3, value.version());
    encoded.field(4, &owner_bytes(value.owner()));
    encoded.field(
        5,
        &[match value.kind() {
            InstrumentKind::Bond => 1,
            InstrumentKind::Futures => 2,
            InstrumentKind::Other => 3,
        }],
    );
    encoded.field(6, value.market().as_bytes());
    encoded.field(7, value.symbol().as_bytes());
    encoded.field(8, &unit_ref_bytes(value.currency()));
    encoded.field(9, &version_ref_bytes(value.calendar()));
    encoded.finish()
}

fn bond_bytes(value: &Bond) -> Vec<u8> {
    let pricing = value.pricing_terms().expect("fixture Bond is priced");
    let mut encoded = Canonical::new("definition/bond/v3");
    encoded.field(2, &version_ref_bytes(value.instrument()));
    encoded.field(3, value.first_issue_date().to_string().as_bytes());
    encoded.field(4, value.current_issue_date().to_string().as_bytes());
    encoded.field(5, value.maturity_date().to_string().as_bytes());
    encoded.field(6, &decimal_bytes(value.cumulative_issued_amount()));
    let tax = value.tax_attributes().unwrap();
    encoded.field(
        7,
        &[
            match tax.value_added_tax_status() {
                ValueAddedTaxStatus::Exempt => 1,
                ValueAddedTaxStatus::Taxable => 2,
            },
            match tax.income_tax_status() {
                IncomeTaxStatus::Exempt => 1,
                IncomeTaxStatus::Taxable => 2,
            },
        ],
    );
    encoded.field(8, &decimal_bytes(value.face_value()));
    encoded.field(9, &decimal_bytes(pricing.coupon_rate()));
    encoded.field(
        10,
        &[match pricing.frequency() {
            BondCouponFrequency::Annual => 1,
            BondCouponFrequency::Semiannual => 2,
        }],
    );
    encoded.field(11, &[1]);
    encoded.field(12, &[1]);
    encoded.finish()
}

fn futures_bytes(value: &FuturesContract) -> Vec<u8> {
    let mut encoded = Canonical::new("definition/futures/v1");
    encoded.field(2, &version_ref_bytes(value.instrument()));
    encoded.field(3, &market_time_bytes(value.last_trade_time()));
    encoded.field(4, &market_time_bytes(value.expiry_time()));
    encoded.field(5, &market_time_bytes(value.settlement_time()));
    encoded.field(6, &decimal_bytes(value.multiplier()));
    encoded.field(7, &version_ref_bytes(value.rule_pack()));
    encoded.field(8, value.product_code().unwrap().as_bytes());
    encoded.field(9, &unit_ref_bytes(value.price_unit().unwrap()));
    encoded.finish()
}

fn calendar_bytes(value: &Calendar) -> Vec<u8> {
    let mut encoded = Canonical::new("definition/calendar/v1");
    encoded.field(2, value.identity().as_bytes());
    encoded.u64(3, value.version());
    encoded.field(4, &owner_bytes(value.owner()));
    encoded.field(5, value.market().as_bytes());
    encoded.field(6, value.market_timezone().as_bytes());
    encoded.field(7, &period_bytes(value.effective()));
    encoded.u64(8, value.sessions().len() as u64);
    for session in value.sessions() {
        let mut item = Canonical::new("calendar-session/v1");
        item.field(2, session.local_date().to_string().as_bytes());
        item.field(
            3,
            session
                .open_local_time()
                .map_or_else(|| "-".to_owned(), |time| time.to_string())
                .as_bytes(),
        );
        item.field(
            4,
            session
                .close_local_time()
                .map_or_else(|| "-".to_owned(), |time| time.to_string())
                .as_bytes(),
        );
        encoded.field(9, &item.finish());
    }
    encoded.finish()
}

fn unit_bytes(value: &Unit) -> Vec<u8> {
    let mut encoded = Canonical::new("definition/unit/v1");
    encoded.field(2, value.identity().as_bytes());
    encoded.u64(3, value.version());
    encoded.field(4, &owner_bytes(value.owner()));
    encoded.field(5, value.code().as_bytes());
    encoded.field(6, value.dimension().as_bytes());
    encoded.u64(7, u64::from(value.scale()));
    encoded.u64(8, u64::from(value.precision()));
    encoded.finish()
}

fn rule_pack_bytes(value: &MarketRulePack) -> Vec<u8> {
    let mut encoded = Canonical::new("definition/rule-pack/v1");
    encoded.field(2, value.identity().as_bytes());
    encoded.u64(3, value.version());
    encoded.field(4, &owner_bytes(value.owner()));
    encoded.field(5, value.market().as_bytes());
    encoded.field(6, value.rule_type().as_bytes());
    encoded.field(7, value.source().as_bytes());
    encoded.field(8, &period_bytes(value.effective()));
    encoded.field(9, &[2]);
    encoded.field(10, value.content_hash().as_bytes());
    encoded.finish()
}

struct Canonical(Vec<u8>);

impl Canonical {
    fn new(schema: &str) -> Self {
        let mut value = Self(Vec::new());
        value.0.extend_from_slice(b"FCMD");
        value.0.extend_from_slice(&1_u16.to_be_bytes());
        value.field(1, schema.as_bytes());
        value
    }

    fn field(&mut self, tag: u8, value: &[u8]) {
        self.0.push(tag);
        self.0
            .extend_from_slice(&(value.len() as u64).to_be_bytes());
        self.0.extend_from_slice(value);
    }

    fn u64(&mut self, tag: u8, value: u64) {
        self.field(tag, &value.to_be_bytes());
    }

    fn finish(self) -> Vec<u8> {
        self.0
    }
}

fn owner_bytes(value: &OwnerRef) -> Vec<u8> {
    let mut encoded = Canonical::new("owner-ref/v1");
    encoded.field(2, value.tenant_id().as_str().as_bytes());
    encoded.field(3, value.owner_id().as_str().as_bytes());
    encoded.finish()
}

fn version_ref_bytes(value: &VersionRef) -> Vec<u8> {
    let mut encoded = Canonical::new("version-ref/v1");
    encoded.field(2, value.id().as_str().as_bytes());
    encoded.u64(3, value.version().get());
    encoded.finish()
}

fn unit_ref_bytes(value: &UnitRef) -> Vec<u8> {
    let mut encoded = Canonical::new("unit-ref/v1");
    encoded.field(2, value.unit_id().as_str().as_bytes());
    encoded.u64(3, value.version().get());
    encoded.finish()
}

fn decimal_bytes(value: &DecimalValue) -> Vec<u8> {
    let mut encoded = Canonical::new("decimal/v1");
    encoded.field(2, value.coefficient().as_bytes());
    encoded.u64(3, u64::from(value.scale()));
    encoded.field(4, &unit_ref_bytes(value.unit()));
    encoded.finish()
}

fn market_time_bytes(value: &MarketTime) -> Vec<u8> {
    let mut encoded = Canonical::new("market-time/v1");
    encoded.field(2, &value.instant().timestamp().to_be_bytes());
    encoded.field(3, &value.instant().timestamp_subsec_nanos().to_be_bytes());
    encoded.field(4, value.market_timezone().as_bytes());
    encoded.field(5, value.local_trading_date().to_string().as_bytes());
    encoded.finish()
}

fn period_bytes(value: &EffectivePeriod) -> Vec<u8> {
    let mut encoded = Canonical::new("effective-period/v1");
    encoded.field(2, &market_time_bytes(value.from()));
    encoded.field(3, &market_time_bytes(value.to()));
    encoded.finish()
}

fn decimal(value: &str, scale: u32, unit: UnitRef) -> DecimalValue {
    DecimalValue::new(value, scale, unit).unwrap()
}

fn fixed_int(value: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(value * FixedDecimal::ONE.scaled())
}

fn fixed_rate(basis_points: i128) -> FixedDecimal {
    FixedDecimal::from_scaled(basis_points * 10_000_000_000)
}

fn owner() -> OwnerRef {
    OwnerRef::new(id('T'), id('Y'))
}

fn unit_requirements() -> Vec<RatesUnitRequirement> {
    [
        ('C', "currency_amount"),
        ('P', "price_per_100"),
        ('R', "rate"),
        ('V', "years"),
        ('W', "years_squared"),
        ('Q', "dv01_per_100"),
        ('J', "dv01"),
        ('N', "dimensionless"),
        ('H', "contract_count"),
    ]
    .into_iter()
    .map(|(suffix, dimension)| RatesUnitRequirement::new(unit_ref(suffix), dimension))
    .collect()
}

fn unit_ref(suffix: char) -> UnitRef {
    UnitRef::new(id(suffix), version())
}

fn version() -> Version {
    Version::new(1).unwrap()
}

fn id(suffix: char) -> Ulid {
    let suffix = match suffix {
        'I' => '1',
        'L' => '2',
        'O' => '3',
        'U' => '4',
        value => value,
    };
    Ulid::new(format!("01ARZ3NDEKTSV4RRFFQ69G5FA{suffix}")).unwrap()
}

fn time(hour: u32) -> MarketTime {
    time_for(2026, 8, 3, hour)
}

fn time_for(year: i32, month: u32, day: u32, hour: u32) -> MarketTime {
    MarketTime::new(
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
            .single()
            .unwrap(),
        "Asia/Shanghai",
        date(year, month, day),
    )
    .unwrap()
}

fn time_with_nanos(hour: u32, nanos: u32) -> MarketTime {
    let instant = format!("2026-08-03T{hour:02}:00:00.{nanos:09}Z")
        .parse()
        .unwrap();
    MarketTime::new(instant, "Asia/Shanghai", date(2026, 8, 3)).unwrap()
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn trace() -> SafeTraceContext {
    SafeTraceContext::new("0123456789abcdef0123456789abcdef").unwrap()
}

fn unavailable() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::StorageUnavailable, false)
}

fn validation() -> ApplicationError {
    ApplicationError::new(ApplicationErrorCategory::ValidationFailed, false)
}
