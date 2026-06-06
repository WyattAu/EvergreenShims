---
layout: default
title: EvergreenShims
---

<style>
  :root {
    --bg-primary: #0a0a0a;
    --bg-surface: #141414;
    --bg-elevated: #1e1e1e;
    --text-primary: #e8e8e8;
    --text-secondary: #888888;
    --accent: #00ff88;
    --accent-dim: #00cc6a;
    --border: #333333;
    --mono: 'JetBrains Mono', 'Fira Code', 'SF Mono', monospace;
    --sans: 'Inter', -apple-system, BlinkMacSystemFont, sans-serif;
  }

  * { margin: 0; padding: 0; box-sizing: border-box; }

  body {
    background: var(--bg-primary);
    color: var(--text-primary);
    font-family: var(--sans);
    line-height: 1.6;
    overflow-x: hidden;
  }

  /* Brutalist typography */
  .hero {
    padding: 8rem 2rem 4rem;
    position: relative;
    overflow: hidden;
  }

  .hero::before {
    content: '';
    position: absolute;
    top: -50%;
    right: -20%;
    width: 600px;
    height: 600px;
    background: radial-gradient(circle, rgba(0,255,136,0.08) 0%, transparent 70%);
    border-radius: 50%;
    filter: blur(80px);
  }

  .hero h1 {
    font-family: var(--mono);
    font-size: clamp(3rem, 8vw, 6rem);
    font-weight: 900;
    letter-spacing: -0.04em;
    line-height: 1.0;
    text-transform: uppercase;
    max-width: 900px;
  }

  .hero h1 .accent {
    color: var(--accent);
    display: block;
  }

  .hero .tagline {
    font-family: var(--mono);
    font-size: 1.1rem;
    color: var(--text-secondary);
    margin-top: 1.5rem;
    max-width: 600px;
    border-left: 3px solid var(--accent);
    padding-left: 1rem;
  }

  /* Spatial layers */
  .content {
    max-width: 1200px;
    margin: 0 auto;
    padding: 0 2rem;
  }

  .section {
    margin: 4rem 0;
    padding: 3rem;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    position: relative;
  }

  .section::before {
    content: attr(data-label);
    position: absolute;
    top: -0.6rem;
    left: 2rem;
    background: var(--bg-primary);
    padding: 0 0.5rem;
    font-family: var(--mono);
    font-size: 0.7rem;
    color: var(--accent);
    text-transform: uppercase;
    letter-spacing: 0.15em;
  }

  .section h2 {
    font-family: var(--mono);
    font-size: 1.4rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-bottom: 2rem;
    padding-bottom: 1rem;
    border-bottom: 1px solid var(--border);
  }

  /* Amoebic grid - fluid, asymmetric */
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 1px;
    background: var(--border);
    border: 1px solid var(--border);
  }

  .grid-item {
    background: var(--bg-surface);
    padding: 2rem;
    transition: background 0.2s;
  }

  .grid-item:hover {
    background: var(--bg-elevated);
  }

  .grid-item h3 {
    font-family: var(--mono);
    font-size: 0.85rem;
    text-transform: uppercase;
    letter-spacing: 0.1em;
    color: var(--accent);
    margin-bottom: 0.5rem;
  }

  .grid-item p {
    font-size: 0.9rem;
    color: var(--text-secondary);
    line-height: 1.5;
  }

  .grid-item code {
    font-family: var(--mono);
    font-size: 0.8rem;
    background: var(--bg-primary);
    padding: 0.15rem 0.4rem;
    border: 1px solid var(--border);
  }

  /* Architecture diagram */
  .arch-diagram {
    font-family: var(--mono);
    font-size: 0.8rem;
    line-height: 1.8;
    background: var(--bg-primary);
    border: 1px solid var(--border);
    padding: 2rem;
    overflow-x: auto;
    white-space: pre;
    color: var(--text-secondary);
  }

  .arch-diagram .highlight {
    color: var(--accent);
  }

  /* Quick start */
  .quickstart {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 2rem;
  }

  @media (max-width: 768px) {
    .quickstart { grid-template-columns: 1fr; }
  }

  .code-block {
    background: var(--bg-primary);
    border: 1px solid var(--border);
    padding: 1.5rem;
    font-family: var(--mono);
    font-size: 0.85rem;
    line-height: 1.6;
    overflow-x: auto;
    position: relative;
  }

  .code-block::before {
    content: attr(data-lang);
    position: absolute;
    top: 0;
    right: 0;
    background: var(--accent);
    color: var(--bg-primary);
    font-size: 0.65rem;
    font-weight: 700;
    padding: 0.2rem 0.6rem;
    text-transform: uppercase;
  }

  .code-block .comment { color: #555; }
  .code-block .keyword { color: var(--accent); }
  .code-block .string { color: #ff6b6b; }

  /* Metrics strip */
  .metrics {
    display: flex;
    gap: 1px;
    background: var(--border);
    border: 1px solid var(--border);
    margin: 4rem 0;
  }

  .metric {
    flex: 1;
    background: var(--bg-surface);
    padding: 2rem;
    text-align: center;
  }

  .metric .value {
    font-family: var(--mono);
    font-size: 2.5rem;
    font-weight: 900;
    color: var(--accent);
  }

  .metric .label {
    font-family: var(--mono);
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.15em;
    color: var(--text-secondary);
    margin-top: 0.5rem;
  }

  /* Navigation */
  .nav-links {
    display: flex;
    gap: 0.5rem;
    margin-top: 2rem;
    flex-wrap: wrap;
  }

  .nav-links a {
    font-family: var(--mono);
    font-size: 0.8rem;
    color: var(--text-primary);
    text-decoration: none;
    padding: 0.6rem 1.2rem;
    border: 1px solid var(--border);
    transition: all 0.15s;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .nav-links a:hover {
    background: var(--accent);
    color: var(--bg-primary);
    border-color: var(--accent);
  }

  .nav-links a.primary {
    background: var(--accent);
    color: var(--bg-primary);
    border-color: var(--accent);
    font-weight: 700;
  }

  /* Footer */
  footer {
    margin-top: 6rem;
    padding: 3rem 2rem;
    border-top: 1px solid var(--border);
    font-family: var(--mono);
    font-size: 0.75rem;
    color: var(--text-secondary);
    text-align: center;
  }

  /* Accessibility: focus styles */
  a:focus-visible, button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  /* Reduced motion */
  @media (prefers-reduced-motion: reduce) {
    * { transition: none !important; }
  }
</style>

<section class="hero" aria-labelledby="hero-title">
  <h1 id="hero-title">
    EVERGREEN<span class="accent">SHIMS</span>
  </h1>
  <p class="tagline">
    Rust-native shims for self-managing container images. Single binary, multiple capabilities, zero runtime overhead.
  </p>
  <nav class="nav-links" aria-label="Primary navigation">
    <a href="docs/architecture" class="primary">Architecture</a>
    <a href="docs/building">Build</a>
    <a href="docs/testing">Testing</a>
    <a href="https://github.com/WyattAu/EvergreenShims">Source</a>
    <a href="https://github.com/WyattAu/EvergreenShims/releases">Releases</a>
  </nav>
</section>

<div class="content">

  <!-- Metrics strip -->
  <div class="metrics" role="list" aria-label="Project metrics">
    <div class="metric" role="listitem">
      <div class="value">32</div>
      <div class="label">Crates</div>
    </div>
    <div class="metric" role="listitem">
      <div class="value">792</div>
      <div class="label">Tests</div>
    </div>
    <div class="metric" role="listitem">
      <div class="value">~2.5MB</div>
      <div class="label">Binary (musl)</div>
    </div>
    <div class="metric" role="listitem">
      <div class="value">0</div>
      <div class="label">Unsafe</div>
    </div>
  </div>

  <!-- Architecture -->
  <section class="section" data-label="architecture" aria-labelledby="arch-heading">
    <h2 id="arch-heading">System Design</h2>
    <div class="arch-diagram" role="img" aria-label="PID 1 architecture diagram showing shim wrapping child process"><span class="highlight">PID 1:</span> /app/shim
  +-- <span class="highlight">PID N:</span> /app/postgres (child process)

<span class="highlight">Capabilities:</span>
  Health probes:       /livez, /readyz, /metrics
  Secrets rotation:    Vault/KMS integration
  Backups:             S3-compatible, WAL archiving
  Migrations:          Idempotent SQL file-based
  Audit logging:       JSON/CEF, SIEM export
  Auto-TLS:            Let's Encrypt / internal CA
  Config hot-reload:   SHA-256 hash change detection
  Failover:            Automatic primary promotion
  Chaos engineering:   Fault injection for resilience
  Multi-tenancy:       Per-tenant resource quotas</div>
  </section>

  <!-- Shim Catalog -->
  <section class="section" data-label="catalog" aria-labelledby="catalog-heading">
    <h2 id="catalog-heading">Shim Catalog</h2>
    <div class="grid" role="list">
      <div class="grid-item" role="listitem">
        <h3>health-shim</h3>
        <p>TCP/HTTP/exec probes, Prometheus metrics, child process management</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>vault-shim</h3>
        <p>Automatic credential rotation from Vault/KMS</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>backup-shim</h3>
        <p>pg_dump, mysqldump, BGSAVE -- compression, retention, S3 upload</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>migration-shim</h3>
        <p>SQL file-based, version tracking, multi-DB orchestration</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>tls-shim</h3>
        <p>Auto-TLS with Let's Encrypt ACME or internal CA</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>failover-shim</h3>
        <p>Patroni/Redis Sentinel/TCP health checks, automatic promotion</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>chaos-shim</h3>
        <p>Fault injection: latency, errors, partitions, process kill, disk fill</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>encryption-shim</h3>
        <p>AES-GCM / ChaCha20-Poly1305, key rotation</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>proxy-shim</h3>
        <p>Connection pooling, circuit breaker, weighted routing, retries</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>cache-shim</h3>
        <p>Query result caching with LRU/FIFO eviction, TTL</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>cdc-shim</h3>
        <p>Change Data Capture, WAL position tracking, Kafka/webhook output</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>compliance-shim</h3>
        <p>CIS/STIG compliance scoring and violation tracking</p>
      </div>
    </div>
  </section>

  <!-- Quick Start -->
  <section class="section" data-label="quickstart" aria-labelledby="quickstart-heading">
    <h2 id="quickstart-heading">Quick Start</h2>
    <div class="quickstart">
      <div class="code-block" data-lang="dockerfile" role="figure" aria-label="Dockerfile example">
<span class="comment"># Dockerfile</span>
<span class="keyword">FROM</span> scratch
<span class="keyword">COPY</span> --from=builder /app/db-shim /app/shim
<span class="keyword">COPY</span> --from=builder /app/postgres /app/postgres
<span class="keyword">ENTRYPOINT</span> [<span class="string">"/app/shim"</span>]</div>
      <div class="code-block" data-lang="bash" role="figure" aria-label="Binary download example">
<span class="comment"># Binary download</span>
curl -L <span class="string">https://github.com/.../shim.gz</span> \
  | gunzip > /app/shim
chmod +x /app/shim

<span class="comment"># Or build from source</span>
cargo build --release \
  --target x86_64-unknown-linux-musl \
  -p evergreen-shim \
  --features db-shim</div>
    </div>
  </section>

  <!-- Binary Matrix -->
  <section class="section" data-label="binaries" aria-labelledby="binaries-heading">
    <h2 id="binaries-heading">Pre-Built Binaries</h2>
    <div class="grid" role="list">
      <div class="grid-item" role="listitem">
        <h3>health-shim</h3>
        <p><code>~300KB</code> -- Health probes only. Any container.</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>db-shim</h3>
        <p><code>~1MB</code> -- Health + vault + backup + migration + audit.</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>proxy-shim</h3>
        <p><code>~700KB</code> -- Health + audit + TLS.</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>ha-shim</h3>
        <p><code>~800KB</code> -- Health + failover + replication.</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>full-shim</h3>
        <p><code>~3MB</code> -- All 27 shims. Full operational stack.</p>
      </div>
      <div class="grid-item" role="listitem">
        <h3>Container Images</h3>
        <p><code>ghcr.io/wyattau/evergreenshim</code> -- Multi-arch, signed.</p>
      </div>
    </div>
  </section>

</div>

<footer>
  EvergreenShims v0.3.0 -- Apache-2.0 -- Rust 2021
</footer>
