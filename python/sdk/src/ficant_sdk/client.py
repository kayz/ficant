"""Synchronous gRPC client for the Ficant onshore rates pricing service."""

from __future__ import annotations

from collections.abc import Callable
from decimal import Decimal, InvalidOperation
import re
from typing import TypeVar

import grpc
from ficant.core.v1.common_pb2 import DecimalValue
from ficant.core.v1.error_pb2 import ErrorDetail
from ficant.rates.v1 import analytics_pb2
from ficant.rates.v1.analytics_pb2_grpc import RatesAnalyticsServiceStub


_Response = TypeVar("_Response")
_Result = TypeVar("_Result")

_SUPPORTED_EXPLICIT_BOND_MODES = frozenset(
    {
        analytics_pb2.BOND_TAX_BASIS_PRE_TAX,
        analytics_pb2.BOND_TAX_BASIS_TAX_ADJUSTED,
    }
)
_CANONICAL_POSITIVE_INT_RE = re.compile(r"\A[1-9][0-9]*\Z")
_MAX_DECIMAL_PRECISION = 28
_MAX_DECIMAL_SCALE = 28


class RatesError(Exception):
    """Structured business failure returned by ``ficant-server``."""

    def __init__(self, detail: ErrorDetail) -> None:
        super().__init__(detail.message)
        self.detail = detail


class RatesClient:
    """Synchronous client for onshore RMB bond, curve, delivery, and hedge pricing."""

    def __init__(
        self,
        endpoint: str,
        bearer_token: str | None = None,
        *,
        insecure: bool = False,
        timeout_seconds: float = 30.0,
        root_certificates: bytes | None = None,
    ) -> None:
        if not endpoint:
            raise ValueError("endpoint is required")
        if timeout_seconds <= 0:
            raise ValueError("timeout_seconds must be positive")
        self._metadata = (
            (("authorization", f"Bearer {bearer_token}"),) if bearer_token else ()
        )
        self._timeout = timeout_seconds
        if insecure:
            self._channel: grpc.Channel = grpc.insecure_channel(endpoint)
        else:
            credentials = grpc.ssl_channel_credentials(
                root_certificates=root_certificates
            )
            self._channel = grpc.secure_channel(endpoint, credentials)
        self._stub = RatesAnalyticsServiceStub(self._channel)

    def close(self) -> None:
        self._channel.close()

    def __enter__(self) -> RatesClient:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def analyze_bond(
        self, request: analytics_pb2.AnalyzeBondRequest
    ) -> analytics_pb2.AnalyzeBondResult:
        result = self._call(self._stub.AnalyzeBond, request, "analysis")
        _validate_analyze_bond_result(request, result)
        return result

    def interpolate_yield_curve(
        self, request: analytics_pb2.InterpolateYieldCurveRequest
    ) -> analytics_pb2.InterpolateYieldCurveResult:
        return self._call(self._stub.InterpolateYieldCurve, request, "point")

    def analyze_carry_roll(
        self, request: analytics_pb2.AnalyzeCarryRollRequest
    ) -> analytics_pb2.AnalyzeCarryRollResult:
        return self._call(self._stub.AnalyzeCarryRoll, request, "analysis")

    def analyze_futures_delivery(
        self, request: analytics_pb2.AnalyzeFuturesDeliveryRequest
    ) -> analytics_pb2.AnalyzeFuturesDeliveryResult:
        return self._call(self._stub.AnalyzeFuturesDelivery, request, "analysis")

    def analyze_futures_hedge(
        self, request: analytics_pb2.AnalyzeFuturesHedgeRequest
    ) -> analytics_pb2.AnalyzeFuturesHedgeResult:
        result = self._call(self._stub.AnalyzeFuturesHedge, request, "analysis")
        _validate_analyze_futures_hedge_result(request, result)
        return result

    def _call(
        self,
        operation: Callable[..., _Response],
        request: object,
        result_field: str,
    ) -> _Result:
        response = operation(request, metadata=self._metadata, timeout=self._timeout)
        branch = response.WhichOneof("result")
        if branch == "error":
            raise RatesError(response.error)
        if branch != result_field:
            raise RuntimeError("ficant-server returned an incomplete rates response")
        return getattr(response, result_field)


