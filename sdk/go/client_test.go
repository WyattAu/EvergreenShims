package evergreenshim

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func setupTestServer(handler http.HandlerFunc) (*httptest.Server, *Client) {
	server := httptest.NewServer(handler)
	client := NewClient(server.URL, WithHTTPClient(server.Client()))
	return server, client
}

func TestNewClient(t *testing.T) {
	c := NewClient("http://localhost:50051")
	if c.endpoint != "http://localhost:50051" {
		t.Errorf("expected endpoint http://localhost:50051, got %s", c.endpoint)
	}
}

func TestStatus(t *testing.T) {
	server, client := setupTestServer(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/v1/status" {
			t.Errorf("expected path /api/v1/status, got %s", r.URL.Path)
		}
		json.NewEncoder(w).Encode(StatusResponse{
			Health:        HealthStatusHealthy,
			Version:       "0.1.0",
			UptimeSeconds: "12345",
		})
	})
	defer server.Close()

	resp, err := client.Status()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if resp.Health != HealthStatusHealthy {
		t.Errorf("expected health healthy, got %v", resp.Health)
	}
	if resp.Version != "0.1.0" {
		t.Errorf("expected version 0.1.0, got %s", resp.Version)
	}
}

func TestMetrics(t *testing.T) {
	server, client := setupTestServer(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(MetricsResponse{
			Metrics: []Metric{
				{Name: "shim_uptime_seconds", Value: 100.0, Type: MetricTypeGauge},
			},
		})
	})
	defer server.Close()

	resp, err := client.Metrics()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(resp.Metrics) != 1 {
		t.Fatalf("expected 1 metric, got %d", len(resp.Metrics))
	}
	if resp.Metrics[0].Name != "shim_uptime_seconds" {
		t.Errorf("expected metric name shim_uptime_seconds, got %s", resp.Metrics[0].Name)
	}
}

func TestConfigReload(t *testing.T) {
	server, client := setupTestServer(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Errorf("expected POST, got %s", r.Method)
		}
		json.NewEncoder(w).Encode(ReloadConfigResponse{
			Success: true,
			Message: "ok",
		})
	})
	defer server.Close()

	resp, err := client.ConfigReload("/etc/config.yaml")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !resp.Success {
		t.Error("expected success true")
	}
}

func TestBackupList(t *testing.T) {
	server, client := setupTestServer(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(BackupListResponse{
			Backups: []BackupEntry{
				{Filename: "db_20240101.sql.gz", SizeBytes: 1024},
			},
		})
	})
	defer server.Close()

	resp, err := client.BackupList()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(resp.Backups) != 1 {
		t.Fatalf("expected 1 backup, got %d", len(resp.Backups))
	}
}

func TestBackupTrigger(t *testing.T) {
	server, client := setupTestServer(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(BackupTriggerResponse{
			Success: true,
			Message: "backup started",
		})
	})
	defer server.Close()

	resp, err := client.BackupTrigger()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !resp.Success {
		t.Error("expected success true")
	}
}

func TestMigrationStatus(t *testing.T) {
	server, client := setupTestServer(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(MigrationStatusResponse{
			CurrentVersion: 3,
			PendingCount:   0,
			IsUpToDate:     true,
		})
	})
	defer server.Close()

	resp, err := client.MigrationStatus()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if resp.CurrentVersion != 3 {
		t.Errorf("expected version 3, got %d", resp.CurrentVersion)
	}
	if !resp.IsUpToDate {
		t.Error("expected is_up_to_date true")
	}
}

func TestMigrationApply(t *testing.T) {
	server, client := setupTestServer(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(MigrationApplyResponse{
			Success:          true,
			MigrationsApplied: 2,
			CurrentVersion:   5,
		})
	})
	defer server.Close()

	resp, err := client.MigrationApply()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if resp.MigrationsApplied != 2 {
		t.Errorf("expected 2 migrations applied, got %d", resp.MigrationsApplied)
	}
}

func TestMigrationRollback(t *testing.T) {
	server, client := setupTestServer(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(MigrationRollbackResponse{
			Success:        true,
			CurrentVersion: 2,
			RolledBack: MigrationRecord{
				Version: 3,
				Name:    "add_email",
			},
		})
	})
	defer server.Close()

	resp, err := client.MigrationRollback()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if resp.RolledBack.Version != 3 {
		t.Errorf("expected rolled back version 3, got %d", resp.RolledBack.Version)
	}
}

func TestServerError(t *testing.T) {
	server, client := setupTestServer(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
		w.Write([]byte("internal error"))
	})
	defer server.Close()

	_, err := client.Status()
	if err == nil {
		t.Fatal("expected error for 500 status")
	}
}

func TestHealthLiveness(t *testing.T) {
	server, client := setupTestServer(func(w http.ResponseWriter, r *http.Request) {
		json.NewEncoder(w).Encode(map[string]interface{}{
			"status": "alive",
		})
	})
	defer server.Close()

	resp, err := client.HealthLiveness()
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if resp["status"] != "alive" {
		t.Errorf("expected status alive, got %v", resp["status"])
	}
}
