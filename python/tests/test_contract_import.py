from __future__ import annotations

import sys
from pathlib import Path


GENERATED_ROOT = (
    Path(__file__).resolve().parents[1]
    / "node-contracts"
    / "src"
    / "ficant_contracts"
    / "generated"
)
sys.path.insert(0, str(GENERATED_ROOT))

from ficant.app.v1.registry_pb2 import AppRegistry  # noqa: E402
from ficant.app.v1.session_pb2 import Session  # noqa: E402
from ficant.core.v1.common_pb2 import DecimalValue, MarketTime, OwnerRef, Sha256, Ulid, UnitRef  # noqa: E402
from ficant.core.v1.governance_pb2 import (  # noqa: E402
    PLATFORM_ROLE_RESEARCHER,
    ChangeJustification,
    FoundationChangeRecord,
)
from ficant.core.v1.governance_pb2_grpc import (  # noqa: E402
    FoundationChangeServiceStub,
)
from ficant.core.v1.evidence_pb2 import FORMAL_INPUT_KIND_FACT  # noqa: E402
from ficant.core.v1.subject_pb2 import RegisterSubjectRequest, Subject  # noqa: E402
from ficant.core.v1.subject_state_pb2 import (  # noqa: E402
    RegisterSubjectStateRequest,
    SubjectStateSnapshot,
)
from ficant.market.v1.cgb_futures_rule_pb2 import (  # noqa: E402
    CgbFuturesDeliveryRulePack,
    CgbFuturesProductRule,
)
from ficant.market.v1.funding_rule_pb2 import FundingRulePack, FundingTierRate  # noqa: E402
from ficant.market.v1.tax_rule_pb2 import (  # noqa: E402
    COUPON_TAX_CLAIM_SCOPE_COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT,
    GROSS_COUPON_TAX_BASIS_VAT_INCLUDED,
    TAX_ROUNDING_MODE_TIES_TO_EVEN,
    BondCouponTaxRule,
    BondCouponTaxTreatmentRule,
    SubjectCouponTaxRate,
    SubjectCouponTaxTreatment,
    TaxRulePack,
    TaxRulePackV2,
)
from ficant.market.v1.data_source_pb2 import (  # noqa: E402
    DataSourceAuthorization,
    InstrumentMapping,
)
from ficant.market.v1.data_source_pb2_grpc import (  # noqa: E402
    DataSourceRegistryServiceStub,
)
from ficant.market.v1.definition_pb2 import (  # noqa: E402
    BondTaxAttributes,
    CompleteInstrumentDefinition,
    MarketDefinition,
)
from ficant.market.v1.definition_pb2_grpc import (  # noqa: E402
    MarketDefinitionServiceStub,
)
from ficant.market.v1.fact_pb2 import (  # noqa: E402
    CASHFLOW_TYPE_COUPON,
    Cashflow,
    CurvePointSet,
    CurveSnapshotInput,
    GetCurveSnapshotRequest,
    MarketFact,
    PublishCurveSnapshotRequest,
    QueryInstrumentFactsRequest,
    VALUATION_VALUE_ROLE_REMAINING_YEARS,
    VALUATION_VALUE_ROLE_YIELD,
    Valuation,
)
from ficant.market.v1.fact_pb2_grpc import MarketFactServiceStub  # noqa: E402
from ficant.market.v1.instrument_pb2 import Instrument  # noqa: E402
from ficant.market.v1.rule_pb2 import MarketRulePack  # noqa: E402
from ficant.rates.v1.analytics_pb2 import (  # noqa: E402
    ANALYSIS_INPUT_ROLE_DATA_SNAPSHOT,
    ANALYSIS_INPUT_ROLE_CURVE_RULE_PACK,
    ANALYSIS_INPUT_ROLE_CURVE_NODE_DEFINITION,
    AnalysisContext,
    AnalysisInputBinding,
    AnalyzeBondRequest,
    AnalyzeCarryRollRequest,
    AnalyzeFuturesDeliveryRequest,
    AnalyzeFuturesDeliveryResult,
    AnalyzeFuturesHedgeRequest,
    ArtifactBinding,
    CurveNodeBinding,
    FuturesDeliveryCandidateResult,
    FuturesDeliveryMeasures,
    InterpolateYieldCurveRequest,
    ObjectBinding,
    ParameterDigest,
    ResultMetadata,
    SnapshotBinding,
    TaxAdjustedBondAnalytics,
)
from ficant.rates.v1.analytics_pb2_grpc import RatesAnalyticsServiceStub  # noqa: E402
from ficant.portfolio.v1.portfolio_pb2 import (  # noqa: E402
    PORTFOLIO_PAGE_DATA_MODE_REAL,
    PORTFOLIO_STATUS_ACTIVE,
    Book,
    D01Projection,
    PortfolioCoverage,
    PortfolioOverview,
    PortfolioPageEnvelope,
)
from ficant.portfolio.v1 import portfolio_pb2  # noqa: E402
from ficant.research.v1.experiment_pb2 import ExperimentRun  # noqa: E402
from ficant.research.v1 import artifact_pb2  # noqa: E402
from ficant.research.v1.artifact_pb2_grpc import ArtifactServiceStub  # noqa: E402
from ficant.research.v1.coverage_pb2 import (  # noqa: E402
    CoverageDeclaration,
    PriceSourceSummary,
)
from ficant.research.v1 import coverage_pb2_grpc  # noqa: E402
from ficant.research.v1.exposure_pb2 import PortfolioKeyRateExposure  # noqa: E402
from ficant.research.v1.health_pb2 import (  # noqa: E402
    DataHealthReport,
    DataHealthThresholdProfile,
    POSITION_SET_STATE_VERIFIED_EMPTY,
)
from ficant.research.v1.health_pb2_grpc import DataHealthServiceStub  # noqa: E402
from ficant.research.v1.position_pb2 import CapitalUse, PositionViews  # noqa: E402
from ficant.research.v1.snapshot_pb2 import DataSnapshot  # noqa: E402
from ficant.research.v1.snapshot_pb2_grpc import SnapshotServiceStub  # noqa: E402


