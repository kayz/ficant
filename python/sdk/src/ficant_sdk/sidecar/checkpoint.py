"""Single-writer atomic JSON checkpoints, /healthz probing, and ledger transition gating."""

from __future__ import annotations

from collections.abc import Callable, Mapping
from dataclasses import dataclass
import hashlib
import http.client
import json
import os
from pathlib import Path
import ssl
import tempfile
import threading
from typing import Any
from urllib.parse import urlsplit

from ficant_sdk.sidecar.codecs import (
    is_canonical_ulid,
    validate_iso_date,
    validate_non_empty_text,
)

CHECKPOINT_SCHEMA_VERSION = "ficant.sidecar.checkpoint.v1"

_FORBIDDEN_CREDENTIAL_KEYS = frozenset(
    {
        "authorization",
        "bearer_token",
        "credential",
        "credentials",
        "password",
        "secret",
        "token",
        "token_digest",
        "token_sha256",
        "token_sha256_hex",
        "x_ficant_credential",
        "x-ficant-credential",
    }
)

_ACTIVE_LOCK_PATHS_GUARD = threading.Lock()
_ACTIVE_LOCK_PATHS: set[Path] = set()

if os.name == "nt":
    import msvcrt
else:
    import fcntl


class SidecarError(Exception):
    """Base exception for Ficant reference sidecar runtime errors."""


class CheckpointLockError(SidecarError):
    """Raised when another writer already holds the single-writer checkpoint lock."""


class CheckpointCorruptionError(SidecarError):
    """Raised when a checkpoint file fails schema or integrity validation."""


class HealthCheckError(SidecarError):
    """Raised when ``/healthz`` is unreachable, unhealthy, disabled, or malformed."""


class LedgerEpochRegressionError(SidecarError):
    """Raised when the server's ``ledger_epoch`` regresses below the expected/checkpoint epoch."""


class LedgerInstanceMismatchError(SidecarError):
    """Raised when ``ledger_instance_id`` mismatches on the same epoch or expected identity."""


class LedgerRolloverValidationError(SidecarError):
    """Raised when an epoch advance fails rollover invariants (new instance/date/input)."""


@dataclass(frozen=True)
class LedgerIdentity:
    """Validated ``(ledger_epoch, ledger_instance_id)`` pair."""

    ledger_epoch: int
    ledger_instance_id: str

    def __post_init__(self) -> None:
        if (
            isinstance(self.ledger_epoch, bool)
            or not isinstance(self.ledger_epoch, int)
            or self.ledger_epoch < 1
        ):
            raise ValueError("ledger_epoch must be a positive integer")
        if not is_canonical_ulid(self.ledger_instance_id):
            raise ValueError(
                "ledger_instance_id must be a canonical 26-character uppercase ULID"
            )

    def as_tuple(self) -> tuple[int, str]:
        return (self.ledger_epoch, self.ledger_instance_id)


@dataclass(frozen=True)
class HealthzStatus:
    """Parsed payload from ``GET /healthz``."""

    status: str
    service: str
    storage_mode: str
    blotter_healthy: bool
    ledger_epoch: int | None
    ledger_instance_id: str | None
    web_ui_mode: str = "disabled"
    embedded_assets_count: int = 0

    def require_healthy_ledger(self) -> LedgerIdentity:
        """Validate health and extract a non-disabled ``LedgerIdentity``."""
        if self.status != "ok":
            raise HealthCheckError(
                f"unhealthy /healthz status={self.status!r} (expected 'ok')"
            )
        if self.service != "ficant-server":
            raise HealthCheckError(
                f"unexpected /healthz service={self.service!r} (expected 'ficant-server')"
            )
        if self.blotter_healthy is not True:
            raise HealthCheckError("/healthz reports blotter_healthy=False")
        if self.storage_mode == "disabled":
            raise HealthCheckError(
                "/healthz reports storage_mode='disabled'; blotter sidecars require active storage"
            )
        if (
            isinstance(self.ledger_epoch, bool)
            or not isinstance(self.ledger_epoch, int)
            or self.ledger_epoch < 1
        ):
            raise HealthCheckError(
                f"/healthz returned invalid ledger_epoch={self.ledger_epoch!r}"
            )
        if not is_canonical_ulid(self.ledger_instance_id):
            raise HealthCheckError(
                f"/healthz returned invalid ledger_instance_id={self.ledger_instance_id!r}"
            )
        assert self.ledger_instance_id is not None
        return LedgerIdentity(
            ledger_epoch=self.ledger_epoch,
            ledger_instance_id=self.ledger_instance_id,
        )


