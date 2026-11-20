"""Verify generated Python Protobuf/gRPC contracts, legacy wire compatibility, and SDK fail-closed validation."""

from __future__ import annotations

from collections.abc import Callable, Iterator
from concurrent import futures
from contextlib import contextmanager
import importlib.util
from pathlib import Path
import sys
from types import ModuleType

import grpc
import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
GENERATED_ROOT = (
    REPO_ROOT
    / "python"
    / "node-contracts"
    / "src"
    / "ficant_contracts"
    / "generated"
)
SDK_ROOT = REPO_ROOT / "python" / "sdk" / "src"
sys.path.insert(0, str(GENERATED_ROOT))
sys.path.insert(0, str(SDK_ROOT))

from ficant.core.v1.common_pb2 import DecimalValue, MarketTime, OwnerRef, Sha256, Ulid, UnitRef  # noqa: E402
from ficant.core.v1.error_pb2 import (  # noqa: E402
    ERROR_CODE_HASH_MISMATCH,
    ERROR_CODE_VALIDATION_FAILED,
    ErrorDetail,
)
from ficant.core.v1.subject_pb2 import FUNDING_TIER_DR_AVAILABLE, Subject, SubjectVersion, TaxTreatment  # noqa: E402
from ficant.core.v1.subject_state_pb2 import LimitCeiling, SubjectStateSnapshot  # noqa: E402
from ficant.market.v1.cgb_futures_rule_pb2 import CgbFuturesDeliveryRulePack  # noqa: E402
from ficant.market.v1.definition_pb2 import Bond, BondTaxAttributes, Calendar  # noqa: E402
from ficant.market.v1.fact_pb2 import PRICE_SOURCE_TYPE_REAL_TRADE, Quote, Trade  # noqa: E402
from ficant.market.v1.funding_rule_pb2 import FundingRulePack  # noqa: E402
from ficant.market.v1.instrument_pb2 import Instrument  # noqa: E402
from ficant.market.v1.regulatory_rule_pb2 import RegulatoryRulePack  # noqa: E402
from ficant.market.v1.rule_pb2 import MarketRulePack  # noqa: E402
from ficant.market.v1.tax_rule_pb2 import (  # noqa: E402
    COUPON_TAX_CLAIM_SCOPE_COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT,
    TaxRulePackV2,
)
from ficant.rates.v1.analytics_pb2 import (  # noqa: E402
    BOND_TAX_BASIS_PRE_TAX,
    BOND_TAX_BASIS_TAX_ADJUSTED,
    BOND_TAX_BASIS_UNSPECIFIED,
    AnalysisUnits,
    AnalyzeBondRequest,
    AnalyzeBondResponse,
    AnalyzeBondResult,
    AnalyzeCarryRollRequest,
    AnalyzeFuturesDeliveryRequest,
    AnalyzeFuturesHedgeRequest,
    AnalyzeFuturesHedgeResponse,
    AnalyzeFuturesHedgeResult,
    FuturesHedgeMeasures,
    InterpolateYieldCurveRequest,
    TaxAdjustedBondAnalytics,
)
from ficant.rates.v1.analytics_pb2_grpc import RatesAnalyticsServiceStub  # noqa: E402
from ficant.research.v1.coverage_pb2 import CoverageDeclaration  # noqa: E402
from ficant.research.v1.exposure_pb2 import PortfolioKeyRateExposure  # noqa: E402
from ficant.research.v1.factor_pb2 import FactorDefinition  # noqa: E402
from ficant.research.v1.position_pb2 import ACCOUNTING_BOOK_FVOCI, Position, PositionSnapshot  # noqa: E402
from ficant_sdk import RatesClient, RatesError  # noqa: E402

