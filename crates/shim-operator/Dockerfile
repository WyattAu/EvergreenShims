# Multi-stage Dockerfile for shim-operator
FROM rust:1-slim-bookworm AS builder

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        protobuf-compiler \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

RUN for attempt in 1 2 3; do \
      cargo build --release -p shim-operator 2>/dev/null && break; \
      echo "Attempt $attempt failed, retrying in 30s..."; \
      sleep 30; \
    done

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -s /bin/false operator

COPY --from=builder /src/target/release/shim-operator /usr/local/bin/shim-operator

USER operator
ENTRYPOINT ["tini", "--"]
CMD ["shim-operator"]
