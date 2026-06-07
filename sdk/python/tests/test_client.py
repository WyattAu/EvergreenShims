"""Tests for the EvergreenShims Python SDK client."""
import json
import threading
from http.server import HTTPServer, BaseHTTPRequestHandler
from urllib.error import HTTPError

import pytest

from evergreen_shims.client import Client
from evergreen_shims.exceptions import APIError


class MockHandler(BaseHTTPRequestHandler):
    """Mock HTTP handler for testing."""

    responses = {}
    request_log = []

    def do_GET(self):
        MockHandler.request_log.append(("GET", self.path))
        if self.path in MockHandler.responses:
            body = MockHandler.responses[self.path]
            self.send_response(body.get("status", 200))
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(body.get("data", {})).encode())
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        MockHandler.request_log.append(("POST", self.path))
        content_length = int(self.headers.get("Content-Length", 0))
        if content_length > 0:
            self.rfile.read(content_length)
        if self.path in MockHandler.responses:
            body = MockHandler.responses[self.path]
            self.send_response(body.get("status", 200))
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(body.get("data", {})).encode())
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, format, *args):
        pass  # Suppress server logs during tests


@pytest.fixture(autouse=True)
def setup_teardown():
    """Reset mock state before each test."""
    MockHandler.responses = {}
    MockHandler.request_log = []
    yield


@pytest.fixture
def server():
    """Start a mock HTTP server."""
    httpd = HTTPServer(("127.0.0.1", 0), MockHandler)
    port = httpd.server_address[1]
    thread = threading.Thread(target=httpd.serve_forever, daemon=True)
    thread.start()
    yield f"http://127.0.0.1:{port}"
    httpd.shutdown()


@pytest.fixture
def client(server):
    """Create a client connected to the mock server."""
    return Client(server, timeout=5.0)


class TestClientCreation:
    def test_create_client(self, server):
        c = Client(server)
        assert c.endpoint == server
        assert c.timeout == 30.0

    def test_create_client_custom_timeout(self, server):
        c = Client(server, timeout=10.0)
        assert c.timeout == 10.0

    def test_strips_trailing_slash(self, server):
        c = Client(server + "/")
        assert c.endpoint == server


class TestStatus:
    def test_status_success(self, client):
        MockHandler.responses["/api/v1/status"] = {
            "data": {
                "health": 1,
                "version": "1.0.0",
                "uptime_seconds": "12345",
                "capabilities": [
                    {"name": "health", "enabled": True, "healthy": True, "last_error": ""}
                ],
            }
        }
        resp = client.status()
        assert resp.health.value == 1
        assert resp.version == "1.0.0"
        assert resp.uptime_seconds == "12345"
        assert len(resp.capabilities) == 1
        assert resp.capabilities[0]["name"] == "health"

    def test_status_server_error(self, client):
        MockHandler.responses["/api/v1/status"] = {"status": 500, "data": {}}
        with pytest.raises(APIError) as exc_info:
            client.status()
        assert exc_info.value.status_code == 500


class TestMetrics:
    def test_metrics_success(self, client):
        MockHandler.responses["/api/v1/metrics"] = {
            "data": {
                "metrics": [
                    {"name": "shim_uptime_seconds", "value": 100.0, "labels": {}, "type": 1}
                ]
            }
        }
        resp = client.metrics()
        assert len(resp.metrics) == 1
        assert resp.metrics[0].name == "shim_uptime_seconds"
        assert resp.metrics[0].value == 100.0


class TestConfigReload:
    def test_config_reload_success(self, client):
        MockHandler.responses["/api/v1/config/reload"] = {
            "data": {"success": True, "message": "reloaded", "warnings": []}
        }
        resp = client.config_reload("/etc/config.yaml")
        assert resp.success is True
        assert resp.message == "reloaded"

    def test_config_reload_sends_post(self, client):
        MockHandler.responses["/api/v1/config/reload"] = {
            "data": {"success": True, "message": "ok", "warnings": []}
        }
        client.config_reload("/etc/config.yaml")
        assert ("POST", "/api/v1/config/reload") in MockHandler.request_log


class TestBackup:
    def test_backup_list(self, client):
        MockHandler.responses["/api/v1/backup/list"] = {
            "data": {
                "backups": [
                    {"filename": "db_20240101.sql.gz", "size_bytes": 1024, "created_at": "2024-01-01", "checksum": "abc"}
                ]
            }
        }
        resp = client.backup_list()
        assert len(resp.backups) == 1
        assert resp.backups[0].filename == "db_20240101.sql.gz"
        assert resp.backups[0].size_bytes == 1024

    def test_backup_trigger(self, client):
        MockHandler.responses["/api/v1/backup/trigger"] = {
            "data": {"success": True, "message": "started", "job_id": "job-123"}
        }
        resp = client.backup_trigger()
        assert resp.success is True
        assert resp.job_id == "job-123"


class TestMigration:
    def test_migration_status(self, client):
        MockHandler.responses["/api/v1/migration/status"] = {
            "data": {
                "current_version": 3,
                "applied_migrations": [],
                "pending_count": 0,
                "is_up_to_date": True,
            }
        }
        resp = client.migration_status()
        assert resp.current_version == 3
        assert resp.is_up_to_date is True

    def test_migration_apply(self, client):
        MockHandler.responses["/api/v1/migration/apply"] = {
            "data": {
                "success": True,
                "message": "applied",
                "migrations_applied": 2,
                "current_version": 5,
            }
        }
        resp = client.migration_apply()
        assert resp.success is True
        assert resp.migrations_applied == 2

    def test_migration_rollback(self, client):
        MockHandler.responses["/api/v1/migration/rollback"] = {
            "data": {
                "success": True,
                "message": "rolled back",
                "current_version": 2,
                "rolled_back": {"version": 3, "name": "add_email", "applied_at": "", "checksum": ""},
            }
        }
        resp = client.migration_rollback()
        assert resp.success is True
        assert resp.rolled_back is not None
        assert resp.rolled_back.version == 3


class TestHealth:
    def test_health_liveness(self, client):
        MockHandler.responses["/healthz"] = {"data": {"status": "alive"}}
        resp = client.health_liveness()
        assert resp["status"] == "alive"

    def test_health_readiness(self, client):
        MockHandler.responses["/readyz"] = {"data": {"status": "ready"}}
        resp = client.health_readiness()
        assert resp["status"] == "ready"


class TestErrorHandling:
    def test_connection_error(self):
        c = Client("http://127.0.0.1:1", timeout=1.0)
        from evergreen_shims.exceptions import ConnectionError as ShimConnectionError
        with pytest.raises((ShimConnectionError, Exception)):
            c.status()
