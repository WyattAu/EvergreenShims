import * as http from "http";
import * as https from "https";
import { URL } from "url";
import {
  StatusResponse,
  MetricsResponse,
  ReloadConfigResponse,
  BackupListResponse,
  BackupTriggerResponse,
  MigrationStatusResponse,
  MigrationApplyResponse,
  MigrationRollbackResponse,
} from "./types";
import { APIError, ConnectionError, TimeoutError } from "./exceptions";

export interface ClientOptions {
  timeout?: number;
}

export class Client {
  private endpoint: string;
  private timeout: number;

  constructor(endpoint: string, options: ClientOptions = {}) {
    this.endpoint = endpoint.replace(/\/+$/, "");
    this.timeout = options.timeout ?? 30_000;
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown
  ): Promise<T> {
    const url = new URL(path, this.endpoint);
    const isHttps = url.protocol === "https:";
    const transport = isHttps ? https : http;

    const headers: Record<string, string> = {
      "Content-Type": "application/json",
    };

    const bodyStr = body !== undefined ? JSON.stringify(body) : undefined;

    return new Promise<T>((resolve, reject) => {
      const req = transport.request(
        url.href,
        {
          method,
          headers,
          timeout: this.timeout,
        },
        (res) => {
          let data = "";
          res.on("data", (chunk: Buffer) => {
            data += chunk.toString();
          });
          res.on("end", () => {
            if (res.statusCode && res.statusCode >= 400) {
              reject(new APIError(res.statusCode, data));
              return;
            }
            try {
              resolve(JSON.parse(data) as T);
            } catch {
              reject(new EvergreenShimError("Failed to parse response"));
            }
          });
        }
      );

      req.on("error", (err: Error) => {
        if (
          err.message.includes("timeout") ||
          err.message.includes("ETIMEDOUT")
        ) {
          reject(
            new TimeoutError(`Request to ${url.href} timed out`)
          );
        } else {
          reject(new ConnectionError(`Connection failed: ${err.message}`));
        }
      });

      req.on("timeout", () => {
        req.destroy();
        reject(new TimeoutError(`Request to ${url.href} timed out`));
      });

      if (bodyStr) {
        req.write(bodyStr);
      }
      req.end();
    });
  }

  private get<T>(path: string): Promise<T> {
    return this.request<T>("GET", path);
  }

  private post<T>(path: string, body?: unknown): Promise<T> {
    return this.request<T>("POST", path, body);
  }

  status(): Promise<StatusResponse> {
    return this.get<StatusResponse>("/api/v1/status");
  }

  metrics(): Promise<MetricsResponse> {
    return this.get<MetricsResponse>("/api/v1/metrics");
  }

  healthLiveness(): Promise<Record<string, unknown>> {
    return this.get<Record<string, unknown>>("/healthz");
  }

  healthReadiness(): Promise<Record<string, unknown>> {
    return this.get<Record<string, unknown>>("/readyz");
  }

  configReload(configPath: string = ""): Promise<ReloadConfigResponse> {
    return this.post<ReloadConfigResponse>("/api/v1/config/reload", {
      config_path: configPath,
    });
  }

  backupList(): Promise<BackupListResponse> {
    return this.get<BackupListResponse>("/api/v1/backup/list");
  }

  backupTrigger(): Promise<BackupTriggerResponse> {
    return this.post<BackupTriggerResponse>("/api/v1/backup/trigger");
  }

  migrationStatus(): Promise<MigrationStatusResponse> {
    return this.get<MigrationStatusResponse>("/api/v1/migration/status");
  }

  migrationApply(): Promise<MigrationApplyResponse> {
    return this.post<MigrationApplyResponse>("/api/v1/migration/apply");
  }

  migrationRollback(): Promise<MigrationRollbackResponse> {
    return this.post<MigrationRollbackResponse>("/api/v1/migration/rollback");
  }
}