HealthProbeFn = Callable[[], HealthzStatus | Mapping[str, Any]]


def validate_healthz_payload(payload: Mapping[str, Any]) -> HealthzStatus:
    """Validate and convert a decoded ``/healthz`` JSON dict into ``HealthzStatus``."""
    if not isinstance(payload, Mapping):
        raise HealthCheckError("/healthz payload must be a JSON object")

    status = payload.get("status")
    service = payload.get("service")
    storage_mode = payload.get("storage_mode")
    blotter_healthy = payload.get("blotter_healthy")
    ledger_epoch = payload.get("ledger_epoch")
    ledger_instance_id = payload.get("ledger_instance_id")
    web_ui_mode = payload.get("web_ui_mode", "disabled")
    embedded_assets_count = payload.get("embedded_assets_count", 0)

    if not isinstance(status, str) or not isinstance(service, str):
        raise HealthCheckError("/healthz missing string status or service")
    if not isinstance(storage_mode, str) or not storage_mode:
        raise HealthCheckError("/healthz missing storage_mode")
    if not isinstance(blotter_healthy, bool):
        raise HealthCheckError("/healthz blotter_healthy must be a boolean")
    if ledger_epoch is not None and (
        isinstance(ledger_epoch, bool) or not isinstance(ledger_epoch, int)
    ):
        raise HealthCheckError("/healthz ledger_epoch must be an integer or null")
    if ledger_instance_id is not None and not isinstance(ledger_instance_id, str):
        raise HealthCheckError("/healthz ledger_instance_id must be a string or null")
    if not isinstance(web_ui_mode, str):
        raise HealthCheckError("/healthz web_ui_mode must be a string")
    if isinstance(embedded_assets_count, bool) or not isinstance(
        embedded_assets_count, int
    ):
        raise HealthCheckError("/healthz embedded_assets_count must be an integer")

    return HealthzStatus(
        status=status,
        service=service,
        storage_mode=storage_mode,
        blotter_healthy=blotter_healthy,
        ledger_epoch=ledger_epoch,
        ledger_instance_id=ledger_instance_id,
        web_ui_mode=web_ui_mode,
        embedded_assets_count=embedded_assets_count,
    )


def _parse_http_target(endpoint: str, *, insecure: bool) -> tuple[str, str, int]:
    validate_non_empty_text(endpoint, "endpoint")
    if "://" in endpoint:
        parsed = urlsplit(endpoint)
        scheme = parsed.scheme.lower()
        if scheme not in ("http", "https"):
            raise ValueError(f"unsupported endpoint scheme: {scheme!r}")
        host = parsed.hostname
        if not host:
            raise ValueError(f"missing host in endpoint: {endpoint!r}")
        port = parsed.port or (443 if scheme == "https" else 80)
        return (scheme, host, port)

    scheme = "http" if insecure else "https"
    parsed = urlsplit(f"{scheme}://{endpoint}")
    host = parsed.hostname
    if not host:
        raise ValueError(f"missing host in endpoint: {endpoint!r}")
    port = parsed.port or (443 if scheme == "https" else 80)
    return (scheme, host, port)


def probe_healthz(
    endpoint: str,
    *,
    insecure: bool = False,
    ca_cert_path: str | Path | None = None,
    root_certificates: bytes | None = None,
    timeout_seconds: float = 2.0,
) -> HealthzStatus:
    """Perform a bounded HTTP(S) ``GET /healthz`` probe against the same-port server."""
    if timeout_seconds <= 0:
        raise ValueError("timeout_seconds must be positive")
    scheme, host, port = _parse_http_target(endpoint, insecure=insecure)

    conn: http.client.HTTPConnection
    if scheme == "https":
        if ca_cert_path is not None and root_certificates is not None:
            raise ValueError("specify at most one of ca_cert_path or root_certificates")
        if ca_cert_path is not None:
            context = ssl.create_default_context(cafile=str(ca_cert_path))
        elif root_certificates is not None:
            context = ssl.create_default_context(
                cadata=root_certificates.decode("ascii")
            )
        else:
            context = ssl.create_default_context()
        conn = http.client.HTTPSConnection(
            host, port, timeout=timeout_seconds, context=context
        )
    else:
        conn = http.client.HTTPConnection(host, port, timeout=timeout_seconds)

    try:
        conn.request("GET", "/healthz", headers={"Accept": "application/json"})
        resp = conn.getresponse()
        body = resp.read(65_536)
    except (OSError, http.client.HTTPException) as exc:
        raise HealthCheckError(f"failed to probe /healthz at {endpoint!r}: {exc}") from exc
    finally:
        conn.close()

    try:
        decoded = json.loads(body.decode("utf-8"))
    except Exception as exc:
        raise HealthCheckError(
            f"/healthz returned HTTP {resp.status} with non-JSON body"
        ) from exc

    status = validate_healthz_payload(decoded)
    if resp.status != 200:
        raise HealthCheckError(
            f"/healthz returned HTTP {resp.status} (status={status.status!r}, blotter_healthy={status.blotter_healthy})"
        )
    return status