# Authoritative legacy success wire bytes serialized directly with the baseline
# `analytics_pb2` generated at `main@8a91799767472af5254d23f6355ba8c729d9698e`
# prior to BP-1 proto additions.
BASELINE_LEGACY_PROTO_COMMIT = "8a91799767472af5254d23f6355ba8c729d9698e"
LEGACY_PRETAX_BOND_RESULT_HEX = (
    "0a99010801120a323032362d30382d30381a0a323032362d30382d313022280a0232351001"
    "1a200a1c0a1a303141525a334e44454b5453563452524646513639473546414110012a270a"
    "033130301a200a1c0a1a303141525a334e44454b5453563452524646513639473546414110"
    "01322a0a043130323510011a200a1c0a1a303141525a334e44454b54535634525246465136"
    "394735464141100112e3020a320a0c313431303935383930343131100c1a200a1c0a1a3031"
    "41525a334e44454b54535634525246465136394735464142100112270a033130301a200a1c"
    "0a1a303141525a334e44454b5453563452524646513639473546414210011a340a0e313031"
    "3431303935383930343131100c1a200a1c0a1a303141525a334e44454b5453563452524646"
    "5136394735464142100122280a02323510031a200a1c0a1a30314b32434742564154303030"
    "3030303030303030303030303010012a280a02383510011a200a1c0a1a303141525a334e44"
    "454b54535634525246465136394735464144100132280a02383310011a200a1c0a1a303141"
    "525a334e44454b5453563452524646513639473546414410013a260a0238301a200a1c0a1a"
    "303141525a334e44454b54535634525246465136394735464145100142280a02383310021a"
    "200a1c0a1a303141525a334e44454b545356345252464651363947354641461001"
)
LEGACY_PRETAX_BOND_RESPONSE_HEX = (
    "0a82040a99010801120a323032362d30382d30381a0a323032362d30382d313022280a0232"
    "3510011a200a1c0a1a303141525a334e44454b545356345252464651363947354641411001"
    "2a270a033130301a200a1c0a1a303141525a334e44454b5453563452524646513639473546"
    "41411001322a0a043130323510011a200a1c0a1a303141525a334e44454b54535634525246"
    "465136394735464141100112e3020a320a0c313431303935383930343131100c1a200a1c0a"
    "1a303141525a334e44454b54535634525246465136394735464142100112270a033130301a"
    "200a1c0a1a303141525a334e44454b5453563452524646513639473546414210011a340a0e"
    "3130313431303935383930343131100c1a200a1c0a1a303141525a334e44454b5453563452"
    "5246465136394735464142100122280a02323510031a200a1c0a1a30314b32434742564154"
    "3030303030303030303030303030303010012a280a02383510011a200a1c0a1a303141525a"
    "334e44454b54535634525246465136394735464144100132280a02383310011a200a1c0a1a"
    "303141525a334e44454b5453563452524646513639473546414410013a260a0238301a200a"
    "1c0a1a303141525a334e44454b54535634525246465136394735464145100142280a023833"
    "10021a200a1c0a1a303141525a334e44454b545356345252464651363947354641461001"
)
LEGACY_TAX_ADJUSTED_BOND_RESULT_HEX = (
    "0a99010801120a323032362d30382d30381a0a323032362d30382d313022280a0232351001"
    "1a200a1c0a1a303141525a334e44454b5453563452524646513639473546414110012a270a"
    "033130301a200a1c0a1a303141525a334e44454b5453563452524646513639473546414110"
    "01322a0a043130323510011a200a1c0a1a303141525a334e44454b54535634525246465136"
    "394735464141100112e3020a320a0c313431303935383930343131100c1a200a1c0a1a3031"
    "41525a334e44454b54535634525246465136394735464142100112270a033130301a200a1c"
    "0a1a303141525a334e44454b5453563452524646513639473546414210011a340a0e313031"
    "3431303935383930343131100c1a200a1c0a1a303141525a334e44454b5453563452524646"
    "5136394735464142100122280a02323510031a200a1c0a1a30314b32434742564154303030"
    "3030303030303030303030303010012a280a02383510011a200a1c0a1a303141525a334e44"
    "454b54535634525246465136394735464144100132280a02383310011a200a1c0a1a303141"
    "525a334e44454b5453563452524646513639473546414410013a260a0238301a200a1c0a1a"
    "303141525a334e44454b54535634525246465136394735464145100142280a02383310021a"
    "200a1c0a1a303141525a334e44454b5453563452524646513639473546414610011adf010a"
    "af010801120a323032362d30382d30381a0a323032362d30382d313022330a0d3233353834"
    "3930353636303338100c1a200a1c0a1a303141525a334e44454b5453563452524646513639"
    "473546414110012a270a033130301a200a1c0a1a303141525a334e44454b54535634525246"
    "465136394735464141100132350a0f313032333538343930353636303338100c1a200a1c0a"
    "1a303141525a334e44454b54535634525246465136394735464141100112290a0332333510"
    "041a200a1c0a1a30314b324347425641543030303030303030303030303030303010011801"
)
LEGACY_TAX_ADJUSTED_BOND_RESPONSE_HEX = (
    "0ae4050a99010801120a323032362d30382d30381a0a323032362d30382d313022280a0232"
    "3510011a200a1c0a1a303141525a334e44454b545356345252464651363947354641411001"
    "2a270a033130301a200a1c0a1a303141525a334e44454b5453563452524646513639473546"
    "41411001322a0a043130323510011a200a1c0a1a303141525a334e44454b54535634525246"
    "465136394735464141100112e3020a320a0c313431303935383930343131100c1a200a1c0a"
    "1a303141525a334e44454b54535634525246465136394735464142100112270a033130301a"
    "200a1c0a1a303141525a334e44454b5453563452524646513639473546414210011a340a0e"
    "3130313431303935383930343131100c1a200a1c0a1a303141525a334e44454b5453563452"
    "5246465136394735464142100122280a02323510031a200a1c0a1a30314b32434742564154"
    "3030303030303030303030303030303010012a280a02383510011a200a1c0a1a303141525a"
    "334e44454b54535634525246465136394735464144100132280a02383310011a200a1c0a1a"
    "303141525a334e44454b5453563452524646513639473546414410013a260a0238301a200a"
    "1c0a1a303141525a334e44454b54535634525246465136394735464145100142280a023833"
    "10021a200a1c0a1a303141525a334e44454b5453563452524646513639473546414610011a"
    "df010aaf010801120a323032362d30382d30381a0a323032362d30382d313022330a0d3233"
    "3538343930353636303338100c1a200a1c0a1a303141525a334e44454b5453563452524646"
    "513639473546414110012a270a033130301a200a1c0a1a303141525a334e44454b54535634"
    "525246465136394735464141100132350a0f313032333538343930353636303338100c1a20"
    "0a1c0a1a303141525a334e44454b54535634525246465136394735464141100112290a0332"
    "333510041a200a1c0a1a30314b324347425641543030303030303030303030303030303010"
    "011801"
)
LEGACY_TS_HEDGE_RESULT_HEX = (
    "0ad8010a350a0f323930333737343930373337393539100c1a200a1c0a1a303141525a334e"
    "44454b54535634525246465136394735464147100112330a0d2d3836303934383237353836"
    "32100b1a200a1c0a1a303141525a334e44454b545356345252464651363947354641481001"
    "18f7ffffffffffffffff0122340a0e3131333339373431363634313633100c1a200a1c0a1a"
    "303141525a334e44454b5453563452524646513639473546414710012a290a033939391003"
    "1a200a1c0a1a303141525a334e44454b545356345252464651363947354641481001"
)
LEGACY_TS_HEDGE_RESPONSE_HEX = (
    "0adb010ad8010a350a0f323930333737343930373337393539100c1a200a1c0a1a30314152"
    "5a334e44454b54535634525246465136394735464147100112330a0d2d3836303934383237"
    "35383632100b1a200a1c0a1a303141525a334e44454b545356345252464651363947354641"
    "48100118f7ffffffffffffffff0122340a0e3131333339373431363634313633100c1a200a"
    "1c0a1a303141525a334e44454b5453563452524646513639473546414710012a290a033939"
    "3910031a200a1c0a1a303141525a334e44454b545356345252464651363947354641481001"
)

