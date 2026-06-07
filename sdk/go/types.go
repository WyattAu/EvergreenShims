package evergreenshim

type HealthStatus int

const (
	HealthStatusUnspecified HealthStatus = iota
	HealthStatusHealthy
	HealthStatusDegraded
	HealthStatusUnhealthy
)

func (h HealthStatus) String() string {
	switch h {
	case HealthStatusHealthy:
		return "healthy"
	case HealthStatusDegraded:
		return "degraded"
	case HealthStatusUnhealthy:
		return "unhealthy"
	default:
		return "unspecified"
	}
}

type MetricType int

const (
	MetricTypeUnspecified MetricType = iota
	MetricTypeCounter
	MetricTypeGauge
	MetricTypeHistogram
	MetricTypeSummary
)

type CapabilityStatus struct {
	Name      string `json:"name"`
	Enabled   bool   `json:"enabled"`
	Healthy   bool   `json:"healthy"`
	LastError string `json:"last_error"`
}

type CapabilityInfo struct {
	Name         string            `json:"name"`
	Enabled      bool              `json:"enabled"`
	Version      string            `json:"version"`
	Dependencies []string          `json:"dependencies"`
	Metadata     map[string]string `json:"metadata"`
}

type Metric struct {
	Name   string            `json:"name"`
	Value  float64           `json:"value"`
	Labels map[string]string `json:"labels"`
	Type   MetricType        `json:"type"`
}

type StatusResponse struct {
	Health         HealthStatus       `json:"health"`
	Version        string             `json:"version"`
	UptimeSeconds  string             `json:"uptime_seconds"`
	Capabilities   []CapabilityStatus `json:"capabilities"`
}

type MetricsResponse struct {
	Metrics []Metric `json:"metrics"`
}

type ReloadConfigResponse struct {
	Success  bool     `json:"success"`
	Message  string   `json:"message"`
	Warnings []string `json:"warnings"`
}

type CapabilitiesResponse struct {
	Capabilities []CapabilityInfo `json:"capabilities"`
}

type BackupEntry struct {
	Filename  string `json:"filename"`
	CreatedAt string `json:"created_at"`
	SizeBytes uint64 `json:"size_bytes"`
	Checksum  string `json:"checksum"`
}

type BackupListResponse struct {
	Backups []BackupEntry `json:"backups"`
}

type BackupTriggerResponse struct {
	Success bool   `json:"success"`
	Message string `json:"message"`
	JobID   string `json:"job_id"`
}

type MigrationRecord struct {
	Version   uint32 `json:"version"`
	Name      string `json:"name"`
	AppliedAt string `json:"applied_at"`
	Checksum  string `json:"checksum"`
}

type MigrationStatusResponse struct {
	CurrentVersion    uint32            `json:"current_version"`
	AppliedMigrations []MigrationRecord `json:"applied_migrations"`
	PendingCount      uint32            `json:"pending_count"`
	IsUpToDate        bool              `json:"is_up_to_date"`
}

type MigrationApplyResponse struct {
	Success          bool   `json:"success"`
	Message          string `json:"message"`
	MigrationsApplied uint32 `json:"migrations_applied"`
	CurrentVersion   uint32 `json:"current_version"`
}

type MigrationRollbackResponse struct {
	Success         bool             `json:"success"`
	Message         string           `json:"message"`
	RolledBack      MigrationRecord  `json:"rolled_back"`
	CurrentVersion  uint32           `json:"current_version"`
}