def invoke_health_probe(
    *,
    health_probe: HealthProbeFn | None,
    endpoint: str | None,
    insecure: bool = False,
    ca_cert_path: str | Path | None = None,
    root_certificates: bytes | None = None,
    timeout_seconds: float = 2.0,
) -> HealthzStatus:
    """Execute either an injected test ``health_probe`` or live HTTP ``/healthz`` probe."""
    if health_probe is not None:
        raw = health_probe()
        if isinstance(raw, HealthzStatus):
            return raw
        return validate_healthz_payload(raw)
    if not endpoint:
        raise HealthCheckError(
            "either health_probe or endpoint must be configured to probe /healthz"
        )
    return probe_healthz(
        endpoint,
        insecure=insecure,
        ca_cert_path=ca_cert_path,
        root_certificates=root_certificates,
        timeout_seconds=timeout_seconds,
    )


def compute_file_fingerprint(path: Path) -> str:
    """Compute canonical lowercase hex SHA-256 of an input file's exact bytes."""
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(65_536)
            if not chunk:
                break
            digest.update(chunk)
    return digest.hexdigest()


def _assert_no_secret_keys(obj: Any, path: str = "$") -> None:
    if isinstance(obj, Mapping):
        for key, value in obj.items():
            if not isinstance(key, str):
                raise CheckpointCorruptionError(
                    f"checkpoint JSON key at {path} must be a string"
                )
            if key.lower() in _FORBIDDEN_CREDENTIAL_KEYS:
                raise CheckpointCorruptionError(
                    f"forbidden credential/secret key {key!r} at {path}.{key}"
                )
            _assert_no_secret_keys(value, f"{path}.{key}")
    elif isinstance(obj, list):
        for idx, item in enumerate(obj):
            _assert_no_secret_keys(item, f"{path}[{idx}]")


def _fsync_directory(directory: Path) -> None:
    if os.name == "nt":
        return
    try:
        dir_fd = os.open(str(directory), os.O_RDONLY)
    except OSError:
        return
    try:
        os.fsync(dir_fd)
    except OSError:
        pass
    finally:
        os.close(dir_fd)


