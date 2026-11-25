"""Unified CLI entrypoint for Ficant reference sidecars (quotes, trade-feed, nlp-draft)."""

from __future__ import annotations

import argparse
from collections.abc import Sequence
from dataclasses import asdict
import json
import os
from pathlib import Path
import signal
import sys
import time
from typing import Any
from urllib.parse import urlsplit

from ficant.rates.v1 import analytics_pb2 as rates_pb2
import grpc

from ficant_sdk.blotter import BlotterError, DeskBlotterClient, _validate_raw_token
from ficant_sdk.sidecar.backoff import BackoffPolicy
from ficant_sdk.sidecar.checkpoint import LedgerIdentity, SidecarError
from ficant_sdk.sidecar.codecs import parse_units_json
from ficant_sdk.sidecar.nlp_gateway import (
    NlpDraftGatewayConfig,
    NlpDraftGatewaySidecar,
    NlpDraftOnlyClient,
)
from ficant_sdk.sidecar.quotes import (
    MAX_QUOTE_BATCH_SIZE,
    QuoteStreamConfig,
    QuoteStreamSidecar,
)
from ficant_sdk.sidecar.trade_feed import (
    MAX_TRADE_FEED_BATCH_SIZE,
    TradeFeedConfig,
    TradeFeedSidecar,
)


def _positive_int(raw: str) -> int:
    try:
        value = int(raw, 10)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"must be a positive integer, got {raw!r}") from exc
    if value < 1:
        raise argparse.ArgumentTypeError(f"must be >= 1, got {value}")
    return value


def _batch_size_int(max_size: int):
    def _parse(raw: str) -> int:
        val = _positive_int(raw)
        if val > max_size:
            raise argparse.ArgumentTypeError(
                f"batch-size must be in 1..={max_size}, got {val}"
            )
        return val

    return _parse


