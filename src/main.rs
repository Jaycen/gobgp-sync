mod config;
pub mod gobgp;
mod install;
mod models;
mod utils;

use clap::Parser;

use crate::config::{Cli, Commands, Settings};
use crate::gobgp::process::GobgpProcess;
use crate::models::dns_scheduler::DnsScheduler;
use crate::models::scheduler::RouteScheduler;
use crate::utils::logger::Logger;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if let Some(Commands::Install(args)) = cli.command {
        return install::run(&args);
    }

    // CLI 覆盖 TOML，再覆盖代码默认值
    let settings = Settings::from_args(cli.run)?;

    // 日志依赖配置路径，须在其它业务日志之前初始化
    Logger::setup(&settings)?;

    log::info!("started");
    log::info!("country_code: {}", settings.country_code);
    if settings.should_filter_cn() {
        log::info!("country_mode: exclude CN");
    } else if settings.is_all() {
        log::info!("country_mode: all countries");
    } else {
        log::info!("country_mode: keep {}", settings.country_code);
    }
    log::info!("ip_version: {:?}", settings.ip_version);
    log::info!(
        "sync_time: {} ({})",
        settings.sync_time,
        settings.sync_schedule.describe()
    );
    log::info!(
        "dns: domains_file={} interval={} servers={} ({})",
        settings.domains_file,
        settings.dns_interval,
        settings.dns_servers.join(","),
        if std::path::Path::new(&settings.domains_file).is_file() {
            "present"
        } else {
            "missing, DNS sync idle until file appears"
        }
    );
    log::info!("gobgp_api: {}", settings.gobgp_api_addr());
    log::info!(
        "nexthop: ipv4={} ipv6={}",
        settings.gobgp_nexthop_ipv4,
        settings.gobgp_nexthop_ipv6
    );
    log::info!(
        "community_asn: {} (format ASN:ISO3166-numeric)",
        settings.community_asn
    );
    log::info!(
        "geo_urls: ipv4={} ipv6={}",
        settings
            .geo_urls
            .get("ipv4")
            .map(String::as_str)
            .unwrap_or("-"),
        settings
            .geo_urls
            .get("ipv6")
            .map(String::as_str)
            .unwrap_or("-")
    );
    log::info!("log_file: {}", settings.log_file);
    log::info!("snapshot_dir: {}", settings.snapshot_dir);
    log::info!("gobgpd_config: {}", settings.gobgpd_config);

    // 启动同目录 gobgpd，由其配置文件负责 BGP 会话
    let mut gobgpd = GobgpProcess::start(&settings).await?;
    if let Err(e) = GobgpProcess::wait_api_ready(&settings).await {
        let _ = gobgpd.stop().await;
        return Err(e);
    }

    let dns_scheduler = DnsScheduler::new(settings.clone())?;
    let scheduler = RouteScheduler::new(settings);

    // 同时监听 Ctrl+C 与 SIGTERM，便于 systemd stop 时停掉子进程
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    let shutdown = async {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = sigterm.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
    };

    tokio::select! {
        _ = scheduler.run() => {
            log::warn!("scheduler returned unexpectedly");
        }
        _ = dns_scheduler.run() => {
            log::warn!("dns scheduler returned unexpectedly");
        }
        _ = gobgpd.wait_unexpected_exit() => {}
        _ = shutdown => {
            log::info!("shutdown signal received, stopping gobgpd");
        }
    }

    gobgpd.stop().await?;
    Ok(())
}
