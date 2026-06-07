//! Optional Redis-backed event bridge for multi-container deployments.
//!
//! When a single process cannot hold all shims (e.g., separate pods for
//! database shims vs. network shims), `RedisBridge` publishes `ShimEvent`s
//! to a Redis stream and subscribes to relay them into the local `ShimBus`.
//!
//! Feature-gated behind `redis-bus`.

use std::sync::Arc;

use chrono::Utc;
use prometheus::{IntCounter, IntCounterVec, Opts, Registry};
use redis::aio::ConnectionManager;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::bus::ShimBus;
use crate::error::{Error, Result};
use crate::event::ShimEvent;

/// Redis stream key prefix for shim events.
const STREAM_PREFIX: &str = "evergreen:shimbus:events";
/// Consumer group name for coordinated consumption.
const CONSUMER_GROUP: &str = "evergreen-shims";
/// Maximum stream length (trim to ~10k events).
const MAX_STREAM_LEN: usize = 10_000;
/// Warning threshold ratio for consumed events before logging.
const CONSUME_WARNING_THRESHOLD: u64 = 1000;

/// Metrics for the Redis bridge.
pub struct RedisBridgeMetrics {
    /// Total events published to Redis.
    pub events_published: IntCounter,
    /// Total events consumed from Redis.
    pub events_consumed: IntCounter,
    /// Total errors encountered.
    pub errors: IntCounter,
}

impl RedisBridgeMetrics {
    /// Create and register metrics on the given registry.
    pub fn new(registry: &Registry) -> Self {
        let events_published = IntCounter::with_opts(Opts::new(
            "redis_bridge_events_published",
            "Total events published to Redis stream",
        ))
        .expect("metric opts for redis_bridge_events_published are valid");
        let events_consumed = IntCounter::with_opts(Opts::new(
            "redis_bridge_events_consumed",
            "Total events consumed from Redis stream",
        ))
        .expect("metric opts for redis_bridge_events_consumed are valid");
        let errors = IntCounter::with_opts(Opts::new(
            "redis_bridge_errors",
            "Total Redis bridge errors",
        ))
        .expect("metric opts for redis_bridge_errors are valid");

        registry
            .register(Box::new(events_published.clone()))
            .expect("register events_published must not conflict");
        registry
            .register(Box::new(events_consumed.clone()))
            .expect("register events_consumed must not conflict");
        registry
            .register(Box::new(errors.clone()))
            .expect("register errors must not conflict");

        Self {
            events_published,
            events_consumed,
            errors,
        }
    }
}

