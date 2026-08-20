use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "gobgp-sync",
    version,
    about = "GoBGP route sync service",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[command(flatten)]
    pub run: CliArgs,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Write systemd (Linux) or launchd (macOS) unit for this binary
    Install(InstallArgs),
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Write the unit file only, do not enable or start
    #[arg(long)]
    pub no_start: bool,
}

#[derive(Args, Debug, Clone)]
pub struct CliArgs {
    #[arg(
        short = 'c',
        long = "config",
        value_name = "FILE",
        help = "Path to TOML config file (default: config/config.toml)"
    )]
    pub config: Option<PathBuf>,

    #[arg(
        short = 'i',
        long = "ip-version",
        help = "IP version: ipv4, ipv6, dual (default: dual)"
    )]
    pub ip_version: Option<String>,

    #[arg(
        short = 'C',
        long = "country",
        help = "Country codes, comma-separated: CN,JP, ALL, NONECN (default: CN)"
    )]
    pub country_code: Option<String>,

    #[arg(
        short = 's',
        long = "sync-time",
        help = "Sync schedule, default daily 02:00. Examples: 02:00, 3d 02:00, 2w Mon 02:00, 1m 15 02:00"
    )]
    pub sync_time: Option<String>,

    #[arg(
        long = "gobgpd-config",
        value_name = "FILE",
        help = "GoBGP native config file (default: config/gobgpd.conf)"
    )]
    pub gobgpd_config: Option<PathBuf>,

    #[arg(long = "gobgp-api-host", help = "GoBGP gRPC API host")]
    pub gobgp_api_host: Option<String>,

    #[arg(long = "gobgp-api-port", help = "GoBGP gRPC API port")]
    pub gobgp_api_port: Option<u16>,

    #[arg(
        long = "gobgp-nexthop-ipv4",
        help = "Default IPv4 next hop for injected routes"
    )]
    pub gobgp_nexthop_ipv4: Option<String>,

    #[arg(
        long = "gobgp-nexthop-ipv6",
        help = "Default IPv6 next hop for injected routes"
    )]
    pub gobgp_nexthop_ipv6: Option<String>,

    #[arg(
        long = "community-nexthop-ipv4",
        value_name = "COUNTRY=NEXTHOP",
        help = "Per-country IPv4 next hop override, e.g. CN=198.19.0.254"
    )]
    pub community_nexthop_ipv4: Vec<String>,

    #[arg(
        long = "community-nexthop-ipv6",
        value_name = "COUNTRY=NEXTHOP",
        help = "Per-country IPv6 next hop override, e.g. CN=2001:db8::fe"
    )]
    pub community_nexthop_ipv6: Vec<String>,

    #[arg(
        short = 'l',
        long = "log-file",
        help = "Log file path (default: logs/gobgp_sync.log)"
    )]
    pub log_file: Option<String>,

    #[arg(
        short = 'd',
        long = "snapshot-dir",
        help = "Snapshot directory (default: snapshot)"
    )]
    pub snapshot_dir: Option<String>,

    #[arg(
        long = "community-asn",
        help = "Community ASN half (ASN:ISO3166-numeric), default 3166"
    )]
    pub community_asn: Option<String>,

    #[arg(
        long = "geo-url-ipv4",
        help = "IPv4 country CIDR CSV URL (overrides [geo].ipv4_url)"
    )]
    pub geo_url_ipv4: Option<String>,

    #[arg(
        long = "geo-url-ipv6",
        help = "IPv6 country CIDR CSV URL (overrides [geo].ipv6_url)"
    )]
    pub geo_url_ipv6: Option<String>,

    #[arg(
        long = "concurrency",
        help = "Concurrent add/delete tasks (default: 100)"
    )]
    pub concurrency: Option<usize>,

    #[arg(
        long = "domains-file",
        value_name = "FILE",
        help = "DNS domains file (default: config/domains.txt); missing disables DNS sync"
    )]
    pub domains_file: Option<String>,

    #[arg(
        long = "dns-interval",
        help = "DNS sync interval, e.g. 10m, 30s, 1h (default: 10m)"
    )]
    pub dns_interval: Option<String>,

    #[arg(
        long = "dns-servers",
        help = "Upstream DNS servers, comma-separated (default: 223.5.5.5,119.29.29.29)"
    )]
    pub dns_servers: Option<String>,
}
