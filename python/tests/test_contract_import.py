"""Verify all generated Python Protobuf/gRPC contracts import and round-trip cleanly."""

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

from ficant.core.v1.common_pb2 import DecimalValue, MarketTime, OwnerRef, Sha256, Ulid, UnitRef  # noqa: E402
from ficant.core.v1.error_pb2 import ERROR_CODE_VALIDATION_FAILED, ErrorDetail  # noqa: E402
from ficant.core.v1.subject_pb2 import FUNDING_TIER_DR_AVAILABLE, Subject, SubjectVersion, TaxTreatment  # noqa: E402
from ficant.core.v1.subject_state_pb2 import LimitCeiling, SubjectStateSnapshot  # noqa: E402
from ficant.market.v1.cgb_futures_rule_pb2 import CgbFuturesDeliveryRulePack  # noqa: E402
from ficant.market.v1.definition_pb2 import Bond, BondTaxAttributes, Calendar  # noqa: E402
from ficant.market.v1.fact_pb2 import PRICE_SOURCE_TYPE_REAL_TRADE, Quote, Trade  # noqa: E402
from ficant.market.v1.funding_rule_pb2 import FundingRulePack  # noqa: E402
from ficant.market.v1.instrument_pb2 import Instrument  # noqa: E402
from ficant.market.v1.regulatory_rule_pb2 import RegulatoryRulePack  # noqa: E402
from ficant.market.v1.rule_pb2 import MarketRulePack  # noqa: E402
from ficant.market.v1.tax_rule_pb2 import TaxRulePackV2  # noqa: E402
from ficant.rates.v1.analytics_pb2 import (  # noqa: E402
    AnalyzeBondRequest,
    AnalyzeCarryRollRequest,
    AnalyzeFuturesDeliveryRequest,
    AnalyzeFuturesHedgeRequest,
    InterpolateYieldCurveRequest,
)
from ficant.rates.v1.analytics_pb2_grpc import RatesAnalyticsServiceStub  # noqa: E402
from ficant.research.v1.coverage_pb2 import CoverageDeclaration  # noqa: E402
from ficant.research.v1.exposure_pb2 import PortfolioKeyRateExposure  # noqa: E402
from ficant.research.v1.factor_pb2 import FactorDefinition  # noqa: E402
from ficant.research.v1.position_pb2 import ACCOUNTING_BOOK_FVOCI, Position, PositionSnapshot  # noqa: E402


def test_core_and_rates_contracts_round_trip() -> None:
    unit = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAA"), version=1)
    amount = DecimalValue(coefficient="100", scale=0, unit=unit)
    raw = amount.SerializeToString()
    decoded = DecimalValue.FromString(raw)
    assert decoded == amount
    assert ERROR_CODE_VALIDATION_FAILED == 1
    assert FUNDING_TIER_DR_AVAILABLE == 1
    assert PRICE_SOURCE_TYPE_REAL_TRADE == 1
    assert ACCOUNTING_BOOK_FVOCI == 2
    assert RatesAnalyticsServiceStub is not None
    assert all(
        cls is not None
        for cls in (
            Subject,
            SubjectVersion,
            TaxTreatment,
            LimitCeiling,
            SubjectStateSnapshot,
            CgbFuturesDeliveryRulePack,
            Bond,
            BondTaxAttributes,
            Calendar,
            Quote,
            Trade,
            FundingRulePack,
            Instrument,
            RegulatoryRulePack,
            MarketRulePack,
            TaxRulePackV2,
            AnalyzeBondRequest,
            InterpolateYieldCurveRequest,
            AnalyzeCarryRollRequest,
            AnalyzeFuturesDeliveryRequest,
            AnalyzeFuturesHedgeRequest,
            CoverageDeclaration,
            PortfolioKeyRateExposure,
            FactorDefinition,
            Position,
            PositionSnapshot,
            ErrorDetail,
            OwnerRef,
            Sha256,
            MarketTime,
        )
    )