CURRENCY_UNIT = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAA"), version=2)
OTHER_CURRENCY_UNIT = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAZ"), version=2)
CURRENCY_UNIT_V3 = UnitRef(unit_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAA"), version=3)


def _load_module_by_path(module_name: str, file_path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(module_name, file_path)
    assert spec is not None and spec.loader is not None, f"cannot load {file_path}"
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)
    return module


@contextmanager
def _raw_rates_client(
    *,
    bond_responder: Callable[[bytes], bytes] | None = None,
    hedge_responder: Callable[[bytes], bytes] | None = None,
) -> Iterator[RatesClient]:
    """Starts an in-process gRPC server returning exact raw protobuf bytes."""

    handlers: dict[str, grpc.RpcMethodHandler] = {}
    if bond_responder is not None:
        handlers["AnalyzeBond"] = grpc.unary_unary_rpc_method_handler(
            lambda req_bytes, _ctx: bond_responder(req_bytes),
            request_deserializer=lambda data: data,
            response_serializer=lambda data: data,
        )
    if hedge_responder is not None:
        handlers["AnalyzeFuturesHedge"] = grpc.unary_unary_rpc_method_handler(
            lambda req_bytes, _ctx: hedge_responder(req_bytes),
            request_deserializer=lambda data: data,
            response_serializer=lambda data: data,
        )

    server = grpc.server(futures.ThreadPoolExecutor(max_workers=2))
    server.add_generic_rpc_handlers(
        (grpc.method_handlers_generic_handler("ficant.rates.v1.RatesAnalyticsService", handlers),)
    )
    port = server.add_insecure_port("127.0.0.1:0")
    server.start()
    client = RatesClient(f"127.0.0.1:{port}", insecure=True, timeout_seconds=5.0)
    try:
        yield client
    finally:
        client.close()
        server.stop(grace=0).wait()


def _hedge_request() -> AnalyzeFuturesHedgeRequest:
    return AnalyzeFuturesHedgeRequest(
        units=AnalysisUnits(currency_amount=CURRENCY_UNIT),
        product_code="TS",
    )


def _compliant_pretax_bond_response() -> AnalyzeBondResponse:
    resp = AnalyzeBondResponse.FromString(bytes.fromhex(LEGACY_PRETAX_BOND_RESPONSE_HEX))
    resp.analysis.execution_mode = BOND_TAX_BASIS_PRE_TAX
    resp.analysis.measures_tax_basis = BOND_TAX_BASIS_PRE_TAX
    return resp


def _compliant_tax_adjusted_bond_response() -> AnalyzeBondResponse:
    resp = AnalyzeBondResponse.FromString(bytes.fromhex(LEGACY_TAX_ADJUSTED_BOND_RESPONSE_HEX))
    resp.analysis.execution_mode = BOND_TAX_BASIS_TAX_ADJUSTED
    resp.analysis.measures_tax_basis = BOND_TAX_BASIS_PRE_TAX
    resp.analysis.after_tax.tax_basis = BOND_TAX_BASIS_TAX_ADJUSTED
    return resp


def _compliant_hedge_response() -> AnalyzeFuturesHedgeResponse:
    resp = AnalyzeFuturesHedgeResponse.FromString(bytes.fromhex(LEGACY_TS_HEDGE_RESPONSE_HEX))
    resp.analysis.measures.recommended_contracts = -4
    resp.analysis.measures.contract_notional.CopyFrom(
        DecimalValue(coefficient="2000000", scale=0, unit=CURRENCY_UNIT)
    )
    return resp


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


def test_bp1_wire_field_numbers_and_enum_values_match_frozen_gates() -> None:
    assert BOND_TAX_BASIS_UNSPECIFIED == 0
    assert BOND_TAX_BASIS_PRE_TAX == 1
    assert BOND_TAX_BASIS_TAX_ADJUSTED == 2

    assert AnalyzeBondRequest.DESCRIPTOR.fields_by_name["execution_mode"].number == 9
    assert AnalyzeBondResult.DESCRIPTOR.fields_by_name["execution_mode"].number == 4
    assert AnalyzeBondResult.DESCRIPTOR.fields_by_name["measures_tax_basis"].number == 5
    assert TaxAdjustedBondAnalytics.DESCRIPTOR.fields_by_name["tax_basis"].number == 4

    assert AnalyzeFuturesHedgeRequest.DESCRIPTOR.fields_by_name["delivery_rule_pack"].number == 6
    assert AnalyzeFuturesHedgeRequest.DESCRIPTOR.fields_by_name["valuation_at"].number == 7
    assert FuturesHedgeMeasures.DESCRIPTOR.fields_by_name["contract_notional"].number == 6


def test_explicit_file_path_oracle_loading_is_isolated() -> None:
    futures_oracle = _load_module_by_path(
        "ficant_contract_import_bp1_futures_oracle",
        REPO_ROOT / "tests/oracle/china-rates/bp1_futures_hedge_v2_decimal_oracle.py",
    )
    bond_oracle = _load_module_by_path(
        "ficant_contract_import_bp1_policy_bank_oracle",
        REPO_ROOT / "tests/oracle/china-rates/bp1_policy_bank_pretax_decimal_oracle.py",
    )
    assert callable(futures_oracle.build_expected)
    assert callable(bond_oracle.build_expected)
    assert len(bond_oracle.EXPECTED_BOND_IDS) == 6
    assert len(bond_oracle.EXPECTED_MODES) == 2


def test_legacy_baseline_bytes_decode_with_candidate_generated_classes() -> None:
    assert BASELINE_LEGACY_PROTO_COMMIT == "8a91799767472af5254d23f6355ba8c729d9698e"

    pretax_res = AnalyzeBondResult.FromString(bytes.fromhex(LEGACY_PRETAX_BOND_RESULT_HEX))
    pretax_resp = AnalyzeBondResponse.FromString(bytes.fromhex(LEGACY_PRETAX_BOND_RESPONSE_HEX))
    assert pretax_resp.WhichOneof("result") == "analysis"
    assert pretax_resp.analysis == pretax_res
    assert len(pretax_res.cashflows) == 1
    assert pretax_res.HasField("measures")
    assert not pretax_res.HasField("after_tax")
    assert pretax_res.execution_mode == BOND_TAX_BASIS_UNSPECIFIED
    assert pretax_res.measures_tax_basis == BOND_TAX_BASIS_UNSPECIFIED

    tax_res = AnalyzeBondResult.FromString(bytes.fromhex(LEGACY_TAX_ADJUSTED_BOND_RESULT_HEX))
    tax_resp = AnalyzeBondResponse.FromString(bytes.fromhex(LEGACY_TAX_ADJUSTED_BOND_RESPONSE_HEX))
    assert tax_resp.WhichOneof("result") == "analysis"
    assert tax_resp.analysis == tax_res
    assert tax_res.HasField("after_tax")
    assert (
        tax_res.after_tax.claim_scope
        == COUPON_TAX_CLAIM_SCOPE_COUPON_OUTPUT_VAT_BEFORE_INPUT_CREDIT
    )
    assert tax_res.after_tax.tax_basis == BOND_TAX_BASIS_UNSPECIFIED
    assert tax_res.execution_mode == BOND_TAX_BASIS_UNSPECIFIED
    assert tax_res.measures_tax_basis == BOND_TAX_BASIS_UNSPECIFIED

    hedge_res = AnalyzeFuturesHedgeResult.FromString(bytes.fromhex(LEGACY_TS_HEDGE_RESULT_HEX))
    hedge_resp = AnalyzeFuturesHedgeResponse.FromString(
        bytes.fromhex(LEGACY_TS_HEDGE_RESPONSE_HEX)
    )
    assert hedge_resp.WhichOneof("result") == "analysis"
    assert hedge_resp.analysis == hedge_res
    assert hedge_res.HasField("measures")
    assert hedge_res.measures.recommended_contracts == -9
    assert not hedge_res.measures.HasField("contract_notional")


def test_public_client_against_legacy_pretax_wire_bytes() -> None:
    raw = bytes.fromhex(LEGACY_PRETAX_BOND_RESPONSE_HEX)
    with _raw_rates_client(bond_responder=lambda _: raw) as client:
        # Legacy UNSPECIFIED (0) retains compatibility path.
        ok = client.analyze_bond(
            AnalyzeBondRequest(execution_mode=BOND_TAX_BASIS_UNSPECIFIED)
        )
        assert len(ok.cashflows) == 1
        assert ok.execution_mode == BOND_TAX_BASIS_UNSPECIFIED

        # Explicit PRE_TAX, TAX_ADJUSTED, and unknown non-zero all fail closed.
        for mode in (BOND_TAX_BASIS_PRE_TAX, BOND_TAX_BASIS_TAX_ADJUSTED, 99):
            with pytest.raises(RuntimeError) as exc_info:
                client.analyze_bond(AnalyzeBondRequest(execution_mode=mode))
            msg = str(exc_info.value)
            assert "AnalyzeBond" in msg
            assert "execution_mode" in msg


def test_public_client_against_legacy_tax_adjusted_wire_bytes() -> None:
    raw = bytes.fromhex(LEGACY_TAX_ADJUSTED_BOND_RESPONSE_HEX)
    with _raw_rates_client(bond_responder=lambda _: raw) as client:
        # Legacy UNSPECIFIED (0) retains compatibility path.
        ok = client.analyze_bond(
            AnalyzeBondRequest(execution_mode=BOND_TAX_BASIS_UNSPECIFIED)
        )
        assert ok.HasField("after_tax")
        assert ok.after_tax.tax_basis == BOND_TAX_BASIS_UNSPECIFIED

        # Explicit PRE_TAX, TAX_ADJUSTED, and unknown non-zero all fail closed.
        for mode in (BOND_TAX_BASIS_PRE_TAX, BOND_TAX_BASIS_TAX_ADJUSTED, 99):
            with pytest.raises(RuntimeError) as exc_info:
                client.analyze_bond(AnalyzeBondRequest(execution_mode=mode))
            msg = str(exc_info.value)
            assert "AnalyzeBond" in msg
            assert "execution_mode" in msg

    # Even if top-level tags are present, legacy after_tax lacking tax_basis (0) must fail closed.
    partial = AnalyzeBondResponse.FromString(raw)
    partial.analysis.execution_mode = BOND_TAX_BASIS_TAX_ADJUSTED
    partial.analysis.measures_tax_basis = BOND_TAX_BASIS_PRE_TAX
    assert partial.analysis.after_tax.tax_basis == BOND_TAX_BASIS_UNSPECIFIED
    with _raw_rates_client(bond_responder=lambda _: partial.SerializeToString()) as client:
        with pytest.raises(RuntimeError) as exc_info:
            client.analyze_bond(
                AnalyzeBondRequest(execution_mode=BOND_TAX_BASIS_TAX_ADJUSTED)
            )
        msg = str(exc_info.value)
        assert "AnalyzeBond" in msg
        assert "after_tax.tax_basis" in msg


def test_public_client_against_legacy_ts_hedge_wire_bytes_and_error_oneof() -> None:
    raw_legacy_hedge = bytes.fromhex(LEGACY_TS_HEDGE_RESPONSE_HEX)
    with _raw_rates_client(hedge_responder=lambda _: raw_legacy_hedge) as client:
        with pytest.raises(RuntimeError) as exc_info:
            client.analyze_futures_hedge(_hedge_request())
        msg = str(exc_info.value)
        assert "AnalyzeFuturesHedge" in msg
        assert "contract_notional" in msg

    # Business errors in the error oneof remain RatesError (not RuntimeError).
    bond_err = AnalyzeBondResponse(
        error=ErrorDetail(
            code=ERROR_CODE_VALIDATION_FAILED,
            message="bond validation failed",
        )
    )
    hedge_err = AnalyzeFuturesHedgeResponse(
        error=ErrorDetail(
            code=ERROR_CODE_HASH_MISMATCH,
            message="pack hash mismatch",
        )
    )
    with _raw_rates_client(
        bond_responder=lambda _: bond_err.SerializeToString(),
        hedge_responder=lambda _: hedge_err.SerializeToString(),
    ) as client:
        with pytest.raises(RatesError) as bond_exc:
            client.analyze_bond(AnalyzeBondRequest(execution_mode=99))
        assert bond_exc.value.detail.code == ERROR_CODE_VALIDATION_FAILED

        with pytest.raises(RatesError) as hedge_exc:
            client.analyze_futures_hedge(_hedge_request())
        assert hedge_exc.value.detail.code == ERROR_CODE_HASH_MISMATCH


def test_public_client_analyze_bond_field_level_mutations() -> None:
    # Compliant PRE_TAX and TAX_ADJUSTED responses succeed.
    with _raw_rates_client(
        bond_responder=lambda _: _compliant_pretax_bond_response().SerializeToString()
    ) as client:
        res = client.analyze_bond(AnalyzeBondRequest(execution_mode=BOND_TAX_BASIS_PRE_TAX))
        assert res.execution_mode == BOND_TAX_BASIS_PRE_TAX
        assert res.measures_tax_basis == BOND_TAX_BASIS_PRE_TAX
        assert not res.HasField("after_tax")

    with _raw_rates_client(
        bond_responder=lambda _: _compliant_tax_adjusted_bond_response().SerializeToString()
    ) as client:
        res = client.analyze_bond(
            AnalyzeBondRequest(execution_mode=BOND_TAX_BASIS_TAX_ADJUSTED)
        )
        assert res.execution_mode == BOND_TAX_BASIS_TAX_ADJUSTED
        assert res.measures_tax_basis == BOND_TAX_BASIS_PRE_TAX
        assert res.HasField("after_tax")
        assert res.after_tax.tax_basis == BOND_TAX_BASIS_TAX_ADJUSTED

    def _assert_bond_mutation_fails(
        request_mode: int,
        mutated_resp: AnalyzeBondResponse,
        expected_field: str,
    ) -> None:
        with _raw_rates_client(
            bond_responder=lambda _: mutated_resp.SerializeToString()
        ) as client:
            with pytest.raises(RuntimeError) as exc_info:
                client.analyze_bond(AnalyzeBondRequest(execution_mode=request_mode))
            msg = str(exc_info.value)
            assert "AnalyzeBond" in msg
            assert expected_field in msg

    # 1. Wrong result.execution_mode
    for bad_mode in (BOND_TAX_BASIS_UNSPECIFIED, BOND_TAX_BASIS_TAX_ADJUSTED, 77):
        resp = _compliant_pretax_bond_response()
        resp.analysis.execution_mode = bad_mode
        _assert_bond_mutation_fails(BOND_TAX_BASIS_PRE_TAX, resp, "execution_mode")

    resp_mismatch_tax = _compliant_tax_adjusted_bond_response()
    resp_mismatch_tax.analysis.execution_mode = BOND_TAX_BASIS_PRE_TAX
    _assert_bond_mutation_fails(
        BOND_TAX_BASIS_TAX_ADJUSTED, resp_mismatch_tax, "execution_mode"
    )

    # 2. Unknown non-zero request execution_mode even if server echoes it back
    resp_echo_unknown = _compliant_pretax_bond_response()
    resp_echo_unknown.analysis.execution_mode = 99
    _assert_bond_mutation_fails(99, resp_echo_unknown, "execution_mode")

    # 3. Wrong measures_tax_basis
    for bad_basis in (BOND_TAX_BASIS_UNSPECIFIED, BOND_TAX_BASIS_TAX_ADJUSTED, 88):
        resp = _compliant_pretax_bond_response()
        resp.analysis.measures_tax_basis = bad_basis
        _assert_bond_mutation_fails(BOND_TAX_BASIS_PRE_TAX, resp, "measures_tax_basis")

        resp_t = _compliant_tax_adjusted_bond_response()
        resp_t.analysis.measures_tax_basis = bad_basis
        _assert_bond_mutation_fails(
            BOND_TAX_BASIS_TAX_ADJUSTED, resp_t, "measures_tax_basis"
        )

    # 4. PRE_TAX with unexpected after_tax
    resp_unexpected_after_tax = _compliant_tax_adjusted_bond_response()
    resp_unexpected_after_tax.analysis.execution_mode = BOND_TAX_BASIS_PRE_TAX
    _assert_bond_mutation_fails(
        BOND_TAX_BASIS_PRE_TAX, resp_unexpected_after_tax, "after_tax"
    )

    # 5. TAX_ADJUSTED missing after_tax
    resp_missing_after_tax = _compliant_pretax_bond_response()
    resp_missing_after_tax.analysis.execution_mode = BOND_TAX_BASIS_TAX_ADJUSTED
    _assert_bond_mutation_fails(
        BOND_TAX_BASIS_TAX_ADJUSTED, resp_missing_after_tax, "after_tax"
    )

    # 6. TAX_ADJUSTED wrong after_tax.tax_basis
    for bad_sub_basis in (BOND_TAX_BASIS_UNSPECIFIED, BOND_TAX_BASIS_PRE_TAX, 42):
        resp = _compliant_tax_adjusted_bond_response()
        resp.analysis.after_tax.tax_basis = bad_sub_basis
        _assert_bond_mutation_fails(
            BOND_TAX_BASIS_TAX_ADJUSTED, resp, "after_tax.tax_basis"
        )


def test_public_client_analyze_futures_hedge_field_level_mutations() -> None:
    # Compliant response succeeds.
    with _raw_rates_client(
        hedge_responder=lambda _: _compliant_hedge_response().SerializeToString()
    ) as client:
        ok = client.analyze_futures_hedge(_hedge_request())
        assert ok.measures.recommended_contracts == -4
        assert ok.measures.contract_notional.coefficient == "2000000"
        assert ok.measures.contract_notional.scale == 0
        assert ok.measures.contract_notional.unit == CURRENCY_UNIT

    def _assert_hedge_mutation_fails(
        mutated_resp: AnalyzeFuturesHedgeResponse,
        expected_field: str,
    ) -> None:
        with _raw_rates_client(
            hedge_responder=lambda _: mutated_resp.SerializeToString()
        ) as client:
            with pytest.raises(RuntimeError) as exc_info:
                client.analyze_futures_hedge(_hedge_request())
            msg = str(exc_info.value)
            assert "AnalyzeFuturesHedge" in msg
            assert expected_field in msg

    # 1. Missing measures
    no_measures = AnalyzeFuturesHedgeResponse(analysis=AnalyzeFuturesHedgeResult())
    _assert_hedge_mutation_fails(no_measures, "measures")

    # 2. Missing contract_notional
    no_notional = _compliant_hedge_response()
    no_notional.analysis.measures.ClearField("contract_notional")
    _assert_hedge_mutation_fails(no_notional, "contract_notional")

    # 3. Zero, negative, non-canonical, and invalid Decimal values
    invalid_decimals = [
        ("0", 0),
        ("-2000000", 0),
        ("-1", 2),
        ("02000000", 0),
        ("20000000", 1),
        ("+2000000", 0),
        ("2", 29),
        ("1" + ("0" * 28), 0),
        ("", 0),
        ("NaN", 0),
        ("Infinity", 0),
        ("-Infinity", 0),
        ("1e6", 0),
        ("2000000.0", 0),
        ("not-a-decimal", 0),
        (" 2000000", 0),
    ]
    for coeff, scale in invalid_decimals:
        mutated = _compliant_hedge_response()
        mutated.analysis.measures.contract_notional.CopyFrom(
            DecimalValue(coefficient=coeff, scale=scale, unit=CURRENCY_UNIT)
        )
        _assert_hedge_mutation_fails(mutated, "contract_notional")

    # 4. Missing unit, missing unit_id, wrong unit_id, wrong version
    missing_unit = _compliant_hedge_response()
    missing_unit.analysis.measures.contract_notional.ClearField("unit")
    _assert_hedge_mutation_fails(missing_unit, "contract_notional.unit")

    missing_unit_id = _compliant_hedge_response()
    missing_unit_id.analysis.measures.contract_notional.unit.ClearField("unit_id")
    _assert_hedge_mutation_fails(missing_unit_id, "contract_notional.unit")

    wrong_unit_id = _compliant_hedge_response()
    wrong_unit_id.analysis.measures.contract_notional.unit.CopyFrom(OTHER_CURRENCY_UNIT)
    _assert_hedge_mutation_fails(wrong_unit_id, "contract_notional.unit")

    wrong_unit_version = _compliant_hedge_response()
    wrong_unit_version.analysis.measures.contract_notional.unit.CopyFrom(CURRENCY_UNIT_V3)
    _assert_hedge_mutation_fails(wrong_unit_version, "contract_notional.unit")