class JsonCheckpointStore:
    """Single-writer locked, crash-atomic JSON checkpoint store (``ficant.sidecar.checkpoint.v1``)."""

    def __init__(self, path: str | Path) -> None:
        self._path = Path(path)
        self._lock_path = Path(f"{self._path}.lock")
        self._canonical_lock_key: Path | None = None
        self._lock_fd: int | None = None

    @property
    def path(self) -> Path:
        return self._path

    @property
    def lock_path(self) -> Path:
        return self._lock_path

    @property
    def is_locked(self) -> bool:
        return self._lock_fd is not None

    def acquire(self) -> None:
        """Acquire exclusive non-blocking single-writer lock for this checkpoint."""
        if self._lock_fd is not None:
            return
        self._path.parent.mkdir(parents=True, exist_ok=True)
        canonical_key = self._lock_path.resolve()

        with _ACTIVE_LOCK_PATHS_GUARD:
            if canonical_key in _ACTIVE_LOCK_PATHS:
                raise CheckpointLockError(
                    f"checkpoint lock already held in this process: {self._lock_path}"
                )
            _ACTIVE_LOCK_PATHS.add(canonical_key)

        fd: int | None = None
        try:
            fd = os.open(str(self._lock_path), os.O_CREAT | os.O_RDWR, 0o600)
            if os.name == "nt":
                os.lseek(fd, 0, os.SEEK_SET)
                # Ensure at least 1 byte exists for msvcrt byte-range lock.
                if os.fstat(fd).st_size == 0:
                    os.write(fd, b"\0")
                    os.lseek(fd, 0, os.SEEK_SET)
                try:
                    msvcrt.locking(fd, msvcrt.LK_NBLCK, 1)
                except OSError as exc:
                    raise CheckpointLockError(
                        f"checkpoint lock is held by another writer: {self._lock_path}"
                    ) from exc
            else:
                try:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except OSError as exc:
                    raise CheckpointLockError(
                        f"checkpoint lock is held by another writer: {self._lock_path}"
                    ) from exc
            self._lock_fd = fd
            self._canonical_lock_key = canonical_key
        except BaseException:
            if fd is not None:
                try:
                    os.close(fd)
                except OSError:
                    pass
            with _ACTIVE_LOCK_PATHS_GUARD:
                _ACTIVE_LOCK_PATHS.discard(canonical_key)
            raise

    def release(self) -> None:
        """Release the exclusive single-writer lock."""
        if self._lock_fd is None:
            return
        fd = self._lock_fd
        canonical_key = self._canonical_lock_key
        self._lock_fd = None
        self._canonical_lock_key = None
        try:
            if os.name == "nt":
                try:
                    os.lseek(fd, 0, os.SEEK_SET)
                    msvcrt.locking(fd, msvcrt.LK_UNLCK, 1)
                except OSError:
                    pass
            else:
                try:
                    fcntl.flock(fd, fcntl.LOCK_UN)
                except OSError:
                    pass
        finally:
            try:
                os.close(fd)
            finally:
                if canonical_key is not None:
                    with _ACTIVE_LOCK_PATHS_GUARD:
                        _ACTIVE_LOCK_PATHS.discard(canonical_key)

    def close(self) -> None:
        self.release()

    def __enter__(self) -> JsonCheckpointStore:
        self.acquire()
        return self

    def __exit__(self, *_: object) -> None:
        self.release()

    def _require_lock(self) -> None:
        if self._lock_fd is None:
            raise CheckpointLockError(
                f"checkpoint store lock must be acquired before accessing {self._path}"
            )

    def load(
        self,
        *,
        expected_kind: str | None = None,
        expected_sidecar_id: str | None = None,
    ) -> dict[str, Any] | None:
        """Load and validate an existing checkpoint JSON file, or return ``None`` if absent."""
        self._require_lock()
        if not self._path.exists():
            return None
        try:
            raw = self._path.read_text(encoding="utf-8")
            data = json.loads(raw)
        except Exception as exc:
            raise CheckpointCorruptionError(
                f"failed to parse checkpoint JSON {self._path}: {exc}"
            ) from exc

        validated = _validate_checkpoint_dict(data)
        if expected_kind is not None and validated["kind"] != expected_kind:
            raise CheckpointCorruptionError(
                f"checkpoint kind mismatch: expected {expected_kind!r}, got {validated['kind']!r}"
            )
        if (
            expected_sidecar_id is not None
            and validated["sidecar_id"] != expected_sidecar_id
        ):
            raise CheckpointCorruptionError(
                f"checkpoint sidecar_id mismatch: expected {expected_sidecar_id!r}, got {validated['sidecar_id']!r}"
            )
        return validated

    def save(self, data: Mapping[str, Any]) -> None:
        """Atomically persist checkpoint JSON via same-directory ``temp + fsync + os.replace``."""
        self._require_lock()
        validated = _validate_checkpoint_dict(data)
        self._path.parent.mkdir(parents=True, exist_ok=True)
        serialized = json.dumps(
            validated, sort_keys=True, indent=2, ensure_ascii=False
        ) + "\n"

        tmp_path: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="w",
                encoding="utf-8",
                dir=str(self._path.parent),
                prefix=f".{self._path.name}.",
                suffix=".tmp",
                delete=False,
            ) as tmp_file:
                tmp_path = Path(tmp_file.name)
                tmp_file.write(serialized)
                tmp_file.flush()
                os.fsync(tmp_file.fileno())
            os.replace(tmp_path, self._path)
            _fsync_directory(self._path.parent)
            tmp_path = None
        finally:
            if tmp_path is not None and tmp_path.exists():
                try:
                    tmp_path.unlink()
                except OSError:
                    pass

    def quarantine_existing(self, *, suffix_tag: str = "restore") -> Path | None:
        """Atomically copy the existing checkpoint to a unique quarantine sibling file."""
        self._require_lock()
        if not self._path.exists():
            return None
        raw_bytes = self._path.read_bytes()
        counter = 1
        while True:
            candidate = self._path.with_name(
                f"{self._path.name}.quarantine.{suffix_tag}.{counter}.json"
            )
            if not candidate.exists():
                tmp_path: Path | None = None
                try:
                    with tempfile.NamedTemporaryFile(
                        mode="wb",
                        dir=str(self._path.parent),
                        prefix=f".{candidate.name}.",
                        suffix=".tmp",
                        delete=False,
                    ) as tmp_file:
                        tmp_path = Path(tmp_file.name)
                        tmp_file.write(raw_bytes)
                        tmp_file.flush()
                        os.fsync(tmp_file.fileno())
                    os.replace(tmp_path, candidate)
                    _fsync_directory(self._path.parent)
                    tmp_path = None
                    return candidate
                finally:
                    if tmp_path is not None and tmp_path.exists():
                        try:
                            tmp_path.unlink()
                        except OSError:
                            pass
            counter += 1