def _add_common_flags(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--endpoint",
        required=True,
        help="Same-port ficant-server host:port or http(s)://host:port endpoint.",
    )
    auth_group = parser.add_mutually_exclusive_group(required=True)
    auth_group.add_argument(
        "--token-env",
        help="Environment variable name holding the sidecar authentication token.",
    )
    auth_group.add_argument(
        "--token-file",
        type=Path,
        help="UTF-8 file path containing the sidecar authentication token.",
    )
    parser.add_argument(
        "--header-mode",
        choices=("bearer", "x-ficant-credential"),
        default="bearer",
        help="Authentication header format (default: bearer).",
    )
    tls_group = parser.add_mutually_exclusive_group(required=False)
    tls_group.add_argument(
        "--insecure",
        action="store_true",
        default=False,
        help="Use plaintext HTTP/gRPC transport (loopback / trusted local dev only).",
    )
    tls_group.add_argument(
        "--ca-cert",
        type=Path,
        default=None,
        help="PEM CA certificate path for TLS verification on both gRPC and /healthz.",
    )
    parser.add_argument(
        "--checkpoint",
        type=Path,
        required=True,
        help="Path to the single-writer atomic JSON checkpoint file.",
    )
    parser.add_argument(
        "--trading-date",
        required=True,
        help="Trading date in YYYY-MM-DD format.",
    )
    parser.add_argument(
        "--sidecar-id",
        required=True,
        help="Stable sidecar instance identifier used in checkpoint and idempotency keys.",
    )
    parser.add_argument(
        "--input",
        type=Path,
        required=True,
        help="Path to the sidecar input file (CSV/JSONL for quotes/trade-feed; JSONL for nlp-draft).",
    )
    mode_group = parser.add_mutually_exclusive_group(required=True)
    mode_group.add_argument(
        "--once",
        action="store_true",
        default=False,
        help="Process available records once and exit.",
    )
    mode_group.add_argument(
        "--poll-interval-ms",
        type=_positive_int,
        default=None,
        help="Continuously poll input at the given interval in milliseconds.",
    )
    parser.add_argument(
        "--expected-ledger-epoch",
        type=_positive_int,
        default=None,
        help="Expected server ledger_epoch (must be paired with --expected-ledger-instance).",
    )
    parser.add_argument(
        "--expected-ledger-instance",
        default=None,
        help="Expected server 26-char uppercase ULID ledger_instance_id (paired with --expected-ledger-epoch).",
    )
    parser.add_argument(
        "--reset-checkpoint-for-restore",
        action="store_true",
        default=False,
        help="Archive/quarantine an existing checkpoint and initialize a fresh baseline on the verified target ledger.",
    )
    parser.add_argument(
        "--units-json",
        default=None,
        help="Optional AnalysisUnits JSON file path or inline JSON object.",
    )
    parser.add_argument(
        "--max-retries",
        type=int,
        default=3,
        help="Maximum bounded retries on transient transport errors (default: 3).",
    )
    parser.add_argument(
        "--rpc-timeout-seconds",
        type=float,
        default=30.0,
        help="Per-RPC gRPC deadline in seconds (default: 30.0).",
    )
    parser.add_argument(
        "--health-timeout-seconds",
        type=float,
        default=2.0,
        help="Timeout in seconds for /healthz probes (default: 2.0).",
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="ficant-sidecar",
        description="Ficant intranet Desk Blotter reference sidecars.",
    )
    subparsers = parser.add_subparsers(dest="subcommand", required=True)

    # quotes
    quotes_parser = subparsers.add_parser(
        "quotes",
        help="Ingest market quote CSV/JSONL into DeskBlotterService/IngestMarketQuotes.",
    )
    _add_common_flags(quotes_parser)
    quotes_parser.add_argument(
        "--batch-size",
        type=_batch_size_int(MAX_QUOTE_BATCH_SIZE),
        default=64,
        help=f"Max ticks per batch (1..={MAX_QUOTE_BATCH_SIZE}, default: 64).",
    )
    quotes_parser.add_argument(
        "--allow-snapshot-discovery",
        action="store_true",
        default=False,
        help="Opt-in to discovering per-account AnalysisUnits via GetAccountSnapshot.",
    )
    quotes_parser.add_argument(
        "--input-format",
        choices=("auto", "csv", "jsonl"),
        default="auto",
        help="Input file format (default: auto).",
    )

    # trade-feed
    trade_parser = subparsers.add_parser(
        "trade-feed",
        help="Ingest trade fill CSV/JSONL into DeskBlotterService/IngestTradeFeed.",
    )
    _add_common_flags(trade_parser)
    trade_parser.add_argument(
        "--batch-size",
        type=_batch_size_int(MAX_TRADE_FEED_BATCH_SIZE),
        default=64,
        help=f"Max fills per batch (1..={MAX_TRADE_FEED_BATCH_SIZE}, default: 64).",
    )
    trade_parser.add_argument(
        "--allow-snapshot-discovery",
        action="store_true",
        default=False,
        help="Opt-in to discovering per-account AnalysisUnits via GetAccountSnapshot.",
    )
    trade_parser.add_argument(
        "--input-format",
        choices=("auto", "csv", "jsonl"),
        default="auto",
        help="Input file format (default: auto).",
    )
    trade_parser.add_argument(
        "--source-id",
        required=True,
        help="Authoritative trade feed source_id.",
    )
    trade_parser.add_argument(
        "--heartbeat-interval-ms",
        type=_positive_int,
        default=30_000,
        help="Trade feed heartbeat interval in milliseconds (default: 30000).",
    )

    # nlp-draft
    nlp_parser = subparsers.add_parser(
        "nlp-draft",
        help="Parse NLP intent JSONL and submit drafts via NlpDraftOnlyClient.",
    )
    _add_common_flags(nlp_parser)
    nlp_parser.add_argument(
        "--adapter",
        choices=("token", "webhook", "spi"),
        default="token",
        help="NLP parser adapter (default: token).",
    )
    nlp_parser.add_argument(
        "--webhook-url",
        default=None,
        help="Configured intranet webhook URL (required when --adapter=webhook).",
    )
    nlp_parser.add_argument(
        "--webhook-timeout-ms",
        type=_positive_int,
        default=5_000,
        help="Webhook HTTP timeout in milliseconds (default: 5000).",
    )
    nlp_parser.add_argument(
        "--parser-source",
        default="reference-token-parser",
        help="Parser source identifier (default: reference-token-parser).",
    )
    nlp_parser.add_argument(
        "--model-version",
        default="1.0.0",
        help="Parser model version string (default: 1.0.0).",
    )

    return parser


def _normalize_endpoints(
    raw_endpoint: str, *, insecure: bool, ca_cert: Path | None
) -> tuple[str, str]:
    """Return ``(grpc_target, healthz_endpoint)`` while rejecting scheme/TLS conflicts."""
    trimmed = raw_endpoint.strip()
    if not trimmed:
        raise ValueError("--endpoint must be non-empty")
    if "://" in trimmed:
        parsed = urlsplit(trimmed)
        scheme = parsed.scheme.lower()
        if scheme not in ("http", "https"):
            raise ValueError(f"unsupported --endpoint scheme: {scheme!r}")
        if scheme == "https" and insecure:
            raise ValueError("cannot combine https:// --endpoint with --insecure")
        if scheme == "http" and ca_cert is not None:
            raise ValueError("cannot combine http:// --endpoint with --ca-cert")
        if not parsed.hostname:
            raise ValueError(f"missing host in --endpoint: {raw_endpoint!r}")
        port = parsed.port or (443 if scheme == "https" else 80)
        host_fmt = f"[{parsed.hostname}]" if ":" in parsed.hostname else parsed.hostname
        grpc_target = f"{host_fmt}:{port}"
        return (grpc_target, trimmed)
    return (trimmed, trimmed)


