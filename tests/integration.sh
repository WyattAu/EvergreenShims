#!/bin/bash
# EvergreenShim Integration Test Suite
# Tests critical shim behaviors that caused production outages.
#
# Usage: ./tests/integration.sh
# Requires: Docker

set -euo pipefail

SHIM_IMAGE="ghcr.io/wyattau/evergreenshim/health-shim:v2.0.0"
PASS=0
FAIL=0

cleanup() {
    docker rm -f test-shim-* 2>/dev/null || true
}
trap cleanup EXIT

assert() {
    local name="$1" actual="$2" expected="$3"
    if [ "$actual" = "$expected" ]; then
        echo "  PASS: $name (got: $actual)"
        PASS=$((PASS + 1))
    else
        echo "  FAIL: $name (expected: $expected, got: $actual)"
        FAIL=$((FAIL + 1))
    fi
}

echo "=== Building test image ==="
docker build -t shim-test -f - . << 'DOCKERFILE' 2>/dev/null
FROM busybox:latest
COPY --from=ghcr.io/wyattau/evergreenshim/health-shim:v2.0.0 /shim /shim
DOCKERFILE

echo ""
echo "=== Test 1: Exit on child death ==="
echo "  Starting shim with 'sleep infinity' child..."
docker run -d --name test-shim-1 --entrypoint=/bin/sh shim-test \
  -c '/shim run -c "sleep infinity" &
      sleep 2
      # Kill the sleep child (PID will be > 1)
      for pid in $(ls /proc | grep -E "^[0-9]+$" | grep -v "^1$"); do
        kill $pid 2>/dev/null || true
      done
      wait' 2>/dev/null
sleep 3
status=$(docker inspect test-shim-1 --format '{{.State.Status}}' 2>/dev/null || echo "unknown")
assert "Shim exits when child dies" "$status" "exited"
docker rm -f test-shim-1 2>/dev/null || true

echo ""
echo "=== Test 2: Command splitting ==="
echo "  Starting shim with multi-word -c..."
docker run --rm --name test-shim-2 --entrypoint=/bin/sh shim-test \
  -c 'timeout 2 /shim run -c "sleep 10" 2>&1; echo "SPLIT_OK"' 2>/dev/null | grep -q "SPLIT_OK" \
  && assert "Multi-word command splits correctly" "ok" "ok" \
  || assert "Multi-word command splits correctly" "fail" "ok"
docker rm -f test-shim-2 2>/dev/null || true

echo ""
echo "=== Test 3: Zombie reaping ==="
echo "  Starting shim with child that forks grandchildren..."
docker run -d --name test-shim-3 --entrypoint=/bin/sh shim-test \
  -c '/shim run -c "sleep infinity" &
      sleep 2
      # Check for zombies
      zombies=$(ls /proc/*/status 2>/dev/null | xargs grep -l "state.*zombie" 2>/dev/null | wc -l)
      echo "ZOMBIES=$zombies"' 2>/dev/null
sleep 3
log=$(docker logs test-shim-3 2>&1)
echo "$log" | grep -q "ZOMBIES=0" \
  && assert "No zombie processes" "ok" "ok" \
  || assert "No zombie processes" "fail" "ok"
docker rm -f test-shim-3 2>/dev/null || true

echo ""
echo "=== Test 4: Shim version ==="
version=$(docker run --rm --entrypoint=/shim shim-test --version 2>&1 | head -1)
assert "Reports correct version" "$version" "shim 2.0.0"

echo ""
echo "=== Test 5: Health check (TCP) ==="
echo "  Starting shim with health capability..."
docker run -d --name test-shim-5 --entrypoint=/shim shim-test \
  run -c "sleep infinity" 2>/dev/null
sleep 2
# Health port 9101 should be listening
health=$(docker exec test-shim-5 /bin/sh -c 'wget -qO- --timeout=1 http://127.0.0.1:9101/livez 2>/dev/null && echo OK' 2>/dev/null || echo "FAIL")
assert "Health endpoint responds" "$health" "OK"
docker rm -f test-shim-5 2>/dev/null || true

echo ""
echo "=== Results ==="
echo "  Passed: $PASS"
echo "  Failed: $FAIL"
if [ "$FAIL" -gt 0 ]; then
    exit 1
fi
echo "  ALL TESTS PASSED"