def _validate_checkpoint_dict(data: Any) -> dict[str, Any]:
    if not isinstance(data, Mapping):
        raise CheckpointCorruptionError("checkpoint root must be a JSON object")
    _assert_no_secret_keys(data)

    if data.get("schema_version") != CHECKPOINT_SCHEMA_VERSION:
        raise CheckpointCorruptionError(
            f"unsupported checkpoint schema_version={data.get('schema_version')!r}, "
            f"expected {CHECKPOINT_SCHEMA_VERSION!r}"
        )

    kind = data.get("kind")
    if kind not in ("quotes", "trade_feed", "nlp_draft"):
        raise CheckpointCorruptionError(f"invalid checkpoint kind={kind!r}")

    sidecar_id = data.get("sidecar_id")
    if not isinstance(sidecar_id, str) or not sidecar_id or sidecar_id != sidecar_id.strip():
        raise CheckpointCorruptionError("checkpoint sidecar_id must be a non-empty trimmed string")

    ledger_epoch = data.get("ledger_epoch")
    ledger_instance_id = data.get("ledger_instance_id")
    try:
        LedgerIdentity(ledger_epoch=ledger_epoch, ledger_instance_id=ledger_instance_id)
    except ValueError as exc:
        raise CheckpointCorruptionError(f"invalid checkpoint ledger identity: {exc}") from exc

    trading_date = data.get("trading_date")
    try:
        validate_iso_date(trading_date, "trading_date")
    except ValueError as exc:
        raise CheckpointCorruptionError(str(exc)) from exc

    input_fingerprint = data.get("input_fingerprint")
    if not isinstance(input_fingerprint, str) or not input_fingerprint:
        raise CheckpointCorruptionError("checkpoint input_fingerprint must be a non-empty string")

    confirmed_offset = data.get("confirmed_offset")
    if (
        isinstance(confirmed_offset, bool)
        or not isinstance(confirmed_offset, int)
        or confirmed_offset < 0
    ):
        raise CheckpointCorruptionError("checkpoint confirmed_offset must be a non-negative integer")

    in_flight = data.get("in_flight")
    if in_flight is not None and not isinstance(in_flight, Mapping):
        raise CheckpointCorruptionError("checkpoint in_flight must be an object or null")

    previous_epoch_summary = data.get("previous_epoch_summary")
    if previous_epoch_summary is not None and not isinstance(
        previous_epoch_summary, Mapping
    ):
        raise CheckpointCorruptionError(
            "checkpoint previous_epoch_summary must be an object or null"
        )

    state = data.get("state")
    if not isinstance(state, Mapping):
        raise CheckpointCorruptionError("checkpoint state must be a JSON object")

    return dict(data)


@dataclass(frozen=True)
class LedgerReconcileOutcome:
    """Result of probing ``/healthz`` and reconciling checkpoint ledger identity."""

    ledger: LedgerIdentity
    checkpoint: dict[str, Any]
    rolled_over: bool
    reset_for_restore: bool
    quarantined_previous_in_flight: bool
    quarantine_archive_path: Path | None = None


