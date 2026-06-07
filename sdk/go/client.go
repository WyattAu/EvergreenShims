package evergreenshim

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"
)

type Client struct {
	endpoint   string
	httpClient *http.Client
}

type ConfigOption func(*Client)

func WithHTTPClient(c *http.Client) ConfigOption {
	return func(client *Client) {
		client.httpClient = c
	}
}

func WithTimeout(d time.Duration) ConfigOption {
	return func(client *Client) {
		client.httpClient.Timeout = d
	}
}

func NewClient(endpoint string, opts ...ConfigOption) *Client {
	c := &Client{
		endpoint: endpoint,
		httpClient: &http.Client{
			Timeout: 30 * time.Second,
		},
	}
	for _, opt := range opts {
		opt(c)
	}
	return c
}

func (c *Client) get(path string, out interface{}) error {
	resp, err := c.httpClient.Get(c.endpoint + path)
	if err != nil {
		return fmt.Errorf("GET %s: %w", path, err)
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 400 {
		body, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("GET %s: status %d: %s", path, resp.StatusCode, string(body))
	}

	return json.NewDecoder(resp.Body).Decode(out)
}

func (c *Client) post(path string, body interface{}, out interface{}) error {
	data, err := json.Marshal(body)
	if err != nil {
		return fmt.Errorf("marshal request: %w", err)
	}

	resp, err := c.httpClient.Post(
		c.endpoint+path,
		"application/json",
		bytes.NewReader(data),
	)
	if err != nil {
		return fmt.Errorf("POST %s: %w", path, err)
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 400 {
		respBody, _ := io.ReadAll(resp.Body)
		return fmt.Errorf("POST %s: status %d: %s", path, resp.StatusCode, string(respBody))
	}

	if out != nil {
		return json.NewDecoder(resp.Body).Decode(out)
	}
	return nil
}

func (c *Client) Status() (*StatusResponse, error) {
	var result StatusResponse
	if err := c.get("/api/v1/status", &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (c *Client) Metrics() (*MetricsResponse, error) {
	var result MetricsResponse
	if err := c.get("/api/v1/metrics", &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (c *Client) HealthLiveness() (map[string]interface{}, error) {
	var result map[string]interface{}
	if err := c.get("/healthz", &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (c *Client) HealthReadiness() (map[string]interface{}, error) {
	var result map[string]interface{}
	if err := c.get("/readyz", &result); err != nil {
		return nil, err
	}
	return result, nil
}

func (c *Client) ConfigReload(configPath string) (*ReloadConfigResponse, error) {
	body := map[string]string{"config_path": configPath}
	var result ReloadConfigResponse
	if err := c.post("/api/v1/config/reload", body, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (c *Client) BackupList() (*BackupListResponse, error) {
	var result BackupListResponse
	if err := c.get("/api/v1/backup/list", &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (c *Client) BackupTrigger() (*BackupTriggerResponse, error) {
	var result BackupTriggerResponse
	if err := c.post("/api/v1/backup/trigger", nil, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (c *Client) MigrationStatus() (*MigrationStatusResponse, error) {
	var result MigrationStatusResponse
	if err := c.get("/api/v1/migration/status", &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (c *Client) MigrationApply() (*MigrationApplyResponse, error) {
	var result MigrationApplyResponse
	if err := c.post("/api/v1/migration/apply", nil, &result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (c *Client) MigrationRollback() (*MigrationRollbackResponse, error) {
	var result MigrationRollbackResponse
	if err := c.post("/api/v1/migration/rollback", nil, &result); err != nil {
		return nil, err
	}
	return &result, nil
}
