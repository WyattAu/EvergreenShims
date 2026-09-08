# loop-retry

Generic async retry with exponential backoff and jitter.

## Usage

```rust
use retry_backoff::{with_backoff, RetryConfig};

async fn fetch_data() -> Result<String, std::io::Error> {
    let config = RetryConfig::default();
    with_backoff(&config, || async {
        // Your fallible async operation here
        Ok("data".to_string())
    }).await.map_err(|e| e.into_inner())
}
```

## Configuration

```rust
use retry_backoff::RetryConfig;
use std::time::Duration;

let config = RetryConfig {
    max_retries: 5,
    initial_delay: Duration::from_millis(200),
    max_delay: Duration::from_secs(10),
    backoff_multiplier: 2.0,
    jitter: true,
};
```

## Error Classification

Implement `IsRetryable` on your error type to control which errors trigger retries:

```rust
use retry_backoff::IsRetryable;

#[derive(Debug)]
enum MyError {
    Transient,
    Permanent,
}

impl std::fmt::Display for MyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MyError::Transient => write!(f, "transient error"),
            MyError::Permanent => write!(f, "permanent error"),
        }
    }
}

impl IsRetryable for MyError {
    fn is_retryable(&self) -> bool {
        matches!(self, MyError::Transient)
    }
}
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.
