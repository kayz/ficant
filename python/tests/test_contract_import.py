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
from ficant.core.v1.common_pb2 import DecimalValue, Sha256, Ulid  # noqa: E402
from ficant.market.v1.cgb_futures_rule_pb2 import (  # noqa: E402
    CgbFuturesDeliveryRulePack,
    CgbFuturesProductRule,
)
from ficant.market.v1.funding_rule_pb2 import FundingRulePack, FundingTierRate  # noqa: E402
from ficant.market.v1.tax_rule_pb2 import (  # noqa: E402
    BondCouponTaxRule,
    SubjectCouponTaxRate,
    TaxRulePack,
)
from ficant.market.v1.definition_pb2 import BondTaxAttributes  # noqa: E402
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
    AnalyzeFuturesHedgeRequest,
    ArtifactBinding,
    CurveNodeBinding,
    InterpolateYieldCurveRequest,
    ObjectBinding,
    ParameterDigest,
    ResultMetadata,
    SnapshotBinding,
)
from ficant.rates.v1.analytics_pb2_grpc import RatesAnalyticsServiceStub  # noqa: E402
from ficant.research.v1.experiment_pb2 import ExperimentRun  # noqa: E402
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


def test_representative_generated_messages_import_from_one_descriptor() -> None:
    instrument = Instrument(instrument_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAV"))
    decimal = DecimalValue(coefficient="10025", scale=2)
    run = ExperimentRun(seed=7)
    registry = AppRegistry()
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

    assert instrument.DESCRIPTOR.full_name == "ficant.market.v1.Instrument"
    assert decimal.DESCRIPTOR.full_name == "ficant.core.v1.DecimalValue"
    assert run.DESCRIPTOR.full_name == "ficant.research.v1.ExperimentRun"
    assert registry.DESCRIPTOR.full_name == "ficant.app.v1.AppRegistry"
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
    assert "candidates" not in AnalyzeFuturesDeliveryRequest.DESCRIPTOR.fields_by_name
    assert hedge_request.target_risk_artifact.DESCRIPTOR.full_name == (
        "ficant.rates.v1.ArtifactBinding"
    )
    assert "target_dv01" not in AnalyzeFuturesHedgeRequest.DESCRIPTOR.fields_by_name
    assert RatesAnalyticsServiceStub.__name__ == "RatesAnalyticsServiceStub"
    assert rule_pack.DESCRIPTOR.fields_by_name["content"].message_type.full_name == "google.protobuf.Any"
    assert cgb_pack.DESCRIPTOR.full_name == "ficant.market.v1.CgbFuturesDeliveryRulePack"
    assert cgb_product.HasField("product_code")
    assert funding_pack.DESCRIPTOR.full_name == "ficant.market.v1.FundingRulePack"
    assert funding_pack.rates[0].DESCRIPTOR.full_name == "ficant.market.v1.FundingTierRate"
    assert tax_pack.DESCRIPTOR.full_name == "ficant.market.v1.TaxRulePack"
    assert tax_pack.coupon_rules[0].rates[0].DESCRIPTOR.full_name == "ficant.market.v1.SubjectCouponTaxRate"
    assert tax_attributes.DESCRIPTOR.full_name == "ficant.market.v1.BondTaxAttributes"
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
