//! Integration tests for EvergreenShims.
//!
//! These tests verify shim behavior with real databases.
//! Run with: cargo test -p evergreen-shims-integration
//!
//! Prerequisites:
//!   docker compose -f tests/docker-compose.yml up -d

mod backup;
mod failover;
mod vault;