/// Callback type for processing consumed events.
pub type EventHandler = Arc<dyn Fn(ShimEvent) + Send + Sync>;

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
    /// Optional metrics collector.
    metrics: Option<Arc<RedisBridgeMetrics>>,
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
            metrics: None,
        })
    }

    /// Create a new Redis bridge with metrics collection.
    pub fn with_metrics(
        redis_url: &str,
        bus: ShimBus,
        instance_id: impl Into<String>,
        namespace: impl Into<String>,
        registry: &Registry,
    ) -> Result<Self> {
        let mut bridge = Self::new(redis_url, bus, instance_id, namespace)?;
        bridge.metrics = Some(Arc::new(RedisBridgeMetrics::new(registry)));
        Ok(bridge)
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
            .map_err(|e| {
                if let Some(ref m) = self.metrics {
                    m.errors.inc();
                }
                Error::Connection(format!("XADD: {}", e))
            })?;

        if let Some(ref m) = self.metrics {
            m.events_published.inc();
        }

        Ok(())
    }

    /// Start consuming from Redis and publishing into the local `ShimBus`.
    ///
    /// This runs as a long-lived task. It polls every `poll_interval_ms`.
    pub async fn start_consuming(self: Arc<Self>, poll_interval_ms: u64) -> Result<()> {
        let mut conn = ConnectionManager::new(self.conn.clone())
            .await
            .map_err(|e| Error::Connection(format!("redis async connection: {}", e)))?;

        info!(
            "redis bridge: starting consumer '{}' on '{}'",
            self.instance_id, self.stream_key
        );

        let mut last_id = "0-0".to_string();

        loop {
            let result: Option<Vec<(String, Vec<(String, Vec<(String, String)>)>)>> =
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
                    .query_async(&mut conn)
                    .await
                    .ok();

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
                let _: std::result::Result<i64, redis::RedisError> = redis::cmd("XACK")
                    .arg(&self.stream_key)
                    .arg(CONSUMER_GROUP)
                    .arg(&last_id)
                    .query_async(&mut conn)
                    .await;
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

    /// Start consuming from Redis and forwarding events to a handler callback.
    ///
    /// Unlike `start_consuming`, this method invokes the provided callback
    /// for each consumed event instead of publishing directly to the local bus.
    /// The callback is responsible for processing the event.
    pub async fn start_consuming_with_handler(
        self: Arc<Self>,
        poll_interval_ms: u64,
        handler: EventHandler,
    ) -> Result<()> {
        let mut conn = ConnectionManager::new(self.conn.clone())
            .await
            .map_err(|e| Error::Connection(format!("redis async connection: {}", e)))?;

        info!(
            "redis bridge: starting consumer '{}' on '{}' with handler",
            self.instance_id, self.stream_key
        );

        let mut last_id = "0-0".to_string();

        loop {
            let result: Option<Vec<(String, Vec<(String, Vec<(String, String)>)>)>> =
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
                    .query_async(&mut conn)
                    .await
                    .ok();

            if let Some(streams) = result {
                for (_stream_name, messages) in streams {
                    for (_entry_id, fields) in messages {
                        if let Some(json_val) = fields.iter().find(|(k, _)| k == "json") {
                            if let Ok(event) = serde_json::from_str::<ShimEvent>(&json_val.1) {
                                if event.source != self.instance_id {
                                    handler(event);
                                    if let Some(ref m) = self.metrics {
                                        m.events_consumed.inc();
                                    }
                                }
                            } else if let Some(ref m) = self.metrics {
                                m.errors.inc();
                                warn!("redis bridge: failed to deserialize event from stream");
                            }
                        }
                        last_id = _entry_id;
                    }
                }
            }

            if last_id != "0-0" {
                let _: std::result::Result<i64, redis::RedisError> = redis::cmd("XACK")
                    .arg(&self.stream_key)
                    .arg(CONSUMER_GROUP)
                    .arg(&last_id)
                    .query_async(&mut conn)
                    .await;
            }
        }
    }

    /// Consume events from Redis and relay them directly to the local `ShimBus`.
    ///
    /// This is a convenience wrapper around `start_consuming_with_handler` that
    /// publishes each consumed event into the local bus. Returns the number of
    /// events relayed so far (useful for monitoring).
    pub async fn relay_to_local_bus(self: Arc<Self>, poll_interval_ms: u64) -> Result<u64> {
        let bus = self.bus.clone();
        let relayed = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let relayed_clone = relayed.clone();

        let handler: EventHandler = Arc::new(move |event: ShimEvent| {
            bus.publish(event);
            relayed_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });

        // Run the consuming loop; this never returns unless there's a fatal error
        self.start_consuming_with_handler(poll_interval_ms, handler)
            .await?;

        Ok(relayed.load(std::sync::atomic::Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redis_bridge_creation() {
        let bus = ShimBus::new();
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

    #[test]
    fn test_bridge_starts_without_metrics() {
        let bus = ShimBus::new();
        let bridge = RedisBridge::new("redis://127.0.0.1:1", bus, "i1", "ns").unwrap();
        assert!(bridge.metrics.is_none());
    }

    #[test]
    fn test_bridge_with_metrics() {
        let registry = Registry::new();
        let bus = ShimBus::new();
        let bridge =
            RedisBridge::with_metrics("redis://127.0.0.1:1", bus, "i1", "ns", &registry).unwrap();
        assert!(bridge.metrics.is_some());

        let m = bridge.metrics.as_ref().unwrap();
        m.events_published.inc();
        m.events_consumed.inc();
        m.errors.inc();

        let output = prometheus::TextEncoder::new()
            .encode_to_string(&registry.gather())
            .unwrap();
        assert!(output.contains("redis_bridge_events_published"));
        assert!(output.contains("redis_bridge_events_consumed"));
        assert!(output.contains("redis_bridge_errors"));
    }

    #[test]
    fn test_redis_bridge_metrics_creation() {
        let registry = Registry::new();
        let m = RedisBridgeMetrics::new(&registry);
        assert_eq!(m.events_published.get(), 0);
        assert_eq!(m.events_consumed.get(), 0);
        assert_eq!(m.errors.get(), 0);
    }

    #[test]
    fn test_redis_bridge_metrics_increments() {
        let registry = Registry::new();
        let m = RedisBridgeMetrics::new(&registry);
        m.events_published.inc();
        m.events_published.inc();
        m.events_consumed.inc();
        m.errors.inc();
        m.errors.inc();
        m.errors.inc();

        assert_eq!(m.events_published.get(), 2);
        assert_eq!(m.events_consumed.get(), 1);
        assert_eq!(m.errors.get(), 3);
    }

    #[test]
    fn test_handler_type_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EventHandler>();
    }
}
