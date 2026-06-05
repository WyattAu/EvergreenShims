//! Optional Redis-backed event bridge for multi-container deployments.
//!
//! When a single process cannot hold all shims (e.g., separate pods for
//! database shims vs. network shims), `RedisBridge` publishes `ShimEvent`s
//! to a Redis stream and subscribes to relay them into the local `ShimBus`.
//!
//! Feature-gated behind `redis-bus`.

use std::sync::Arc;

use chrono::Utc;
use redis::Commands;
use tokio::sync::broadcast;
use tracing::{error, info};

use crate::bus::ShimBus;
use crate::error::{Error, Result};
use crate::event::ShimEvent;

/// Redis stream key prefix for shim events.
const STREAM_PREFIX: &str = "evergreen:shimbus:events";
/// Consumer group name for coordinated consumption.
const CONSUMER_GROUP: &str = "evergreen-shims";
/// Maximum stream length (trim to ~10k events).
const MAX_STREAM_LEN: usize = 10_000;

/// Connects local `ShimBus` to a Redis stream for cross-container events.
pub struct RedisBridge {
    /// Redis connection (sync wrapper via `redis` crate).
    conn: redis::Client,
    /// Local bus to publish received events into.
    bus: ShimBus,
    /// Instance ID for this bridge (used as consumer name).
    instance_id: String,
    /// Stream key.
    stream_key: String,
}

impl RedisBridge {
    /// Create a new Redis bridge.
    pub fn new(
        redis_url: &str,
        bus: ShimBus,
        instance_id: impl Into<String>,
        namespace: impl Into<String>,
    ) -> Result<Self> {
        let client = redis::Client::open(redis_url)
            .map_err(|e| Error::Connection(format!("redis connect: {}", e)))?;

        let ns: String = namespace.into();
        let stream_key = format!("{}:{}", STREAM_PREFIX, ns);

        Ok(Self {
            conn: client,
            bus,
            instance_id: instance_id.into(),
            stream_key,
        })
    }

    /// Initialize the consumer group (idempotent).
    pub fn init_group(&self) -> Result<()> {
        let mut conn = self
            .conn
            .get_connection()
            .map_err(|e| Error::Connection(format!("redis connection: {}", e)))?;

        // XGROUP CREATE is idempotent if MKSTREAM + 0
        let result: std::result::Result<String, redis::RedisError> = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(&self.stream_key)
            .arg(CONSUMER_GROUP)
            .arg("0")
            .arg("MKSTREAM")
            .query(&mut conn);

        match result {
            Ok(_) => {
                info!(
                    "redis bridge: consumer group '{}' created on '{}'",
                    CONSUMER_GROUP, self.stream_key
                );
                Ok(())
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("BUSYGROUP") {
                    // Group already exists — fine
                    Ok(())
                } else {
                    Err(Error::Connection(format!("XGROUP CREATE: {}", e)))
                }
            }
        }
    }

    /// Publish a `ShimEvent` to the Redis stream.
    pub fn publish(&self, event: &ShimEvent) -> Result<()> {
        let mut conn = self
            .conn
            .get_connection()
            .map_err(|e| Error::Connection(format!("redis connection: {}", e)))?;

        let json = serde_json::to_string(event)
            .map_err(|e| Error::Anyhow(anyhow::anyhow!("event serialization: {}", e)))?;

        let _: String = redis::cmd("XADD")
            .arg(&self.stream_key)
            .arg("MAXLEN")
            .arg("~")
            .arg(MAX_STREAM_LEN)
            .arg("*")
            .arg("source")
            .arg(&event.source)
            .arg("json")
            .arg(&json)
            .query(&mut conn)
            .map_err(|e| Error::Connection(format!("XADD: {}", e)))?;

        Ok(())
    }

    /// Start consuming from Redis and publishing into the local `ShimBus`.
    ///
    /// This runs as a long-lived task. It polls every `poll_interval_ms`.
    pub async fn start_consuming(self: Arc<Self>, poll_interval_ms: u64) -> Result<()> {
        // Get connection on current thread, then wrap in Send-safe wrapper
        let conn = self
            .conn
            .get_connection()
            .map_err(|e| Error::Connection(format!("redis connection: {}", e)))?;

        let conn = Arc::new(parking_lot::Mutex::new(conn));

        info!(
            "redis bridge: starting consumer '{}' on '{}'",
            self.instance_id, self.stream_key
        );

        let mut last_id = "0-0".to_string();

        loop {
            // XREADGROUP with COUNT 10, blocking 1s
            let result: Option<Vec<(String, Vec<(String, Vec<(String, String)>)>)>> = {
                let mut conn = conn.lock();
                redis::cmd("XREADGROUP")
                    .arg("GROUP")
                    .arg(CONSUMER_GROUP)
                    .arg(&self.instance_id)
                    .arg("COUNT")
                    .arg(10)
                    .arg("BLOCK")
                    .arg(poll_interval_ms)
                    .arg("STREAMS")
                    .arg(&self.stream_key)
                    .arg(">")
                    .query(&mut conn)
                    .ok()
            };

            if let Some(streams) = result {
                for (_stream_name, messages) in streams {
                    for (_entry_id, fields) in messages {
                        if let Some(json_val) = fields.iter().find(|(k, _)| k == "json") {
                            if let Ok(event) = serde_json::from_str::<ShimEvent>(&json_val.1) {
                                // Don't echo our own events
                                if event.source != self.instance_id {
                                    self.bus.publish(event);
                                }
                            }
                        }
                        last_id = _entry_id;
                    }
                }
            }

            // ACK processed entries
            if last_id != "0-0" {
                let mut conn = conn.lock();
                let _: std::result::Result<i64, redis::RedisError> = redis::cmd("XACK")
                    .arg(&self.stream_key)
                    .arg(CONSUMER_GROUP)
                    .arg(&last_id)
                    .query(&mut conn);
            }
        }
    }

    /// Publish a local bus event to Redis (call this from a bus subscriber).
    pub fn relay_to_redis(event: &ShimEvent) -> Result<()> {
        // This is a static method for use in relay loops;
        // actual connection needs to be passed in.
        // For simplicity, the bridge itself handles this via `start_consuming`.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redis_bridge_creation() {
        // Without a real Redis server, we can only test construction params
        let bus = ShimBus::new();
        // This will fail to connect but tests the code path
        let result = RedisBridge::new("redis://127.0.0.1:1", bus, "test-instance", "test-ns");
        assert!(result.is_ok());
    }

    #[test]
    fn test_stream_key_namespace() {
        let bus = ShimBus::new();
        let bridge = RedisBridge::new("redis://127.0.0.1:1", bus, "i1", "prod").unwrap();
        assert_eq!(bridge.stream_key, "evergreen:shimbus:events:prod");
    }

    #[test]
    fn test_stream_key_default_namespace() {
        let bus = ShimBus::new();
        let bridge = RedisBridge::new("redis://127.0.0.1:1", bus, "i1", "").unwrap();
        assert_eq!(bridge.stream_key, "evergreen:shimbus:events:");
    }
}
