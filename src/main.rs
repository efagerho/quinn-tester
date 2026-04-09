use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use quinn::{ClientConfig, Connection, Endpoint, ServerConfig};

const SERVER_ADDR: &str = "127.0.0.1:5000";
const PINGS_PER_CONN: usize = 5;
const PING: &[u8] = b"ping";
const PONG: &[u8] = b"pong";
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Stats {
    conns_started: AtomicU64,
    conns_ok: AtomicU64,
    conns_err: AtomicU64,
    pongs_ok: AtomicU64,
    pongs_err: AtomicU64,
    latency_us_total: AtomicU64,
    latency_us_min: AtomicU64,
    latency_us_max: AtomicU64,
}

impl Stats {
    fn new() -> Arc<Self> {
        let s = Arc::new(Self::default());
        s.latency_us_min.store(u64::MAX, Ordering::Relaxed);
        s
    }

    fn record_latency(&self, us: u64) {
        self.latency_us_total.fetch_add(us, Ordering::Relaxed);
        let mut cur = self.latency_us_min.load(Ordering::Relaxed);
        while us < cur {
            match self.latency_us_min.compare_exchange_weak(
                cur, us, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
        let mut cur = self.latency_us_max.load(Ordering::Relaxed);
        while us > cur {
            match self.latency_us_max.compare_exchange_weak(
                cur, us, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur = v,
            }
        }
    }

    fn print(&self, elapsed: Duration) {
        let conns_ok = self.conns_ok.load(Ordering::Relaxed);
        let conns_err = self.conns_err.load(Ordering::Relaxed);
        let pongs_ok = self.pongs_ok.load(Ordering::Relaxed);
        let pongs_err = self.pongs_err.load(Ordering::Relaxed);
        let lat_total = self.latency_us_total.load(Ordering::Relaxed);
        let lat_min = self.latency_us_min.load(Ordering::Relaxed);
        let lat_max = self.latency_us_max.load(Ordering::Relaxed);

        let secs = elapsed.as_secs_f64();
        let conn_rate = conns_ok as f64 / secs;
        let pong_rate = pongs_ok as f64 / secs;
        let lat_avg = if pongs_ok > 0 { lat_total / pongs_ok } else { 0 };
        let lat_min_display = if lat_min == u64::MAX { 0 } else { lat_min };

        println!("\n=== Load test results ({secs:.1}s) ===");
        println!("Connections : {} ok  {} err  ({conn_rate:.1}/s)", conns_ok, conns_err);
        println!("Pings       : {} ok  {} err  ({pong_rate:.1}/s)", pongs_ok, pongs_err);
        println!("RTT         : avg {lat_avg}µs  min {lat_min_display}µs  max {lat_max}µs");
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("server") => run_server().await,
        Some("client") => {
            let rate: u64 = args
                .get(2)
                .context("Usage: quinn-tester client <rate> [duration_secs]")?
                .parse()
                .context("rate must be a positive integer")?;
            let duration: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
            run_load_test(rate, duration).await
        }
        Some("bench") => run_benchmark().await,
        _ => {
            eprintln!("Usage:");
            eprintln!("  quinn-tester server");
            eprintln!("  quinn-tester client <conns_per_sec> [duration_secs]");
            eprintln!("  quinn-tester bench");
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

async fn run_server() -> Result<()> {
    let server_config =
        ServerConfig::with_crypto(Arc::new(quinn_plaintext::PlaintextServerConfig));
    let endpoint = Endpoint::server(server_config, SERVER_ADDR.parse()?)?;
    println!("Server listening on {SERVER_ADDR}");

    while let Some(incoming) = endpoint.accept().await {
        tokio::spawn(async move {
            match incoming.await {
                Ok(conn) => {
                    if let Err(e) = handle_connection(conn).await {
                        eprintln!("Connection error: {e}");
                    }
                }
                Err(_) => {}
            }
        });
    }

    Ok(())
}

async fn handle_connection(conn: Connection) -> Result<()> {
    loop {
        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(s) => s,
            Err(quinn::ConnectionError::ApplicationClosed(_))
            | Err(quinn::ConnectionError::ConnectionClosed(_))
            | Err(quinn::ConnectionError::Reset)
            | Err(quinn::ConnectionError::TimedOut)
            | Err(quinn::ConnectionError::LocallyClosed) => break,
            Err(e) => return Err(e.into()),
        };

        let mut buf = [0u8; 4];
        recv.read_exact(&mut buf).await?;
        send.write_all(PONG).await?;
        send.finish()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Load test client
// ---------------------------------------------------------------------------

fn make_endpoint() -> Result<Endpoint> {
    let ep = Endpoint::client("0.0.0.0:0".parse()?)?;
    ep.set_default_client_config(ClientConfig::new(Arc::new(
        quinn_plaintext::PlaintextClientConfig,
    )));
    Ok(ep)
}

async fn run_load_test_inner(rate: u64, duration_secs: u64) -> Result<Arc<Stats>> {
    let endpoint = Arc::new(make_endpoint()?);
    let stats = Stats::new();
    let deadline = Instant::now() + Duration::from_secs(duration_secs);

    let interval_us = 1_000_000u64 / rate;
    let mut ticker = tokio::time::interval(Duration::from_micros(interval_us));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut handles = Vec::new();

    while Instant::now() < deadline {
        ticker.tick().await;

        let ep = endpoint.clone();
        let stats = stats.clone();
        stats.conns_started.fetch_add(1, Ordering::Relaxed);

        handles.push(tokio::spawn(async move {
            let result = tokio::time::timeout(
                CONNECTION_TIMEOUT,
                run_one_connection(ep),
            ).await;
            match result {
                Ok(Ok(latencies)) => {
                    stats.conns_ok.fetch_add(1, Ordering::Relaxed);
                    for us in latencies {
                        stats.pongs_ok.fetch_add(1, Ordering::Relaxed);
                        stats.record_latency(us);
                    }
                }
                Ok(Err(_)) | Err(_) => {
                    stats.conns_err.fetch_add(1, Ordering::Relaxed);
                    stats.pongs_err.fetch_add(PINGS_PER_CONN as u64, Ordering::Relaxed);
                }
            }
        }));
    }

    for h in handles {
        let _ = h.await;
    }
    endpoint.wait_idle().await;
    Ok(stats)
}

async fn run_load_test(rate: u64, duration_secs: u64) -> Result<()> {
    println!(
        "Starting load test: {rate} conn/s for {duration_secs}s \
         ({PINGS_PER_CONN} pings/conn)"
    );
    let start = Instant::now();
    let stats = run_load_test_inner(rate, duration_secs).await?;
    stats.print(start.elapsed());
    Ok(())
}

/// Opens a connection, sends PINGS_PER_CONN pings sequentially, returns per-ping RTT in µs.
async fn run_one_connection(endpoint: Arc<Endpoint>) -> Result<Vec<u64>> {
    let conn = endpoint
        .connect(SERVER_ADDR.parse()?, "localhost")?
        .await?;

    let mut latencies = Vec::with_capacity(PINGS_PER_CONN);

    for _ in 0..PINGS_PER_CONN {
        let (mut send, mut recv) = conn.open_bi().await?;
        let t0 = Instant::now();

        send.write_all(PING).await?;
        send.finish()?;

        let mut buf = [0u8; 4];
        recv.read_exact(&mut buf).await?;

        latencies.push(t0.elapsed().as_micros() as u64);
    }

    conn.close(0u32.into(), b"done");
    Ok(latencies)
}

// ---------------------------------------------------------------------------
// Benchmark: find maximum sustainable connection rate
// ---------------------------------------------------------------------------

const BENCH_DURATION: u64 = 30;
const BENCH_MAX_RATE: u64 = 1_000_000;

async fn run_benchmark() -> Result<()> {
    tokio::spawn(async {
        if let Err(e) = run_server().await {
            eprintln!("Server error: {e}");
        }
    });
    // Give the server time to bind.
    tokio::time::sleep(Duration::from_millis(200)).await;

    println!("=== Connection rate benchmark ({BENCH_DURATION}s per probe) ===");
    println!("Sustainable = zero connection errors.\n");

    let mut low = 0u64;
    let mut high = 100u64;

    // Phase 1: double until the first failing rate is found.
    println!("Phase 1: exponential probe");
    loop {
        print!("  {high:>8} conns/sec ... ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let stats = run_load_test_inner(high, BENCH_DURATION).await?;
        let ok  = stats.conns_ok.load(Ordering::Relaxed);
        let err = stats.conns_err.load(Ordering::Relaxed);
        println!("{ok} ok  {err} err");
        if err > 0 || high >= BENCH_MAX_RATE {
            break;
        }
        low = high;
        high = (high * 2).min(BENCH_MAX_RATE);
    }

    // Phase 2: binary search between last sustainable and first failing rate.
    println!("\nPhase 2: binary search ({low}..{high} conns/sec)");
    while high - low > 10 {
        let mid = (low + high) / 2;
        print!("  {mid:>8} conns/sec ... ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let stats = run_load_test_inner(mid, BENCH_DURATION).await?;
        let ok  = stats.conns_ok.load(Ordering::Relaxed);
        let err = stats.conns_err.load(Ordering::Relaxed);
        println!("{ok} ok  {err} err");
        if err == 0 {
            low = mid;
        } else {
            high = mid;
        }
    }

    println!("\nMaximum sustainable rate: {low} conns/sec");
    Ok(())
}