def _validate_analyze_bond_result(
    request: analytics_pb2.AnalyzeBondRequest,
    result: analytics_pb2.AnalyzeBondResult,
) -> None:
    if request.execution_mode != analytics_pb2.BOND_TAX_BASIS_UNSPECIFIED:
        if request.execution_mode not in _SUPPORTED_EXPLICIT_BOND_MODES:
            raise RuntimeError(
                "AnalyzeBond response failed execution_mode validation: "
                f"unsupported request execution_mode={request.execution_mode}"
            )
        if result.execution_mode not in _SUPPORTED_EXPLICIT_BOND_MODES:
            raise RuntimeError(
                "AnalyzeBond response failed execution_mode validation: "
                f"unsupported response execution_mode={result.execution_mode}"
            )
        if result.execution_mode != request.execution_mode:
            raise RuntimeError(
                "AnalyzeBond response failed execution_mode validation: "
                f"expected {request.execution_mode}, got {result.execution_mode}"
            )
        if result.measures_tax_basis != analytics_pb2.BOND_TAX_BASIS_PRE_TAX:
            raise RuntimeError(
                "AnalyzeBond response failed measures_tax_basis validation: "
                f"expected BOND_TAX_BASIS_PRE_TAX, got {result.measures_tax_basis}"
            )
        if request.execution_mode == analytics_pb2.BOND_TAX_BASIS_PRE_TAX:
            if result.HasField("after_tax"):
                raise RuntimeError(
                    "AnalyzeBond response failed after_tax validation: "
                    "unexpected after_tax sub-result for BOND_TAX_BASIS_PRE_TAX"
                )
        elif request.execution_mode == analytics_pb2.BOND_TAX_BASIS_TAX_ADJUSTED:
            if not result.HasField("after_tax"):
                raise RuntimeError(
                    "AnalyzeBond response failed after_tax validation: "
                    "missing after_tax sub-result for BOND_TAX_BASIS_TAX_ADJUSTED"
                )
            if result.after_tax.tax_basis != analytics_pb2.BOND_TAX_BASIS_TAX_ADJUSTED:
                raise RuntimeError(
                    "AnalyzeBond response failed after_tax.tax_basis validation: "
                    "expected BOND_TAX_BASIS_TAX_ADJUSTED, "
                    f"got {result.after_tax.tax_basis}"
                )


def _validate_positive_canonical_decimal(
    value: DecimalValue,
    rpc_name: str,
    field_name: str,
) -> None:
    coeff = value.coefficient
    scale = value.scale
    if not isinstance(coeff, str) or not _CANONICAL_POSITIVE_INT_RE.fullmatch(coeff):
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"non-canonical or non-positive coefficient={coeff!r}"
        )
    if len(coeff) > _MAX_DECIMAL_PRECISION:
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"coefficient precision exceeds {_MAX_DECIMAL_PRECISION}"
        )
    if (
        isinstance(scale, bool)
        or not isinstance(scale, int)
        or scale < 0
        or scale > _MAX_DECIMAL_SCALE
    ):
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"invalid scale={scale!r}"
        )
    if scale > 0 and coeff.endswith("0"):
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"non-canonical trailing zero in coefficient={coeff!r} with scale={scale}"
        )
    try:
        decoded = Decimal(f"{coeff}e-{scale}")
    except InvalidOperation as exc:
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"invalid decimal representation ({coeff!r}, scale={scale})"
        ) from exc
    if not decoded.is_finite() or decoded <= 0:
        raise RuntimeError(
            f"{rpc_name} response failed {field_name} validation: "
            f"value must be finite and strictly positive, got {decoded}"
        )


def _validate_analyze_futures_hedge_result(
    request: analytics_pb2.AnalyzeFuturesHedgeRequest,
    result: analytics_pb2.AnalyzeFuturesHedgeResult,
) -> None:
    if not result.HasField("measures"):
        raise RuntimeError(
            "AnalyzeFuturesHedge response failed measures validation: "
            "missing measures"
        )
    if not result.measures.HasField("contract_notional"):
        raise RuntimeError(
            "AnalyzeFuturesHedge response failed contract_notional validation: "
            "missing measures.contract_notional"
        )
    notional = result.measures.contract_notional
    _validate_positive_canonical_decimal(
        notional,
        "AnalyzeFuturesHedge",
        "contract_notional",
    )
    if (
        not request.HasField("units")
        or not request.units.HasField("currency_amount")
        or not request.units.currency_amount.HasField("unit_id")
    ):
        raise RuntimeError(
            "AnalyzeFuturesHedge response failed contract_notional.unit validation: "
            "request missing units.currency_amount"
        )
    if not notional.HasField("unit") or not notional.unit.HasField("unit_id"):
        raise RuntimeError(
            "AnalyzeFuturesHedge response failed contract_notional.unit validation: "
            "missing contract_notional.unit"
        )
    expected_unit = request.units.currency_amount
    actual_unit = notional.unit
    if (
        actual_unit.unit_id.value != expected_unit.unit_id.value
        or actual_unit.version != expected_unit.version
    ):
        raise RuntimeError(
            "AnalyzeFuturesHedge response failed contract_notional.unit validation: "
            f"expected ({expected_unit.unit_id.value!r}, v{expected_unit.version}), "
            f"got ({actual_unit.unit_id.value!r}, v{actual_unit.version})"
        )
