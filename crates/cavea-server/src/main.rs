//! `cavea-server`: read configuration, start logging, migrate the database,
//! serve until Ctrl-C or SIGTERM, then stop within 20 seconds (08 §14,
//! `cavea_server::serve`). `cavea-server verify …` runs the reconciler
//! instead (13 §3.3, `cavea_server::verify`); `cavea-server openapi` prints
//! the OpenAPI document (08 §6.1); `cavea-server probe` exits 0 when the
//! server on `CAVEA_LISTEN`'s port answers `GET /readyz` with `200`, for a
//! container's health check, which has no shell or HTTP client (08 §14).

use std::sync::Arc;

use anyhow::Context as _;
use cavea_engine::Clock as _;
use cavea_server::config::Config;
use cavea_server::http::AppState;
use cavea_server::limiter::Limiter;
use cavea_server::realtime::{Realtime, Timing};
use cavea_server::seats::{Seats, ServerClock};
use cavea_server::store;
use tokio::net::TcpListener;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("verify") {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let url = std::env::var("CAVEA_DATABASE_URL").ok();
        let (mut out, mut err) = (std::io::stdout(), std::io::stderr());
        let code = runtime.block_on(cavea_server::verify::run(
            &args[1..],
            url.as_deref(),
            &mut out,
            &mut err,
        ));
        let _ = std::io::Write::flush(&mut out);
        std::process::exit(i32::from(code));
    }
    if args.first().map(String::as_str) == Some("openapi") {
        let text = cavea_server::http::openapi::text();
        std::io::Write::write_all(&mut std::io::stdout(), text.as_bytes())?;
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("probe") {
        std::process::exit(i32::from(!probe()));
    }
    hold_standard_descriptors();
    let config = Config::from_env()?;
    let _logging =
        cavea_telemetry::init(&config.log, config.log_format).context("CAVEA_LOG is invalid")?;
    config.warn_insecure();
    // Requests, actors and commits on one runtime; realtime connections'
    // streams on another, so that fanning out never queues a request behind
    // tens of thousands of ready subscribers (08 §8.3, D-41).
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    let realtime = cavea_server::realtime::runtime(cores)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cores)
        .enable_all()
        .build()?;
    let served = runtime.block_on(run(config, realtime.handle().clone()));
    realtime.shutdown_background();
    served
}

async fn run(config: Config, streams: tokio::runtime::Handle) -> anyhow::Result<()> {
    // Never print the URL: it carries the password.
    let options = config.database_url.options();
    store::migrate(&options).await?;
    let db = store::connect(options, config.db_pool.get())
        .await
        .context("cannot connect to PostgreSQL (CAVEA_DATABASE_URL)")?;

    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("cannot bind {}", config.listen))?;
    tracing::info!(addr = %listener.local_addr()?, version = env!("CARGO_PKG_VERSION"), "listening");
    let seats = Seats::new(
        db.clone(),
        &config,
        ServerClock::System,
        Some(streams.clone()),
    );
    let realtime = Realtime::on_runtime(
        config.rt_max_connections.get() as usize,
        config.rt_max_per_ip.get(),
        Timing::default(),
        streams,
    );
    // While no user exists: the one-time link to create the owner (09 §2),
    // the one secret the server logs (12 §4).
    let public_url = config.public_url.as_ref().map(|u| u.as_str());
    if let Some(link) = cavea_server::console::setup::prepare(&db, public_url).await? {
        cavea_server::console::setup::announce(&link, public_url.is_some());
    }
    tokio::task::spawn_blocking(cavea_server::console::password::warm).await?;
    let limiter = Limiter::new(config.rate_limit_holds, config.rate_limit_reads)
        .with_sign_in(config.rate_limit_sign_in);
    let licences = cavea_server::licence::Licences::new(verifier(), config.licence.clone());
    // Bounded: a licence that cannot be read in time is read again in the
    // background, and serving never waits for it (D-20).
    let read = licences.refresh(&db, seats.clock.now().0);
    if tokio::time::timeout(std::time::Duration::from_secs(5), read)
        .await
        .is_err()
    {
        tracing::warn!(
            "the licence was not read within 5 s; serving, and reading it again every 30 s"
        );
    }
    let console = cavea_server::http::web::Files::embedded();
    let (files, bytes) = console.size();
    tracing::info!(files, bytes, "console files");
    let state = AppState {
        console,
        limiter: Arc::new(limiter),
        asset_memory: cavea_server::assets::Memory::new(config.asset_max_bytes.get()),
        sign_in: Arc::default(),
        config: Arc::new(config),
        db,
        seats,
        realtime,
        health: cavea_server::http::Health::default(),
        licences,
    };
    cavea_server::serve(listener, state, shutdown_signal()).await?;
    Ok(())
}

/// The keys licences are verified against: the ones compiled into every
/// build (`cavea_licence::TRUSTED`), plus, in a build with the
/// `trust-test-key` feature only, the base64 public key that
/// `CAVEA_TEST_LICENCE_KEY` held when it was compiled (D-80). No setting read
/// at run time adds a key.
#[cfg(all(feature = "trust-test-key", not(debug_assertions)))]
compile_error!("`trust-test-key` is for debug builds only: a release never trusts a test key");

fn verifier() -> cavea_licence::Verifier {
    let keys = cavea_licence::TRUSTED
        .iter()
        .filter_map(|k| cavea_licence::PublicKey::from_base64(k));
    #[cfg(feature = "trust-test-key")]
    let keys = keys.chain(cavea_licence::PublicKey::from_base64(env!(
        "CAVEA_TEST_LICENCE_KEY"
    )));
    cavea_licence::Verifier::new(keys)
}

/// Started with standard input, output or error closed, the server's first
/// files and sockets would take those descriptors, and what is written to
/// standard output or error would reach them. Hold each closed one open on
/// `/dev/null` instead: `open` returns the lowest free descriptor.
fn hold_standard_descriptors() {
    #[cfg(unix)]
    for _ in 0..3 {
        use std::os::fd::{AsRawFd as _, IntoRawFd as _};
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null");
        match opened {
            // Kept open for the life of the process.
            Ok(file) if file.as_raw_fd() <= 2 => drop(file.into_raw_fd()),
            _ => break,
        }
    }
}

/// `GET /readyz` on this host at `CAVEA_LISTEN`'s port, within 2 s.
fn probe() -> bool {
    use std::io::{Read as _, Write as _};
    let port = std::env::var("CAVEA_LISTEN")
        .ok()
        .and_then(|l| l.parse::<std::net::SocketAddr>().ok())
        .map_or(8080, |a| a.port());
    let timeout = std::time::Duration::from_secs(2);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let mut status = [0; 12];
    stream
        .write_all(b"GET /readyz HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .and_then(|()| stream.read_exact(&mut status))
        .is_ok_and(|()| status.ends_with(b" 200"))
}

/// Resolves on Ctrl-C or, on Unix, SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "cannot listen for Ctrl-C");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutting down");
}
