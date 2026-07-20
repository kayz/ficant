"""Synchronous gRPC client for the Phase 2 reference analytics service."""

from __future__ import annotations

from collections.abc import Callable
from typing import TypeVar

import grpc
from ficant.core.v1.error_pb2 import ErrorDetail
from ficant.rates.v1 import analytics_pb2
from ficant.rates.v1.analytics_pb2_grpc import RatesAnalyticsServiceStub


_Response = TypeVar("_Response")
_Result = TypeVar("_Result")


class RatesError(Exception):
    """Safe structured business failure returned by ``ficant-server``."""

    def __init__(self, detail: ErrorDetail) -> None:
        super().__init__(detail.message)
        self.detail = detail


class RatesClient:
    """Authenticated synchronous client for all Phase 2A–2D calculations."""

    def __init__(
        self,
        endpoint: str,
        bearer_token: str,
        *,
        insecure: bool = False,
        timeout_seconds: float = 30.0,
        root_certificates: bytes | None = None,
    ) -> None:
        if not endpoint or not bearer_token:
            raise ValueError("endpoint and bearer_token are required")
        if timeout_seconds <= 0:
            raise ValueError("timeout_seconds must be positive")
        self._metadata = (("authorization", f"Bearer {bearer_token}"),)
        self._timeout = timeout_seconds
        if insecure:
            self._channel: grpc.Channel = grpc.insecure_channel(endpoint)
        else:
            credentials = grpc.ssl_channel_credentials(root_certificates=root_certificates)
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
        return self._call(self._stub.AnalyzeBond, request, "analysis")

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
        return self._call(self._stub.AnalyzeFuturesHedge, request, "analysis")

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
