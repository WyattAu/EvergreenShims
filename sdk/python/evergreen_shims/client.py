from typing import Any, Optional
from urllib.request import Request, urlopen
from urllib.error import HTTPError, URLError
import json
import time

from .types import (
    StatusResponse,
    HealthStatus,
    MetricsResponse,
    Metric,
    MetricType,
    ReloadConfigResponse,
    BackupListResponse,
    BackupEntry,
    BackupTriggerResponse,
    MigrationStatusResponse,
    MigrationRecord,
    MigrationApplyResponse,
    MigrationRollbackResponse,
)
from .exceptions import APIError, ConnectionError, TimeoutError


def _parse_health_status(value: int) -> HealthStatus:
    try:
        return HealthStatus(value)
    except ValueError:
        return HealthStatus.UNSPECIFIED


def _parse_metric_type(value: int) -> MetricType:
    try:
        return MetricType(value)
    except ValueError:
        return MetricType.UNSPECIFIED


def _parse_status_response(data: dict) -> StatusResponse:
    return StatusResponse(
        health=_parse_health_status(data.get("health", 0)),
        version=data.get("version", ""),
        uptime_seconds=data.get("uptime_seconds", "0"),
        capabilities=[
            {
                "name": c.get("name", ""),
                "enabled": c.get("enabled", False),
                "healthy": c.get("healthy", False),
                "last_error": c.get("last_error", ""),
            }
            for c in data.get("capabilities", [])
        ],
    )


def _parse_metrics_response(data: dict) -> MetricsResponse:
    return MetricsResponse(
        metrics=[
            Metric(
                name=m.get("name", ""),
                value=m.get("value", 0.0),
                labels=m.get("labels", {}),
                type=_parse_metric_type(m.get("type", 0)),
            )
            for m in data.get("metrics", [])
        ]
    )


def _parse_reload_config_response(data: dict) -> ReloadConfigResponse:
    return ReloadConfigResponse(
        success=data.get("success", False),
        message=data.get("message", ""),
        warnings=data.get("warnings", []),
    )


def _parse_backup_entry(data: dict) -> BackupEntry:
    return BackupEntry(
        filename=data.get("filename", ""),
        created_at=data.get("created_at", ""),
        size_bytes=data.get("size_bytes", 0),
        checksum=data.get("checksum", ""),
    )


def _parse_backup_list_response(data: dict) -> BackupListResponse:
    return BackupListResponse(
        backups=[_parse_backup_entry(b) for b in data.get("backups", [])]
    )


def _parse_backup_trigger_response(data: dict) -> BackupTriggerResponse:
    return BackupTriggerResponse(
        success=data.get("success", False),
        message=data.get("message", ""),
        job_id=data.get("job_id", ""),
    )


def _parse_migration_record(data: dict) -> MigrationRecord:
    return MigrationRecord(
        version=data.get("version", 0),
        name=data.get("name", ""),
        applied_at=data.get("applied_at", ""),
        checksum=data.get("checksum", ""),
    )


def _parse_migration_status_response(data: dict) -> MigrationStatusResponse:
    return MigrationStatusResponse(
        current_version=data.get("current_version", 0),
        applied_migrations=[
            _parse_migration_record(r) for r in data.get("applied_migrations", [])
        ],
        pending_count=data.get("pending_count", 0),
        is_up_to_date=data.get("is_up_to_date", False),
    )


def _parse_migration_apply_response(data: dict) -> MigrationApplyResponse:
    return MigrationApplyResponse(
        success=data.get("success", False),
        message=data.get("message", ""),
        migrations_applied=data.get("migrations_applied", 0),
        current_version=data.get("current_version", 0),
    )


def _parse_migration_rollback_response(data: dict) -> MigrationRollbackResponse:
    rolled_back = data.get("rolled_back")
    return MigrationRollbackResponse(
        success=data.get("success", False),
        message=data.get("message", ""),
        rolled_back=_parse_migration_record(rolled_back) if rolled_back else None,
        current_version=data.get("current_version", 0),
    )


class Client:
    """HTTP client for the EvergreenShims management API."""

    def __init__(self, endpoint: str, timeout: float = 30.0):
        self.endpoint = endpoint.rstrip("/")
        self.timeout = timeout

    def _request(self, method: str, path: str, body: Optional[dict] = None) -> Any:
        url = self.endpoint + path
        data = json.dumps(body).encode("utf-8") if body is not None else None

        req = Request(url, data=data, method=method)
        req.add_header("Content-Type", "application/json")

        try:
            with urlopen(req, timeout=self.timeout) as resp:
                return json.loads(resp.read().decode("utf-8"))
        except HTTPError as e:
            raise APIError(e.code, e.read().decode("utf-8")) from e
        except URLError as e:
            raise ConnectionError(f"Failed to connect: {e}") from e
        except TimeoutError:
            raise TimeoutError(f"Request to {url} timed out") from None

    def _get(self, path: str) -> Any:
        return self._request("GET", path)

    def _post(self, path: str, body: Optional[dict] = None) -> Any:
        return self._request("POST", path, body)

    def status(self) -> StatusResponse:
        data = self._get("/api/v1/status")
        return _parse_status_response(data)

    def metrics(self) -> MetricsResponse:
        data = self._get("/api/v1/metrics")
        return _parse_metrics_response(data)

    def health_liveness(self) -> dict:
        return self._get("/healthz")

    def health_readiness(self) -> dict:
        return self._get("/readyz")

    def config_reload(self, config_path: str = "") -> ReloadConfigResponse:
        data = self._post("/api/v1/config/reload", {"config_path": config_path})
        return _parse_reload_config_response(data)

    def backup_list(self) -> BackupListResponse:
        data = self._get("/api/v1/backup/list")
        return _parse_backup_list_response(data)

    def backup_trigger(self) -> BackupTriggerResponse:
        data = self._post("/api/v1/backup/trigger")
        return _parse_backup_trigger_response(data)

    def migration_status(self) -> MigrationStatusResponse:
        data = self._get("/api/v1/migration/status")
        return _parse_migration_status_response(data)

    def migration_apply(self) -> MigrationApplyResponse:
        data = self._post("/api/v1/migration/apply")
        return _parse_migration_apply_response(data)

    def migration_rollback(self) -> MigrationRollbackResponse:
        data = self._post("/api/v1/migration/rollback")
        return _parse_migration_rollback_response(data)
