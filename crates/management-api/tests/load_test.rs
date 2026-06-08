use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use tokio::net::TcpListener;
use tokio::sync::Barrier;
use tonic::transport::Channel;

fn percentile(sorted: &mut [f64], p: f64) -> f64 {
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((sorted.len() as f64) * p).ceil() as usize;
    sorted[idx.saturating_sub(1)]
}

async fn start_server_on_random_port() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let state = management_api::ShimState::new();
    let svc = management_api::ShimManagementServiceServer::new(state);
    let layer = management_api::rate_limiter::RateLimitLayer::new(60);

    let handle = tokio::spawn(async move {
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        tonic::transport::Server::builder()
            .layer(layer)
            .add_service(svc)
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    (addr, handle)
}

async fn connect_client(
    port: u16,
) -> management_api::proto::shim_management_service_client::ShimManagementServiceClient<Channel> {
    let channel = Channel::from_shared(format!("http://127.0.0.1:{port}"))
        .unwrap()
        .connect()
        .await
        .unwrap();

    management_api::proto::shim_management_service_client::ShimManagementServiceClient::new(channel)
}

#[tokio::test]
async fn load_test_throughput_and_latency() {
    let (addr, _server) = start_server_on_random_port().await;

    let num_clients = 100;
    let requests_per_client = 100;
    let total_requests = num_clients * requests_per_client;

    let barrier = Arc::new(Barrier::new(num_clients));
    let mut handles = Vec::with_capacity(num_clients);

    let start = Instant::now();

    for client_id in 0..num_clients {
        let barrier = barrier.clone();
        let port = addr.port();

        handles.push(tokio::spawn(async move {
            let mut client = connect_client(port).await;

            barrier.wait().await;

            let mut latencies = Vec::with_capacity(requests_per_client);

            for req_id in 0..requests_per_client {
                let req_start = Instant::now();

                let call_idx = (client_id + req_id) % 3;
                match call_idx {
                    0 => {
                        let _ = client
                            .get_status(management_api::proto::GetStatusRequest {})
                            .await;
                    }
                    1 => {
                        let _ = client
                            .get_metrics(management_api::proto::GetMetricsRequest {})
                            .await;
                    }
                    _ => {
                        let _ = client
                            .list_capabilities(management_api::proto::ListCapabilitiesRequest {})
                            .await;
                    }
                }

                latencies.push(req_start.elapsed().as_secs_f64() * 1000.0);
            }

            latencies
        }));
    }

    let mut all_latencies: Vec<f64> = Vec::with_capacity(total_requests);
    for h in handles {
        let mut lats = h.await.unwrap();
        all_latencies.append(&mut lats);
    }

    let elapsed = start.elapsed();
    let throughput = total_requests as f64 / elapsed.as_secs_f64();

    let p50 = percentile(&mut all_latencies, 0.5);
    let p95 = percentile(&mut all_latencies, 0.95);
    let p99 = percentile(&mut all_latencies, 0.99);

    println!("=== Load Test Results ===");
    println!("total_requests: {total_requests}");
    println!("elapsed: {:.2?}", elapsed);
    println!("throughput: {throughput:.1} req/s");
    println!("p50: {p50:.2} ms");
    println!("p95: {p95:.2} ms");
    println!("p99: {p99:.2} ms");
    println!(
        "min: {:.2} ms",
        all_latencies.iter().cloned().fold(f64::INFINITY, f64::min)
    );
    println!(
        "max: {:.2} ms",
        all_latencies
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max)
    );
}

#[tokio::test]
async fn load_test_rate_limiting() {
    let (addr, _server) = start_server_on_random_port().await;
    let port = addr.port();

    let mut client = connect_client(port).await;

    // Default rate limit is 60 RPM. Send 65 requests rapidly.
    let mut successes = 0;
    let mut rate_limited = 0;

    for _ in 0..65 {
        match client
            .get_status(management_api::proto::GetStatusRequest {})
            .await
        {
            Ok(_) => successes += 1,
            Err(status) => {
                if status.code() == tonic::Code::ResourceExhausted {
                    rate_limited += 1;
                }
            }
        }
    }

    println!("=== Rate Limit Test ===");
    println!("successes: {successes}");
    println!("rate_limited: {rate_limited}");

    // At least some should be rate-limited (after 60 RPM)
    assert!(
        rate_limited > 0,
        "Expected some requests to be rate-limited, got 0"
    );
    assert!(
        successes >= 55,
        "Expected at least 55 successes before rate limit, got {successes}"
    );
}

#[tokio::test]
async fn load_test_malformed_requests() {
    let (addr, _server) = start_server_on_random_port().await;
    let port = addr.port();

    let mut client = connect_client(port).await;

    // Send a ReloadConfigRequest with a null byte (should be rejected by validation)
    let result = client
        .reload_config(management_api::proto::ReloadConfigRequest {
            config_path: "path\0bad".to_string(),
        })
        .await;

    assert!(result.is_err(), "Malformed request should be rejected");
    let status = result.unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);

    // Send a ReloadConfigRequest with an overly long path
    let result = client
        .reload_config(management_api::proto::ReloadConfigRequest {
            config_path: "a".repeat(2000),
        })
        .await;

    assert!(result.is_err(), "Long request should be rejected");
}

#[tokio::test]
async fn load_test_audit_log() {
    let handles: Vec<_> = (0..50)
        .map(|i| {
            tokio::spawn(async move {
                let peer = std::net::IpAddr::from([10, 0, 0, (i % 255) as u8]);
                management_api::audit::audit_get_status(peer);
                management_api::audit::audit_reload_config(peer, "/etc/test.toml", true);
            })
        })
        .collect();

    for h in handles {
        h.await.unwrap();
    }
}