def _load_token(token_env: str | None, token_file: Path | None) -> str:
    if token_env is not None:
        raw = os.environ.get(token_env)
        if raw is None or not raw:
            raise ValueError(
                f"authentication token environment variable {token_env!r} is unset or empty"
            )
        return _validate_raw_token(raw, f"env:{token_env}")
    assert token_file is not None
    if not token_file.is_file():
        raise ValueError(f"authentication token file does not exist: {token_file}")
    raw = token_file.read_text(encoding="utf-8").strip()
    return _validate_raw_token(raw, f"file:{token_file}")


def _resolve_expected_ledger(
    epoch: int | None, instance: str | None
) -> tuple[int, str] | None:
    if epoch is None and instance is None:
        return None
    if epoch is None or instance is None:
        raise ValueError(
            "--expected-ledger-epoch and --expected-ledger-instance must be provided together"
        )
    ident = LedgerIdentity(ledger_epoch=epoch, ledger_instance_id=instance)
    return ident.as_tuple()


def _resolve_units(units_arg: str | None) -> rates_pb2.AnalysisUnits | None:
    if units_arg is None:
        return None
    trimmed = units_arg.strip()
    if trimmed.startswith("{"):
        return parse_units_json(trimmed)
    path = Path(trimmed)
    if not path.is_file():
        raise ValueError(f"--units-json file does not exist: {path}")
    return parse_units_json(path)


def _load_ca_cert_bytes(ca_cert: Path | None) -> bytes | None:
    if ca_cert is None:
        return None
    if not ca_cert.is_file():
        raise ValueError(f"--ca-cert file does not exist: {ca_cert}")
    return ca_cert.read_bytes()


def _token_kwargs(token: str, header_mode: str) -> dict[str, Any]:
    if header_mode == "bearer":
        return {"bearer_token": token, "header_mode": "bearer"}
    return {"x_ficant_credential": token, "header_mode": "x-ficant-credential"}


def _emit_report(report: Any) -> None:
    print(json.dumps(asdict(report), ensure_ascii=False, sort_keys=True), flush=True)


def _run_loop(
    run_once_fn,
    *,
    once: bool,
    poll_interval_ms: int | None,
) -> int:
    if once:
        report = run_once_fn()
        _emit_report(report)
        return 0

    assert poll_interval_ms is not None
    stop_requested = False

    def _handle_signal(_signum: int, _frame: Any) -> None:
        nonlocal stop_requested
        stop_requested = True

    prev_int = signal.getsignal(signal.SIGINT)
    prev_term = signal.getsignal(signal.SIGTERM) if hasattr(signal, "SIGTERM") else None
    try:
        signal.signal(signal.SIGINT, _handle_signal)
        if prev_term is not None:
            signal.signal(signal.SIGTERM, _handle_signal)
    except ValueError:
        # Running outside main thread (e.g. unit test harness)
        prev_int = None
        prev_term = None

    try:
        while not stop_requested:
            report = run_once_fn()
            _emit_report(report)
            if stop_requested:
                break
            time.sleep(poll_interval_ms / 1_000.0)
        return 0
    finally:
        if prev_int is not None:
            signal.signal(signal.SIGINT, prev_int)
        if prev_term is not None:
            signal.signal(signal.SIGTERM, prev_term)


