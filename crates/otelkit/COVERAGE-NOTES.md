# Coverage Notes — otelkit

## Measurement

```
cargo llvm-cov --summary-only --all-features
```

Final: **99.76%** line coverage (1/354 missed). Tests added in this pass:

- `src/lib.rs` — `guard_drop_reports_provider_shutdown_error`: dropping a
  `TelemetryGuard` holding an already-shut-down `SdkTracerProvider` exercises the
  `Drop` error branch (opentelemetry-sdk 0.28 returns `AlreadyShutdown` on the
  second `shutdown()`).
- `tests/init_otlp_errors.rs` — malformed OTLP endpoint maps to
  `OtlpConnection`; a second `init` in one process fails `try_init` and maps to
  `InvalidConfig`.
- `tests/init_json_errors.rs` — same rebind-refusal path for the JSON-format
  branch.
- `tests/init_sentry_errors.rs` — with the `sentry` feature compiled in, a
  config with neither an OTLP endpoint nor a DSN fails the DSN requirement.

Integration tests are kept in separate files on purpose: tracing allows one
successful global subscriber per process, and each scenario needs the global
state to itself.

## Known exception: `src/lib.rs` line 90 (1 line)

Line 90 re-validates `EnvFilter::try_new(&config.log_level)` inside the OTLP
branch. The identical call at line 46 already rejects any invalid level string
before the OTLP branch is reachable, so line 90's `map_err` arm is
**provably unreachable** defensive code. It is kept deliberately: removing it
would couple the branch's correctness to the earlier validation surviving
refactors. If it is ever removed, the earlier check must be audited first.
