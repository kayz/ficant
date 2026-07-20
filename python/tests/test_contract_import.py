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
from ficant.market.v1.instrument_pb2 import Instrument  # noqa: E402
from ficant.rates.v1.analytics_pb2 import AnalyzeBondRequest  # noqa: E402
from ficant.rates.v1.analytics_pb2_grpc import RatesAnalyticsServiceStub  # noqa: E402
from ficant.research.v1.experiment_pb2 import ExperimentRun  # noqa: E402


def test_representative_generated_messages_import_from_one_descriptor() -> None:
    instrument = Instrument(instrument_id=Ulid(value="01ARZ3NDEKTSV4RRFFQ69G5FAV"))
    decimal = DecimalValue(coefficient="10025", scale=2)
    run = ExperimentRun(seed=7)
    registry = AppRegistry()
    rates_request = AnalyzeBondRequest()

    assert instrument.DESCRIPTOR.full_name == "ficant.market.v1.Instrument"
    assert decimal.DESCRIPTOR.full_name == "ficant.core.v1.DecimalValue"
    assert run.DESCRIPTOR.full_name == "ficant.research.v1.ExperimentRun"
    assert registry.DESCRIPTOR.full_name == "ficant.app.v1.AppRegistry"
    assert rates_request.DESCRIPTOR.full_name == "ficant.rates.v1.AnalyzeBondRequest"
    assert RatesAnalyticsServiceStub.__name__ == "RatesAnalyticsServiceStub"
