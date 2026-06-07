import * as http from "http";
import { describe, it, before, after } from "node:test";
import assert from "node:assert/strict";
import { Client } from "./client";
import { HealthStatus, MetricType } from "./types";

function createMockServer(
  handler: (req: http.IncomingMessage, res: http.ServerResponse) => void
): Promise<{ server: http.Server; port: number }> {
  return new Promise((resolve) => {
    const server = http.createServer(handler);
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address() as { port: number };
      resolve({ server, port: addr.port });
    });
  });
}

function jsonResponse(res: http.ServerResponse, data: unknown, status = 200) {
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(data));
}

describe("Client", () => {
  describe("construction", () => {
    it("creates client with endpoint", () => {
      const c = new Client("http://localhost:50051");
      assert.equal((c as any).endpoint, "http://localhost:50051");
    });

    it("strips trailing slashes", () => {
      const c = new Client("http://localhost:50051///");
      assert.equal((c as any).endpoint, "http://localhost:50051");
    });

    it("uses default timeout of 30s", () => {
      const c = new Client("http://localhost:50051");
      assert.equal((c as any).timeout, 30_000);
    });

    it("accepts custom timeout", () => {
      const c = new Client("http://localhost:50051", { timeout: 5000 });
      assert.equal((c as any).timeout, 5000);
    });
  });

  describe("status", () => {
    let server: http.Server;
    let port: number;

    before(async () => {
      const mock = await createMockServer((req, res) => {
        if (req.url === "/api/v1/status") {
          jsonResponse(res, {
            health: HealthStatus.Healthy,
            version: "1.0.0",
            uptime_seconds: "12345",
            capabilities: [
              { name: "health", enabled: true, healthy: true, last_error: "" },
            ],
          });
        } else {
          res.writeHead(404).end();
        }
      });
      server = mock.server;
      port = mock.port;
    });

    after(() => server.close());

    it("fetches status successfully", async () => {
      const client = new Client(`http://127.0.0.1:${port}`);
      const resp = await client.status();
      assert.equal(resp.health, HealthStatus.Healthy);
      assert.equal(resp.version, "1.0.0");
      assert.equal(resp.uptime_seconds, "12345");
      assert.equal(resp.capabilities.length, 1);
      assert.equal(resp.capabilities[0].name, "health");
    });
  });

  describe("metrics", () => {
    let server: http.Server;
    let port: number;

    before(async () => {
      const mock = await createMockServer((req, res) => {
        if (req.url === "/api/v1/metrics") {
          jsonResponse(res, {
            metrics: [
              { name: "shim_uptime_seconds", value: 100.0, labels: {}, type: MetricType.Gauge },
            ],
          });
        } else {
          res.writeHead(404).end();
        }
      });
      server = mock.server;
      port = mock.port;
    });

    after(() => server.close());

    it("fetches metrics successfully", async () => {
      const client = new Client(`http://127.0.0.1:${port}`);
      const resp = await client.metrics();
      assert.equal(resp.metrics.length, 1);
      assert.equal(resp.metrics[0].name, "shim_uptime_seconds");
      assert.equal(resp.metrics[0].value, 100.0);
    });
  });

  describe("config reload", () => {
    let server: http.Server;
    let port: number;
    let lastMethod: string | undefined;

    before(async () => {
      const mock = await createMockServer((req, res) => {
        lastMethod = req.method;
        if (req.url === "/api/v1/config/reload") {
          jsonResponse(res, { success: true, message: "reloaded", warnings: [] });
        } else {
          res.writeHead(404).end();
        }
      });
      server = mock.server;
      port = mock.port;
    });

    after(() => server.close());

    it("sends POST request", async () => {
      const client = new Client(`http://127.0.0.1:${port}`);
      const resp = await client.configReload("/etc/config.yaml");
      assert.equal(resp.success, true);
      assert.equal(lastMethod, "POST");
    });
  });

  describe("backup", () => {
    let server: http.Server;
    let port: number;

    before(async () => {
      const mock = await createMockServer((req, res) => {
        if (req.url === "/api/v1/backup/list") {
          jsonResponse(res, {
            backups: [
              { filename: "db_20240101.sql.gz", size_bytes: 1024, created_at: "2024-01-01", checksum: "abc" },
            ],
          });
        } else if (req.url === "/api/v1/backup/trigger") {
          jsonResponse(res, { success: true, message: "started", job_id: "job-123" });
        } else {
          res.writeHead(404).end();
        }
      });
      server = mock.server;
      port = mock.port;
    });

    after(() => server.close());

    it("lists backups", async () => {
      const client = new Client(`http://127.0.0.1:${port}`);
      const resp = await client.backupList();
      assert.equal(resp.backups.length, 1);
      assert.equal(resp.backups[0].filename, "db_20240101.sql.gz");
    });

    it("triggers backup", async () => {
      const client = new Client(`http://127.0.0.1:${port}`);
      const resp = await client.backupTrigger();
      assert.equal(resp.success, true);
      assert.equal(resp.job_id, "job-123");
    });
  });

  describe("migration", () => {
    let server: http.Server;
    let port: number;

    before(async () => {
      const mock = await createMockServer((req, res) => {
        if (req.url === "/api/v1/migration/status") {
          jsonResponse(res, {
            current_version: 3,
            applied_migrations: [],
            pending_count: 0,
            is_up_to_date: true,
          });
        } else if (req.url === "/api/v1/migration/apply") {
          jsonResponse(res, {
            success: true,
            message: "applied",
            migrations_applied: 2,
            current_version: 5,
          });
        } else if (req.url === "/api/v1/migration/rollback") {
          jsonResponse(res, {
            success: true,
            message: "rolled back",
            current_version: 2,
            rolled_back: { version: 3, name: "add_email", applied_at: "", checksum: "" },
          });
        } else {
          res.writeHead(404).end();
        }
      });
      server = mock.server;
      port = mock.port;
    });

    after(() => server.close());

    it("fetches migration status", async () => {
      const client = new Client(`http://127.0.0.1:${port}`);
      const resp = await client.migrationStatus();
      assert.equal(resp.current_version, 3);
      assert.equal(resp.is_up_to_date, true);
    });

    it("applies migrations", async () => {
      const client = new Client(`http://127.0.0.1:${port}`);
      const resp = await client.migrationApply();
      assert.equal(resp.success, true);
      assert.equal(resp.migrations_applied, 2);
    });

    it("rolls back migration", async () => {
      const client = new Client(`http://127.0.0.1:${port}`);
      const resp = await client.migrationRollback();
      assert.equal(resp.success, true);
      assert.notEqual(resp.rolled_back, null);
      assert.equal(resp.rolled_back!.version, 3);
    });
  });

  describe("health", () => {
    let server: http.Server;
    let port: number;

    before(async () => {
      const mock = await createMockServer((req, res) => {
        if (req.url === "/healthz") {
          jsonResponse(res, { status: "alive" });
        } else if (req.url === "/readyz") {
          jsonResponse(res, { status: "ready" });
        } else {
          res.writeHead(404).end();
        }
      });
      server = mock.server;
      port = mock.port;
    });

    after(() => server.close());

    it("checks liveness", async () => {
      const client = new Client(`http://127.0.0.1:${port}`);
      const resp = await client.healthLiveness();
      assert.equal(resp.status, "alive");
    });

    it("checks readiness", async () => {
      const client = new Client(`http://127.0.0.1:${port}`);
      const resp = await client.healthReadiness();
      assert.equal(resp.status, "ready");
    });
  });

  describe("error handling", () => {
    let server: http.Server;
    let port: number;

    before(async () => {
      const mock = await createMockServer((req, res) => {
        res.writeHead(500).end("internal error");
      });
      server = mock.server;
      port = mock.port;
    });

    after(() => server.close());

    it("rejects on server error", async () => {
      const client = new Client(`http://127.0.0.1:${port}`);
      await assert.rejects(() => client.status(), (err: any) => {
        assert.equal(err.name, "APIError");
        assert.equal(err.statusCode, 500);
        return true;
      });
    });
  });
});
