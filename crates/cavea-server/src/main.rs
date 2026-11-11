//! `cavea-server`: read configuration, start logging, migrate the database,
//! serve until Ctrl-C or SIGTERM, then stop within 20 seconds (08 §14,
//! `cavea_server::serve`). `cavea-server verify …` runs the reconciler
//! instead (13 §3.3, `cavea_server::verify`); `cavea-server openapi` prints
//! the OpenAPI document (08 §6.1).

use std::sync::Arc;

use anyhow::Context as _;
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
    hold_standard_descriptors();
    let config = Config::from_env()?;
    let _logging =
        cavea_telemetry::init(&config.log, config.log_format).context("CAVEA_LOG is invalid")?;
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
    if config.admin_key.is_none() {
        tracing::warn!("CAVEA_ADMIN_KEY is not set: the workspace endpoints refuse every request");
    }
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
    if let Some(url) = config.public_url.as_ref().filter(|u| !u.is_https()) {
        tracing::warn!(
            public_url = url.as_str(),
            "CAVEA_PUBLIC_URL is not HTTPS; use TLS in production"
        );
    }
    // While no user exists: the one-time link to create the owner (09 §2),
    // the one secret the server logs (12 §4).
    let public_url = config.public_url.as_ref().map(|u| u.as_str());
    if let Some(link) = cavea_server::console::setup::prepare(&db, public_url).await? {
        cavea_server::console::setup::announce(&link, public_url.is_some());
    }
    tokio::task::spawn_blocking(cavea_server::console::password::warm).await?;
    let limiter = Limiter::new(config.rate_limit_holds, config.rate_limit_reads)
        .with_sign_in(config.rate_limit_sign_in);
    let state = AppState {
        limiter: Arc::new(limiter),
        asset_memory: cavea_server::assets::Memory::new(config.asset_max_bytes.get()),
        sign_in: Arc::default(),
        config: Arc::new(config),
        db,
        seats,
        realtime,
        health: cavea_server::http::Health::default(),
    };
    cavea_server::serve(listener, state, shutdown_signal()).await?;
    Ok(())
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