def build_fresh_checkpoint(
    *,
    kind: str,
    sidecar_id: str,
    ledger: LedgerIdentity,
    trading_date: str,
    input_fingerprint: str,
    initial_state: dict[str, Any],
    previous_epoch_summary: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Construct a fresh v1 checkpoint dictionary."""
    validate_iso_date(trading_date, "trading_date")
    return {
        "schema_version": CHECKPOINT_SCHEMA_VERSION,
        "kind": kind,
        "sidecar_id": sidecar_id,
        "ledger_epoch": ledger.ledger_epoch,
        "ledger_instance_id": ledger.ledger_instance_id,
        "trading_date": trading_date,
        "input_fingerprint": input_fingerprint,
        "confirmed_offset": 0,
        "in_flight": None,
        "previous_epoch_summary": previous_epoch_summary,
        "state": initial_state,
    }


def reconcile_ledger_checkpoint(
    *,
    store: JsonCheckpointStore,
    server_health: HealthzStatus,
    kind: str,
    sidecar_id: str,
    trading_date: str,
    input_fingerprint: str,
    initial_state_factory: Callable[[], dict[str, Any]],
    expected_ledger: tuple[int, str] | LedgerIdentity | None = None,
    reset_checkpoint_for_restore: bool = False,
) -> LedgerReconcileOutcome:
    """Enforce ``/healthz`` validity and strict ledger transition priority rules.

    Check order:
      1. Validate ``/healthz`` (healthy, non-disabled, positive epoch, canonical ULID).
      2. If ``expected_ledger`` is provided, compare server against ``expected_ledger``:
         - First: server epoch < expected epoch -> ``LedgerEpochRegressionError``
         - Second: server instance != expected instance (or epoch > expected) -> ``LedgerInstanceMismatchError``
      3. Compare server against persisted checkpoint (if any):
         - If ``reset_checkpoint_for_restore=True``: archive/quarantine the old checkpoint
           and establish a fresh baseline on the verified target ledger.
         - Otherwise in strict priority order:
           (a) server epoch < checkpoint epoch -> ``LedgerEpochRegressionError``
           (b) server epoch == checkpoint epoch and server instance != checkpoint instance
               -> ``LedgerInstanceMismatchError``
           (c) server epoch > checkpoint epoch (rollover advance) -> require:
               - ``server.ledger_instance_id != checkpoint.ledger_instance_id``
               - ``trading_date > checkpoint.trading_date``
               - ``input_fingerprint != checkpoint.input_fingerprint``
               Quarantine any unacknowledged ``in_flight`` in ``previous_epoch_summary``
               and NEVER replay it against the new ledger.
    """
    validate_non_empty_text(sidecar_id, "sidecar_id")
    current_date = validate_iso_date(trading_date, "trading_date")
    validate_non_empty_text(input_fingerprint, "input_fingerprint")

    # 1. Validate /healthz
    server_ledger = server_health.require_healthy_ledger()

    # 2. Validate against configured expected_ledger (if any)
    if expected_ledger is not None:
        exp = (
            expected_ledger
            if isinstance(expected_ledger, LedgerIdentity)
            else LedgerIdentity(
                ledger_epoch=expected_ledger[0],
                ledger_instance_id=expected_ledger[1],
            )
        )
        if server_ledger.ledger_epoch < exp.ledger_epoch:
            raise LedgerEpochRegressionError(
                f"server ledger_epoch={server_ledger.ledger_epoch} regressed below "
                f"expected ledger_epoch={exp.ledger_epoch}"
            )
        if server_ledger.ledger_epoch == exp.ledger_epoch:
            if server_ledger.ledger_instance_id != exp.ledger_instance_id:
                raise LedgerInstanceMismatchError(
                    f"server ledger_instance_id={server_ledger.ledger_instance_id!r} "
                    f"does not match expected {exp.ledger_instance_id!r} at epoch {exp.ledger_epoch}"
                )
        else:
            raise LedgerInstanceMismatchError(
                f"server ledger_epoch={server_ledger.ledger_epoch} does not match "
                f"configured expected ledger_epoch={exp.ledger_epoch}"
            )

    # 3. Reconcile against persisted checkpoint
    existing = store.load(expected_kind=kind, expected_sidecar_id=sidecar_id)

    if existing is None:
        fresh = build_fresh_checkpoint(
            kind=kind,
            sidecar_id=sidecar_id,
            ledger=server_ledger,
            trading_date=trading_date,
            input_fingerprint=input_fingerprint,
            initial_state=initial_state_factory(),
        )
        store.save(fresh)
        return LedgerReconcileOutcome(
            ledger=server_ledger,
            checkpoint=fresh,
            rolled_over=False,
            reset_for_restore=False,
            quarantined_previous_in_flight=False,
        )

    cp_epoch = existing["ledger_epoch"]
    cp_instance = existing["ledger_instance_id"]
    cp_date = validate_iso_date(existing["trading_date"], "checkpoint.trading_date")
    cp_fingerprint = existing["input_fingerprint"]
    cp_in_flight = existing.get("in_flight")

    if reset_checkpoint_for_restore:
        quarantine_path = store.quarantine_existing(
            suffix_tag=f"e{cp_epoch}-{cp_instance}"
        )
        prev_summary: dict[str, Any] = {
            "reason": "reset_checkpoint_for_restore",
            "ledger_epoch": cp_epoch,
            "ledger_instance_id": cp_instance,
            "trading_date": existing["trading_date"],
            "input_fingerprint": cp_fingerprint,
            "confirmed_offset": existing["confirmed_offset"],
            "quarantined_in_flight": cp_in_flight,
            "quarantine_archive_file": quarantine_path.name if quarantine_path else None,
        }
        fresh = build_fresh_checkpoint(
            kind=kind,
            sidecar_id=sidecar_id,
            ledger=server_ledger,
            trading_date=trading_date,
            input_fingerprint=input_fingerprint,
            initial_state=initial_state_factory(),
            previous_epoch_summary=prev_summary,
        )
        store.save(fresh)
        return LedgerReconcileOutcome(
            ledger=server_ledger,
            checkpoint=fresh,
            rolled_over=False,
            reset_for_restore=True,
            quarantined_previous_in_flight=cp_in_flight is not None,
            quarantine_archive_path=quarantine_path,
        )

    # Strict priority order:
    # First: epoch regression
    if server_ledger.ledger_epoch < cp_epoch:
        raise LedgerEpochRegressionError(
            f"server ledger_epoch={server_ledger.ledger_epoch} regressed below "
            f"checkpoint ledger_epoch={cp_epoch}"
        )

    # Second: same epoch, different instance
    if server_ledger.ledger_epoch == cp_epoch:
        if server_ledger.ledger_instance_id != cp_instance:
            raise LedgerInstanceMismatchError(
                f"same ledger_epoch={cp_epoch} but server ledger_instance_id="
                f"{server_ledger.ledger_instance_id!r} != checkpoint {cp_instance!r}"
            )
        return LedgerReconcileOutcome(
            ledger=server_ledger,
            checkpoint=existing,
            rolled_over=False,
            reset_for_restore=False,
            quarantined_previous_in_flight=False,
        )

    # Third: rollover advance (server_ledger.ledger_epoch > cp_epoch)
    if server_ledger.ledger_instance_id == cp_instance:
        raise LedgerRolloverValidationError(
            f"ledger_epoch advanced ({cp_epoch} -> {server_ledger.ledger_epoch}) "
            f"but ledger_instance_id remained unchanged ({cp_instance!r})"
        )
    if current_date <= cp_date:
        raise LedgerRolloverValidationError(
            f"ledger_epoch advanced ({cp_epoch} -> {server_ledger.ledger_epoch}) "
            f"but trading_date {trading_date!r} did not advance past {existing['trading_date']!r}"
        )
    if input_fingerprint == cp_fingerprint:
        raise LedgerRolloverValidationError(
            f"ledger_epoch advanced ({cp_epoch} -> {server_ledger.ledger_epoch}) "
            "but input_fingerprint matches previous trading day's input"
        )

    prev_summary = {
        "reason": "ledger_rollover_advance",
        "ledger_epoch": cp_epoch,
        "ledger_instance_id": cp_instance,
        "trading_date": existing["trading_date"],
        "input_fingerprint": cp_fingerprint,
        "confirmed_offset": existing["confirmed_offset"],
        "quarantined_in_flight": cp_in_flight,
    }
    rolled = build_fresh_checkpoint(
        kind=kind,
        sidecar_id=sidecar_id,
        ledger=server_ledger,
        trading_date=trading_date,
        input_fingerprint=input_fingerprint,
        initial_state=initial_state_factory(),
        previous_epoch_summary=prev_summary,
    )
    store.save(rolled)
    return LedgerReconcileOutcome(
        ledger=server_ledger,
        checkpoint=rolled,
        rolled_over=True,
        reset_for_restore=False,
        quarantined_previous_in_flight=cp_in_flight is not None,
    )
