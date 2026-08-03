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
from ficant.core.v1.common_pb2 import DecimalValue, Ulid  # noqa: E402
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
from ficant.rates.v1.analytics_pb2 import AnalyzeBondRequest  # noqa: E402
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
    rates_request = AnalyzeBondRequest()
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