def test_representative_generated_messages_import_from_one_descriptor() -> None:
    owner = OwnerRef(tenant_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAT"), owner_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAP"))
    subject_request = RegisterSubjectRequest(
        subject=Subject(
            subject_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAS"),
            display_name="consumer Subject",
            owner=owner,
        ),
        idempotency_key="fixture",
    )
    state_request = RegisterSubjectStateRequest(
        snapshot=SubjectStateSnapshot(owner=owner),
        idempotency_key="subject-state-consumer-v1",
    )
    instrument = Instrument(instrument_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAV"))
    decimal = DecimalValue(coefficient="10025", scale=2)
    run = ExperimentRun(seed=7)
    registry = AppRegistry()
    session = Session(active_role=PLATFORM_ROLE_RESEARCHER)
    change = ChangeJustification(reason="human-approved")
    change_record = FoundationChangeRecord(
        active_role=PLATFORM_ROLE_RESEARCHER,
        change=change,
    )
    complete_instrument = CompleteInstrumentDefinition(instrument=instrument)
    definition = MarketDefinition(instrument=complete_instrument)
    fact = MarketFact()
    valuation = Valuation(
        values=[DecimalValue(), DecimalValue()],
        value_roles=[
            VALUATION_VALUE_ROLE_YIELD,
            VALUATION_VALUE_ROLE_REMAINING_YEARS,
        ],
    )
    curve_input = CurveSnapshotInput()
    curve_publish = PublishCurveSnapshotRequest(
        points=CurvePointSet(),
        curve=curve_input,
    )
    fact_query = QueryInstrumentFactsRequest(knowledge_at=MarketTime())
    curve_query = GetCurveSnapshotRequest(knowledge_at=MarketTime())
    authorization = DataSourceAuthorization()
    data_snapshot = DataSnapshot()
    mapping = InstrumentMapping()
    snapshot_binding = SnapshotBinding(
        snapshot_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAA"),
        content_hash=Sha256(value=b"s" * 32),
    )
    artifact_binding = ArtifactBinding(
        artifact_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAB"),
        content_hash=Sha256(value=b"a" * 32),
    )
    curve_node_binding = CurveNodeBinding(
        curve_node_id="cn.gov.yield-curve.10y",
        content_hash=Sha256(value=b"f" * 32),
    )
    input_binding = AnalysisInputBinding(
        role=ANALYSIS_INPUT_ROLE_DATA_SNAPSHOT,
        snapshot=snapshot_binding,
    )
    parameter_digest = ParameterDigest(canonical_parameters_sha256=Sha256(value=b"p" * 32))
    result_metadata = ResultMetadata(
        consumed_inputs=[input_binding],
        parameter_digest=parameter_digest,
        request_fingerprint=Sha256(value=b"r" * 32),
    )
    rates_request = AnalyzeBondRequest(
        calendar=ObjectBinding(),
        data_snapshot=snapshot_binding,
        tax_rule_pack=ObjectBinding(),
    )
    curve_request = InterpolateYieldCurveRequest(curve=snapshot_binding)
    carry_request = AnalyzeCarryRollRequest(curve=snapshot_binding)
    delivery_request = AnalyzeFuturesDeliveryRequest(
        data_snapshot=snapshot_binding,
        funding_rule_pack=ObjectBinding(),
        tax_rule_pack=ObjectBinding(),
    )
    hedge_request = AnalyzeFuturesHedgeRequest(
        target_risk_artifact=artifact_binding,
        delivery_artifact=artifact_binding,
        ctd_analytics_artifact=artifact_binding,
    )
    rule_pack = MarketRulePack()
    cgb_pack = CgbFuturesDeliveryRulePack(delivery_months=[3, 6, 9, 12])
    cgb_product = CgbFuturesProductRule(product_code="T")
    funding_pack = FundingRulePack(rates=[FundingTierRate()])
    tax_pack = TaxRulePack(
        coupon_rules=[
            BondCouponTaxRule(rates=[SubjectCouponTaxRate(value_added_tax_profile="synthetic")])
        ]
    )
    rate_unit = UnitRef(
        unit_id=Ulid(value="01K2CGBVAT0000000000000000"),
        version=1,
    )
    tax_treatment = SubjectCouponTaxTreatment(
        value_added_tax_profile="general-taxpayer",
        income_tax_profile="general-enterprise",
        value_added_tax_rate=DecimalValue(coefficient="6", scale=2, unit=rate_unit),
        income_tax_rate=DecimalValue(coefficient="0", scale=0, unit=rate_unit),
        gross_coupon_basis=GROSS_COUPON_TAX_BASIS_VAT_INCLUDED,
        rounding=TAX_ROUNDING_MODE_TIES_TO_EVEN,
        claim_scope=COUPON_TAX_CLAIM_SCOPE_COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT,
    )
    tax_pack_v2 = TaxRulePackV2(
        coupon_rules=[
            BondCouponTaxTreatmentRule(
                first_issue_from="2025-08-08",
                tax_attributes=BondTaxAttributes(),
                treatments=[tax_treatment],
            )
        ]
    )
    delivery_measures = FuturesDeliveryMeasures(
        tax_adjusted_interim_coupons=DecimalValue(
            coefficient="2830188679245", scale=12, unit=rate_unit
        ),
        subject_tax_adjusted_irr=DecimalValue(
            coefficient="123456789", scale=12, unit=rate_unit
        ),
    )
    delivery_result = AnalyzeFuturesDeliveryResult(
        candidates=[
            FuturesDeliveryCandidateResult(
                measures=delivery_measures,
                claim_scope=(
                    COUPON_TAX_CLAIM_SCOPE_COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT
                ),
            )
        ],
        subject_ctd_index=1,
    )
    after_tax = TaxAdjustedBondAnalytics(
        claim_scope=COUPON_TAX_CLAIM_SCOPE_COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT
    )
    tax_attributes = BondTaxAttributes()
    coverage = CoverageDeclaration(
        imported_position_count=2,
        participating_position_count=1,
        source_confidence=PriceSourceSummary(),
    )
    portfolio = PortfolioKeyRateExposure(coverage=coverage)
    views = PositionViews(coverage=coverage)
    capital = CapitalUse(coverage=coverage)
    health = DataHealthReport(
        threshold_profile=DataHealthThresholdProfile(),
        position_set_state=POSITION_SET_STATE_VERIFIED_EMPTY,
        coverage=CoverageDeclaration(),
    )
    book = Book(status=PORTFOLIO_STATUS_ACTIVE)
    portfolio_coverage = PortfolioCoverage(participation=coverage)
    overview = PortfolioOverview(coverage=portfolio_coverage)
    page = PortfolioPageEnvelope(
        schema_version="portfolio-workbench.v1",
        data_mode=PORTFOLIO_PAGE_DATA_MODE_REAL,
        d01=D01Projection(overview=overview),
        coverage=portfolio_coverage,
    )

    assert instrument.DESCRIPTOR.full_name == "ficant.market.v1.Instrument"
    assert decimal.DESCRIPTOR.full_name == "ficant.core.v1.DecimalValue"
    assert run.DESCRIPTOR.full_name == "ficant.research.v1.ExperimentRun"
    assert registry.DESCRIPTOR.full_name == "ficant.app.v1.AppRegistry"
    assert session.active_role == PLATFORM_ROLE_RESEARCHER
    assert Session.DESCRIPTOR.fields_by_name["actor_id"].number == 6
    assert Session.DESCRIPTOR.fields_by_name["allowed_owner_ids"].number == 9
    assert change_record.DESCRIPTOR.full_name == "ficant.core.v1.FoundationChangeRecord"
    assert change_record.change.reason == "human-approved"
    assert definition.WhichOneof("definition") == "instrument"
    assert definition.instrument.instrument == instrument
    assert fact.DESCRIPTOR.full_name == "ficant.market.v1.MarketFact"
    assert Cashflow.DESCRIPTOR.fields_by_name["cashflow_type"].number == 10
    assert CASHFLOW_TYPE_COUPON == 1
    assert FORMAL_INPUT_KIND_FACT == 21
    assert Valuation.DESCRIPTOR.fields_by_name["value_roles"].number == 10
    assert list(valuation.value_roles) == [
        VALUATION_VALUE_ROLE_YIELD,
        VALUATION_VALUE_ROLE_REMAINING_YEARS,
    ]
    assert "content_hash" not in CurveSnapshotInput.DESCRIPTOR.fields_by_name
    assert "curve_snapshot" not in PublishCurveSnapshotRequest.DESCRIPTOR.fields_by_name
    assert curve_publish.curve.DESCRIPTOR.full_name == "ficant.market.v1.CurveSnapshotInput"
    assert fact_query.HasField("knowledge_at")
    assert QueryInstrumentFactsRequest.DESCRIPTOR.fields_by_name["knowledge_at"].number == 5
    assert curve_query.HasField("knowledge_at")
    assert GetCurveSnapshotRequest.DESCRIPTOR.fields_by_name["knowledge_at"].number == 2
    assert authorization.DESCRIPTOR.fields_by_name["mapping_hash"].number == 14
    assert data_snapshot.DESCRIPTOR.fields_by_name["authorization_ref"].number == 9
    assert mapping.DESCRIPTOR.fields_by_name["content_hash"].number == 5
    assert rates_request.DESCRIPTOR.full_name == "ficant.rates.v1.AnalyzeBondRequest"
    assert AnalysisContext.DESCRIPTOR.fields_by_name["knowledge_at"].number == 9
    assert "rule_pack" not in AnalysisContext.DESCRIPTOR.fields_by_name
    assert input_binding.WhichOneof("binding") == "snapshot"
    assert input_binding.snapshot.snapshot_id.value == "01ARZ3NDEKTSV4RRFFQ69G5FAA"
    assert result_metadata.consumed_inputs[0].role == ANALYSIS_INPUT_ROLE_DATA_SNAPSHOT
    assert ANALYSIS_INPUT_ROLE_CURVE_RULE_PACK == 15
    assert ANALYSIS_INPUT_ROLE_CURVE_NODE_DEFINITION == 16
    curve_node_input_binding = AnalysisInputBinding(
        role=ANALYSIS_INPUT_ROLE_CURVE_NODE_DEFINITION,
        curve_node=curve_node_binding,
    )
    assert curve_node_input_binding.WhichOneof("binding") == "curve_node"
    assert curve_node_input_binding.curve_node.curve_node_id == "cn.gov.yield-curve.10y"
    assert AnalysisInputBinding.DESCRIPTOR.fields_by_name["curve_node"].number == 10
    assert result_metadata.parameter_digest.canonical_parameters_sha256.value == b"p" * 32
    assert result_metadata.request_fingerprint.value == b"r" * 32
    assert rates_request.calendar.DESCRIPTOR.full_name == "ficant.rates.v1.ObjectBinding"
    assert rates_request.data_snapshot.DESCRIPTOR.full_name == "ficant.rates.v1.SnapshotBinding"
    assert "terms" not in AnalyzeBondRequest.DESCRIPTOR.fields_by_name
    assert curve_request.curve.DESCRIPTOR.full_name == "ficant.rates.v1.SnapshotBinding"
    assert carry_request.curve.DESCRIPTOR.full_name == "ficant.rates.v1.SnapshotBinding"
    assert "calendar" not in AnalyzeCarryRollRequest.DESCRIPTOR.fields_by_name
    assert delivery_request.data_snapshot.DESCRIPTOR.full_name == "ficant.rates.v1.SnapshotBinding"
    assert delivery_request.tax_rule_pack.DESCRIPTOR.full_name == "ficant.rates.v1.ObjectBinding"
    assert "candidates" not in AnalyzeFuturesDeliveryRequest.DESCRIPTOR.fields_by_name
    assert hedge_request.target_risk_artifact.DESCRIPTOR.full_name == (
        "ficant.rates.v1.ArtifactBinding"
    )
    assert "target_dv01" not in AnalyzeFuturesHedgeRequest.DESCRIPTOR.fields_by_name
    assert RatesAnalyticsServiceStub.__name__ == "RatesAnalyticsServiceStub"
    assert FoundationChangeServiceStub.__name__ == "FoundationChangeServiceStub"
    assert subject_request.subject.owner == owner
    assert subject_request.idempotency_key == "fixture"
    assert state_request.snapshot.owner == owner
    assert state_request.idempotency_key == "subject-state-consumer-v1"
    assert DataSourceRegistryServiceStub.__name__ == "DataSourceRegistryServiceStub"
    assert MarketDefinitionServiceStub.__name__ == "MarketDefinitionServiceStub"
    assert MarketFactServiceStub.__name__ == "MarketFactServiceStub"
    assert SnapshotServiceStub.__name__ == "SnapshotServiceStub"
    assert ArtifactServiceStub.__name__ == "ArtifactServiceStub"
    artifact = artifact_pb2.Artifact(kind=artifact_pb2.ARTIFACT_KIND_GENERIC)
    artifact_response = artifact_pb2.GetArtifactResponse(artifact=artifact)
    lineage_page = artifact_pb2.LineagePage()
    lineage_response = artifact_pb2.ReadArtifactLineageResponse(
        lineage_page=lineage_page
    )
    assert artifact_response.WhichOneof("result") == "artifact"
    assert lineage_response.WhichOneof("result") == "lineage_page"
    assert {
        method.name
        for method in artifact_pb2.DESCRIPTOR.services_by_name["ArtifactService"].methods
    } == {
        "GetArtifact",
        "GetSignalSet",
        "ReadArtifactLineage",
        "ReadSignalSetLineage",
    }
    assert set(artifact_pb2.ArtifactKind.values()) == {0, 1, 5}
    assert not hasattr(artifact_pb2, "PublishArtifactRequest")
    assert not hasattr(artifact_pb2, "PublishSignalSetRequest")
    assert rule_pack.DESCRIPTOR.fields_by_name["content"].message_type.full_name == "google.protobuf.Any"
    assert cgb_pack.DESCRIPTOR.full_name == "ficant.market.v1.CgbFuturesDeliveryRulePack"
    assert cgb_product.HasField("product_code")
    assert funding_pack.DESCRIPTOR.full_name == "ficant.market.v1.FundingRulePack"
    assert funding_pack.rates[0].DESCRIPTOR.full_name == "ficant.market.v1.FundingTierRate"
    assert tax_pack.DESCRIPTOR.full_name == "ficant.market.v1.TaxRulePack"
    assert tax_pack.coupon_rules[0].rates[0].DESCRIPTOR.full_name == "ficant.market.v1.SubjectCouponTaxRate"
    assert tax_attributes.DESCRIPTOR.full_name == "ficant.market.v1.BondTaxAttributes"
    assert tax_pack_v2.DESCRIPTOR.full_name == "ficant.market.v1.TaxRulePackV2"
    assert tax_pack_v2.coupon_rules[0].treatments[0].value_added_tax_rate.unit == rate_unit
    assert tax_treatment.gross_coupon_basis == GROSS_COUPON_TAX_BASIS_VAT_INCLUDED
    assert tax_treatment.rounding == TAX_ROUNDING_MODE_TIES_TO_EVEN
    assert tax_treatment.claim_scope == (
        COUPON_TAX_CLAIM_SCOPE_COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT
    )
    assert delivery_result.candidates[0].measures.tax_adjusted_interim_coupons.scale == 12
    assert delivery_result.candidates[0].claim_scope != 0
    assert delivery_result.subject_ctd_index == 1
    assert after_tax.claim_scope != 0
    assert coverage.DESCRIPTOR.full_name == "ficant.research.v1.CoverageDeclaration"
    assert coverage_pb2_grpc.__name__ == "ficant.research.v1.coverage_pb2_grpc"
    assert portfolio.coverage.imported_position_count == 2
    assert views.coverage.participating_position_count == 1
    assert capital.coverage.source_confidence.DESCRIPTOR.full_name == (
        "ficant.research.v1.PriceSourceSummary"
    )
    assert health.DESCRIPTOR.full_name == "ficant.research.v1.DataHealthReport"
    assert health.position_set_state == POSITION_SET_STATE_VERIFIED_EMPTY
    assert health.HasField("coverage")
    assert DataHealthServiceStub.__name__ == "DataHealthServiceStub"
    assert book.DESCRIPTOR.full_name == "ficant.portfolio.v1.Book"
    assert page.schema_version == "portfolio-workbench.v1"
    assert page.WhichOneof("projection") == "d01"
    assert page.d01.overview.coverage.participation.imported_position_count == 2
    assert page.coverage.missing_reasons == []
    assert not hasattr(portfolio_pb2, "PORTFOLIO_PAGE_DATA_MODE_DEMO")
    assert {
        method.name
        for method in portfolio_pb2.DESCRIPTOR.services_by_name[
            "PortfolioCatalogService"
        ].methods
    } == {"ListBooksAndPortfolios"}
    assert {
        method.name
        for method in portfolio_pb2.DESCRIPTOR.services_by_name[
            "PortfolioAggregationService"
        ].methods
    } == {"GetPortfolioOverview"}
    assert {
        method.name
        for method in portfolio_pb2.DESCRIPTOR.services_by_name[
            "PortfolioWorkbenchService"
        ].methods
    } == {"GetDefaultContext", "GetPage"}
