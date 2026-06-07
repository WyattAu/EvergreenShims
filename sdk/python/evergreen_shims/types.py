from dataclasses import dataclass, field
from enum import IntEnum
from typing import Optional


class HealthStatus(IntEnum):
    UNSPECIFIED = 0
    HEALTHY = 1
    DEGRADED = 2
    UNHEALTHY = 3

    def __str__(self) -> str:
        return self.name.lower()


class MetricType(IntEnum):
    UNSPECIFIED = 0
    COUNTER = 1
    GAUGE = 2
    HISTOGRAM = 3
    SUMMARY = 4


@dataclass
class CapabilityStatus:
    name: str
    enabled: bool
    healthy: bool
    last_error: str = ""


@dataclass
class CapabilityInfo:
    name: str
    enabled: bool
    version: str
    dependencies: list[str] = field(default_factory=list)
    metadata: dict[str, str] = field(default_factory=dict)


@dataclass
class Metric:
    name: str
    value: float
    labels: dict[str, str] = field(default_factory=dict)
    type: MetricType = MetricType.UNSPECIFIED


@dataclass
class StatusResponse:
    health: HealthStatus
    version: str
    uptime_seconds: str
    capabilities: list[CapabilityStatus] = field(default_factory=list)


@dataclass
class MetricsResponse:
    metrics: list[Metric] = field(default_factory=list)


@dataclass
class ReloadConfigResponse:
    success: bool
    message: str
    warnings: list[str] = field(default_factory=list)


@dataclass
class BackupEntry:
    filename: str
    created_at: str
    size_bytes: int
    checksum: str


@dataclass
class BackupListResponse:
    backups: list[BackupEntry] = field(default_factory=list)


@dataclass
class BackupTriggerResponse:
    success: bool
    message: str
    job_id: str = ""


@dataclass
class MigrationRecord:
    version: int
    name: str
    applied_at: str
    checksum: str


@dataclass
class MigrationStatusResponse:
    current_version: int
    applied_migrations: list[MigrationRecord] = field(default_factory=list)
    pending_count: int = 0
    is_up_to_date: bool = False


@dataclass
class MigrationApplyResponse:
    success: bool
    message: str
    migrations_applied: int = 0
    current_version: int = 0


@dataclass
class MigrationRollbackResponse:
    success: bool
    message: str
    rolled_back: Optional[MigrationRecord] = None
    current_version: int = 0
