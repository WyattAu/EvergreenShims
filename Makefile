# EvergreenShims Makefile
# Quickstart: make deploy

.PHONY: build build-health build-db build-proxy deploy test clean

# Build health-shim (smallest, fastest)
build-health:
	cargo build --release -p evergreen-shim --no-default-features --features health

# Build db-shim (health + vault + backup + migration + audit)
build-db:
	cargo build --release -p evergreen-shim --no-default-features --features "health,db-shim"

# Build proxy-shim (health + proxy)
build-proxy:
	cargo build --release -p evergreen-shim --no-default-features --features "health,proxy"

# Build all variants
build: build-health build-db build-proxy

# Deploy to test server
deploy: build-health
	scp target/release/shim wyatt@192.168.1.191:~/evergreen-shims/bin/shim
	ssh wyatt@192.168.1.191 "cd ~/evergreen-shims && pkill -f 'bin/shim' 2>/dev/null; sleep 1; setsid ./bin/shim run -c /usr/bin/sleep -- infinity > data/shim.log 2>&1 &"
	sleep 3
	ssh wyatt@192.168.1.191 "curl -s http://localhost:9101/livez && echo"

# Run all tests
test:
	cargo test --workspace --lib

# Run clippy
clippy:
	cargo clippy --workspace --all-targets -- -D warnings

# Format code
fmt:
	cargo fmt --all

# Clean build artifacts
clean:
	cargo clean
