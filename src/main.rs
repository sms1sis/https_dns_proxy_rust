#![cfg(feature = "cli")]

use clap::Parser;
use anyhow::{Result, Context};
use std::sync::Arc;
use https_dns_proxy_rust::{Config, Stats, run_proxy};

// CLI-only imports
use tracing::Level;
#[cfg(not(target_os = "android"))]
use tracing_subscriber::prelude::*;
#[cfg(not(target_os = "android"))]
use std::fs::File;
#[cfg(not(target_os = "android"))]
use nix::unistd::{User, Group, setuid, setgid};

#[derive(Parser, Clone)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Local IPv4/v6 address to bind to
    #[arg(short = 'a', long, default_value = "127.0.0.1")]
    listen_addr: String,

    /// Local port to bind to
    #[arg(short = 'p', long, default_value_t = 5053)]
    listen_port: u16,

    /// Number of TCP clients to serve
    #[arg(short = 'T', long, default_value_t = 20)]
    tcp_client_limit: usize,

    /// Comma-separated IPv4/v6 addresses and ports (addr:port) of DNS servers to resolve resolver host
    #[arg(short = 'b', long, default_value = "1.1.1.1")]
    bootstrap_dns: String,

    /// Optional polling interval of DNS servers
    #[arg(short = 'i', long, default_value_t = 120)]
    polling_interval: u64,

    /// Force IPv4 hostnames for DNS resolvers
    #[arg(short = '4', long)]
    force_ipv4: bool,

    /// The HTTPS path to the resolver URL
    #[arg(short = 'r', long, default_value = "https://cloudflare-dns.com/dns-query")]
    resolver_url: String,

    /// Optional HTTP proxy (e.g., socks5://127.0.0.1:1080)
    #[arg(short = 't', long)]
    proxy_server: Option<String>,

    /// Source IPv4/v6 address for outbound HTTPS connections
    #[arg(short = 'S', long)]
    source_addr: Option<String>,

    /// Use HTTP/1.1 instead of HTTP/2
    #[arg(short = 'x', long)]
    http11: bool,

    /// Use HTTP/3 (QUIC) only
    #[arg(short = 'q', long)]
    http3: bool,

    /// Maximum idle time in seconds allowed for reusing a HTTPS connection
    #[arg(short = 'm', long, default_value_t = 118)]
    max_idle_time: u64,

    /// Time in seconds to tolerate connection timeouts of reused connections
    #[arg(short = 'L', long, default_value_t = 15)]
    conn_loss_time: u64,

    /// Optional file containing CA certificates
    #[arg(short = 'C', long)]
    ca_path: Option<String>,

    /// Optional statistic printout interval (seconds, 0 = disabled)
    #[arg(short = 's', long, default_value_t = 0)]
    statistic_interval: u64,

    /// Cache TTL in seconds
    #[arg(long, default_value_t = 60)]
    cache_ttl: u64,

    /// Optional domain to exclude from cache
    #[arg(short = 'e', long)]
    exclude_domain: Option<String>,

    /// Print version and exit
    #[arg(short = 'P', long)]
    print_version: bool,

    // ── Desktop / server only ────────────────────────────────────────────────
    /// Daemonize the process
    #[cfg(not(target_os = "android"))]
    #[arg(short = 'd', long)]
    daemonize: bool,

    /// Optional user to drop privileges to after binding
    #[cfg(not(target_os = "android"))]
    #[arg(short = 'u', long)]
    user: Option<String>,

    /// Optional group to drop privileges to after binding
    #[cfg(not(target_os = "android"))]
    #[arg(short = 'g', long)]
    group: Option<String>,

    /// Increase logging verbosity (-v debug, -vv trace)
    #[cfg(not(target_os = "android"))]
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Path to log file (defaults to stderr)
    #[cfg(not(target_os = "android"))]
    #[arg(short = 'l', long)]
    logfile: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.print_version {
        println!("https_dns_proxy_rust {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Logging: only set up the tracing subscriber on non-Android targets.
    // On Android, android_logger is initialised by the JNI initLogger call.
    #[cfg(not(target_os = "android"))]
    setup_logging(args.verbose, &args.logfile);

    #[cfg(not(target_os = "android"))]
    if args.daemonize {
        // Daemonize using nix (already a dependency) — avoids the unmaintained
        // `daemonize` crate. Double-fork pattern per POSIX: first fork exits the
        // parent so the child is adopted by init; setsid() creates a new session
        // (detaches from the controlling terminal); second fork ensures the daemon
        // is never a session leader and cannot reacquire a terminal.
        use nix::unistd::{fork, setsid, ForkResult, chdir};
        use nix::sys::stat::{umask, Mode};
        umask(Mode::from_bits_truncate(0o022));
        match unsafe { fork() }.context("First fork failed")? {
            ForkResult::Parent { .. } => std::process::exit(0),
            ForkResult::Child => {}
        }
        setsid().context("setsid failed")?;
        match unsafe { fork() }.context("Second fork failed")? {
            ForkResult::Parent { .. } => std::process::exit(0),
            ForkResult::Child => {}
        }
        chdir("/tmp").context("chdir failed")?;
    }

    // Drop privileges if requested — only available on non-Android targets
    // because it depends on the `nix` crate bindings.
    #[cfg(not(target_os = "android"))]
    if args.user.is_some() || args.group.is_some() {
        drop_privileges(&args.user, &args.group)?;
    }

    let config = Config {
        listen_addr: args.listen_addr,
        listen_port: args.listen_port,
        tcp_client_limit: args.tcp_client_limit,
        bootstrap_dns: args.bootstrap_dns,
        polling_interval: args.polling_interval,
        force_ipv4: args.force_ipv4,
        allow_ipv6: !args.force_ipv4,
        resolver_url: args.resolver_url,
        proxy_server: args.proxy_server,
        source_addr: args.source_addr,
        http11: args.http11,
        http3: args.http3,
        max_idle_time: args.max_idle_time,
        conn_loss_time: args.conn_loss_time,
        ca_path: args.ca_path,
        statistic_interval: args.statistic_interval,
        cache_ttl: args.cache_ttl,
        exclude_suffixes: args.exclude_domain
            .map(|s| s.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default(),
    };

    let stats = Arc::new(Stats::new());

    // Keep _shutdown_tx alive for the entire duration of run_proxy.
    // Dropping it immediately (e.g. with `let (_, rx) = ...`) would close
    // the channel at once, causing the proxy to shut down before it starts.
    let (_shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    run_proxy(config, stats, shutdown_rx).await?;

    Ok(())
}

#[cfg(not(target_os = "android"))]
fn setup_logging(verbosity: u8, logfile: &Option<String>) {
    let level = match verbosity {
        0 => Level::INFO,
        1 => Level::DEBUG,
        _ => Level::TRACE,
    };

    let filter = tracing_subscriber::EnvFilter::from_default_env()
        .add_directive(level.into());

    let registry = tracing_subscriber::registry().with(filter);

    if let Some(path) = logfile {
        if let Ok(file) = File::create(path) {
            let layer = tracing_subscriber::fmt::layer()
                .with_writer(file)
                .with_ansi(false);
            registry.with(layer).init();
            return;
        }
    }

    let layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr);
    registry.with(layer).init();
}

#[cfg(not(target_os = "android"))]
fn drop_privileges(user_name: &Option<String>, group_name: &Option<String>) -> Result<()> {
    if let Some(group) = group_name {
        let g = Group::from_name(group)?
            .ok_or_else(|| anyhow::anyhow!("Group {} not found", group))?;
        setgid(g.gid).context("Failed to set gid")?;
    }
    if let Some(user) = user_name {
        let u = User::from_name(user)?
            .ok_or_else(|| anyhow::anyhow!("User {} not found", user))?;
        setuid(u.uid).context("Failed to set uid")?;
    }
    Ok(())
}