def _execute(args: argparse.Namespace) -> int:
    grpc_target, healthz_endpoint = _normalize_endpoints(
        args.endpoint, insecure=args.insecure, ca_cert=args.ca_cert
    )
    token = _load_token(args.token_env, args.token_file)
    expected_ledger = _resolve_expected_ledger(
        args.expected_ledger_epoch, args.expected_ledger_instance
    )
    explicit_units = _resolve_units(args.units_json)
    root_certs = _load_ca_cert_bytes(args.ca_cert)
    backoff = BackoffPolicy(max_retries=args.max_retries)
    tok_kwargs = _token_kwargs(token, args.header_mode)

    if args.subcommand == "quotes":
        config = QuoteStreamConfig(
            sidecar_id=args.sidecar_id,
            trading_date=args.trading_date,
            checkpoint_path=args.checkpoint,
            input_path=args.input,
            batch_size=args.batch_size,
            input_format=args.input_format,
            expected_ledger=expected_ledger,
            reset_checkpoint_for_restore=args.reset_checkpoint_for_restore,
            allow_snapshot_discovery=args.allow_snapshot_discovery,
            explicit_units=explicit_units,
            backoff=backoff,
            endpoint=healthz_endpoint,
            insecure=args.insecure,
            ca_cert_path=args.ca_cert,
            root_certificates=None if args.ca_cert is not None else root_certs,
            health_timeout_seconds=args.health_timeout_seconds,
        )
        with DeskBlotterClient(
            grpc_target,
            **tok_kwargs,
            expected_ledger=expected_ledger,
            insecure=args.insecure,
            timeout_seconds=args.rpc_timeout_seconds,
            root_certificates=root_certs,
        ) as client:
            sidecar = QuoteStreamSidecar(config, client)
            return _run_loop(
                sidecar.run_once,
                once=args.once,
                poll_interval_ms=args.poll_interval_ms,
            )

    if args.subcommand == "trade-feed":
        config_tf = TradeFeedConfig(
            sidecar_id=args.sidecar_id,
            source_id=args.source_id,
            trading_date=args.trading_date,
            checkpoint_path=args.checkpoint,
            input_path=args.input,
            batch_size=args.batch_size,
            input_format=args.input_format,
            heartbeat_interval_seconds=args.heartbeat_interval_ms / 1_000.0,
            expected_ledger=expected_ledger,
            reset_checkpoint_for_restore=args.reset_checkpoint_for_restore,
            allow_snapshot_discovery=args.allow_snapshot_discovery,
            explicit_units=explicit_units,
            backoff=backoff,
            endpoint=healthz_endpoint,
            insecure=args.insecure,
            ca_cert_path=args.ca_cert,
            root_certificates=None if args.ca_cert is not None else root_certs,
            health_timeout_seconds=args.health_timeout_seconds,
        )
        with DeskBlotterClient(
            grpc_target,
            **tok_kwargs,
            expected_ledger=expected_ledger,
            insecure=args.insecure,
            timeout_seconds=args.rpc_timeout_seconds,
            root_certificates=root_certs,
        ) as client:
            sidecar_tf = TradeFeedSidecar(config_tf, client)
            return _run_loop(
                sidecar_tf.run_once,
                once=args.once,
                poll_interval_ms=args.poll_interval_ms,
            )

    if args.subcommand == "nlp-draft":
        config_nlp = NlpDraftGatewayConfig(
            sidecar_id=args.sidecar_id,
            trading_date=args.trading_date,
            checkpoint_path=args.checkpoint,
            input_path=args.input,
            adapter=args.adapter,
            parser_source_id=args.parser_source,
            model_version=args.model_version,
            webhook_url=args.webhook_url,
            webhook_timeout_ms=args.webhook_timeout_ms,
            expected_ledger=expected_ledger,
            reset_checkpoint_for_restore=args.reset_checkpoint_for_restore,
            explicit_units=explicit_units,
            backoff=backoff,
            endpoint=healthz_endpoint,
            insecure=args.insecure,
            ca_cert_path=args.ca_cert,
            root_certificates=None if args.ca_cert is not None else root_certs,
            health_timeout_seconds=args.health_timeout_seconds,
        )
        with NlpDraftOnlyClient(
            grpc_target,
            **tok_kwargs,
            expected_ledger=expected_ledger,
            insecure=args.insecure,
            timeout_seconds=args.rpc_timeout_seconds,
            root_certificates=root_certs,
        ) as client:
            sidecar_nlp = NlpDraftGatewaySidecar(config_nlp, client)
            return _run_loop(
                sidecar_nlp.run_once,
                once=args.once,
                poll_interval_ms=args.poll_interval_ms,
            )

    raise ValueError(f"unsupported subcommand: {args.subcommand!r}")


def main(argv: Sequence[str] | None = None) -> int:
    """CLI entrypoint returning process exit status code."""
    parser = build_parser()
    try:
        args = parser.parse_args(argv)
    except SystemExit as exc:
        return int(exc.code) if isinstance(exc.code, int) else 2

    try:
        return _execute(args)
    except (ValueError, SidecarError, BlotterError, grpc.RpcError, OSError) as exc:
        print(f"ficant-sidecar error: {exc}", file=sys.stderr)
        return 1
