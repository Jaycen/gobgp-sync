use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use crate::models::country::CountryCodeMap;

mod cli;
mod schedule;
mod types;

pub use cli::{Cli, CliArgs, Commands, InstallArgs};
pub use schedule::SyncSchedule;
pub use types::{ConfigFile, IpVersion, Settings};

impl Settings {
    // CLI > 配置文件 > 代码默认值
    pub fn from_args(args: CliArgs) -> anyhow::Result<Self> {
        let mut config = Settings {
            ip_version: IpVersion::Dual,
            country_code: "CN".to_string(),
            sync_time: "02:00".to_string(),
            sync_schedule: SyncSchedule::default(),
            gobgpd_config: "config/gobgpd.conf".to_string(),
            gobgp_api_host: "127.0.0.1".to_string(),
            gobgp_api_port: 50051,
            gobgp_nexthop_ipv4: "0.0.0.0".to_string(),
            gobgp_nexthop_ipv6: "::".to_string(),
            community_nexthop_ipv4: HashMap::new(),
            community_nexthop_ipv6: HashMap::new(),
            log_file: "logs/gobgp_sync.log".to_string(),
            snapshot_dir: "snapshot".to_string(),
            community_asn: "3166".to_string(),
            concurrency: 100,
            domains_file: "config/domains.txt".to_string(),
            dns_interval: "10m".to_string(),
            dns_interval_secs: 600,
            dns_servers: Self::default_dns_servers(),
            snapshot_ipv4_file: String::new(),
            snapshot_ipv6_file: String::new(),
            snapshot_dns_file: String::new(),
            geo_urls: Self::default_geo_urls(),
        };

        let config_path = args
            .config
            .clone()
            .unwrap_or_else(|| PathBuf::from("config/config.toml"));
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            if let Ok(raw) = toml::from_str::<toml::Value>(&content) {
                if raw.get("bgp").is_some() {
                    log::warn!("ignored [bgp] in config.toml, use gobgpd.conf for BGP settings");
                }
            }
            let cfg_file: ConfigFile = toml::from_str(&content)?;
            if let Some(g) = cfg_file.global {
                if let Some(v) = Self::nonempty(g.ip_version) {
                    config.ip_version = IpVersion::from_str(&v);
                }
                if let Some(v) = Self::nonempty(g.country_code) {
                    config.country_code = v.to_uppercase();
                }
                if let Some(v) = Self::nonempty(g.sync_time) {
                    config.sync_time = v;
                }
                if let Some(v) = Self::nonempty(g.log_file) {
                    config.log_file = v;
                }
                if let Some(v) = Self::nonempty(g.snapshot_dir) {
                    config.snapshot_dir = v;
                }
                if let Some(v) = Self::nonempty(g.community_asn) {
                    config.community_asn = v;
                }
                if let Some(v) = g.concurrency {
                    if v > 0 {
                        config.concurrency = v;
                    }
                }
            }
            if let Some(g) = cfg_file.gobgp {
                if let Some(v) = Self::nonempty(g.config) {
                    config.gobgpd_config = v;
                }
                if let Some(v) = Self::nonempty(g.api_host) {
                    config.gobgp_api_host = v;
                }
                if let Some(v) = g.api_port {
                    config.gobgp_api_port = v;
                }
                if let Some(v) = Self::nonempty(g.nexthop_ipv4) {
                    config.gobgp_nexthop_ipv4 = v;
                }
                if let Some(v) = Self::nonempty(g.nexthop_ipv6) {
                    config.gobgp_nexthop_ipv6 = v;
                }
                if let Some(v) = g.community_nexthop_ipv4 {
                    config.community_nexthop_ipv4 = config.convert_country_next_hop_map(v, "IPv4");
                }
                if let Some(v) = g.community_nexthop_ipv6 {
                    config.community_nexthop_ipv6 = config.convert_country_next_hop_map(v, "IPv6");
                }
            }
            if let Some(g) = cfg_file.geo {
                if let Some(v) = Self::nonempty(g.ipv4_url) {
                    config.geo_urls.insert("ipv4".to_string(), v);
                }
                if let Some(v) = Self::nonempty(g.ipv6_url) {
                    config.geo_urls.insert("ipv6".to_string(), v);
                }
            }
            if let Some(d) = cfg_file.dns {
                if let Some(v) = Self::nonempty(d.domains_file) {
                    config.domains_file = v;
                }
                if let Some(v) = Self::nonempty(d.interval) {
                    config.dns_interval = v;
                }
                if let Some(v) = Self::nonempty(d.servers) {
                    config.dns_servers = Self::parse_dns_servers(&v);
                }
            }
        } else if args.config.is_some() {
            log::warn!("config file not found: {}", config_path.display());
        }

        if let Some(v) = Self::nonempty_str(args.ip_version.as_deref()) {
            config.ip_version = IpVersion::from_str(&v);
        }
        if let Some(v) = Self::nonempty_str(args.country_code.as_deref()) {
            config.country_code = v.to_uppercase();
        }
        if let Some(v) = Self::nonempty_str(args.sync_time.as_deref()) {
            config.sync_time = v;
        }
        if let Some(v) = args
            .gobgpd_config
            .as_ref()
            .and_then(|p| Self::nonempty_str(Some(&p.to_string_lossy())))
        {
            config.gobgpd_config = v;
        }
        if let Some(v) = Self::nonempty_str(args.gobgp_api_host.as_deref()) {
            config.gobgp_api_host = v;
        }
        if let Some(v) = args.gobgp_api_port {
            config.gobgp_api_port = v;
        }
        if let Some(v) = Self::nonempty_str(args.gobgp_nexthop_ipv4.as_deref()) {
            config.gobgp_nexthop_ipv4 = v;
        }
        if let Some(v) = Self::nonempty_str(args.gobgp_nexthop_ipv6.as_deref()) {
            config.gobgp_nexthop_ipv6 = v;
        }
        for item in &args.community_nexthop_ipv4 {
            if let Some((code, next_hop)) = config.parse_country_next_hop(item, "IPv4") {
                config.community_nexthop_ipv4.insert(code, next_hop);
            }
        }
        for item in &args.community_nexthop_ipv6 {
            if let Some((code, next_hop)) = config.parse_country_next_hop(item, "IPv6") {
                config.community_nexthop_ipv6.insert(code, next_hop);
            }
        }
        if let Some(v) = Self::nonempty_str(args.log_file.as_deref()) {
            config.log_file = v;
        }
        if let Some(v) = Self::nonempty_str(args.snapshot_dir.as_deref()) {
            config.snapshot_dir = v;
        }
        if let Some(v) = Self::nonempty_str(args.community_asn.as_deref()) {
            config.community_asn = v;
        }
        if let Some(v) = Self::nonempty_str(args.geo_url_ipv4.as_deref()) {
            config.geo_urls.insert("ipv4".to_string(), v);
        }
        if let Some(v) = Self::nonempty_str(args.geo_url_ipv6.as_deref()) {
            config.geo_urls.insert("ipv6".to_string(), v);
        }
        if let Some(v) = args.concurrency {
            if v > 0 {
                config.concurrency = v;
            }
        }
        if let Some(v) = Self::nonempty_str(args.domains_file.as_deref()) {
            config.domains_file = v;
        }
        if let Some(v) = Self::nonempty_str(args.dns_interval.as_deref()) {
            config.dns_interval = v;
        }
        if let Some(v) = Self::nonempty_str(args.dns_servers.as_deref()) {
            config.dns_servers = Self::parse_dns_servers(&v);
        }

        config.validate_country_code();
        config.sync_schedule = SyncSchedule::parse(&config.sync_time);
        config.dns_interval_secs = Self::parse_dns_interval(&config.dns_interval);
        if config.dns_servers.is_empty() {
            log::warn!("no valid dns_servers, using 223.5.5.5,119.29.29.29");
            config.dns_servers = Self::default_dns_servers();
        }
        config.validate_community_asn();
        config.validate_next_hops();
        config.validate_gobgpd_config()?;

        let snap = Path::new(&config.snapshot_dir);
        config.snapshot_ipv4_file = snap
            .join("snapshot_ipv4_routing.prefix")
            .to_string_lossy()
            .into_owned();
        config.snapshot_ipv6_file = snap
            .join("snapshot_ipv6_routing.prefix")
            .to_string_lossy()
            .into_owned();
        config.snapshot_dns_file = snap
            .join("snapshot_dns_routing.prefix")
            .to_string_lossy()
            .into_owned();

        Ok(config)
    }

    /// Parse `10m` / `30s` / `1h` / bare seconds. Invalid → 600s.
    pub fn parse_dns_interval(raw: &str) -> u64 {
        let raw = raw.trim();
        if raw.is_empty() {
            log::warn!("empty dns_interval, using 10m");
            return 600;
        }
        if let Ok(secs) = raw.parse::<u64>() {
            return secs.max(1);
        }
        let (num, unit) = if let Some(n) = raw.strip_suffix(['s', 'S']) {
            (n, 's')
        } else if let Some(n) = raw.strip_suffix(['m', 'M']) {
            (n, 'm')
        } else if let Some(n) = raw.strip_suffix(['h', 'H']) {
            (n, 'h')
        } else {
            log::warn!("invalid dns_interval: {}, using 10m", raw);
            return 600;
        };
        let Ok(n) = num.trim().parse::<u64>() else {
            log::warn!("invalid dns_interval: {}, using 10m", raw);
            return 600;
        };
        let secs = match unit {
            's' => n,
            'm' => n.saturating_mul(60),
            'h' => n.saturating_mul(3600),
            _ => 600,
        };
        secs.max(1)
    }

    fn default_dns_servers() -> Vec<String> {
        vec!["223.5.5.5".to_string(), "119.29.29.29".to_string()]
    }

    pub fn parse_dns_servers(raw: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for tok in raw.split(|c: char| c == ',' || c.is_whitespace()) {
            let tok = tok.trim();
            if tok.is_empty() {
                continue;
            }
            if tok.parse::<IpAddr>().is_err() {
                log::warn!("ignored invalid dns_servers entry: {}", tok);
                continue;
            }
            if seen.insert(tok.to_string()) {
                out.push(tok.to_string());
            }
        }
        out
    }

    fn nonempty(value: Option<String>) -> Option<String> {
        Self::nonempty_str(value.as_deref())
    }

    fn nonempty_str(value: Option<&str>) -> Option<String> {
        value.and_then(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        })
    }

    fn validate_gobgpd_config(&self) -> anyhow::Result<()> {
        let path = Path::new(&self.gobgpd_config);
        if !path.is_file() {
            anyhow::bail!(
                "gobgpd config not found: {} (set --gobgpd-config or [gobgp].config)",
                path.display()
            );
        }
        Ok(())
    }

    fn validate_community_asn(&mut self) {
        let asn = self.community_asn.trim();
        if asn.parse::<u32>().is_err() {
            log::warn!("invalid community_asn: {}, using 3166", self.community_asn);
            self.community_asn = "3166".to_string();
        } else {
            self.community_asn = asn.to_string();
        }
    }

    fn validate_country_code(&mut self) {
        let countries = CountryCodeMap::default();
        let mut valid = Vec::new();
        for token in Self::split_country_tokens(&self.country_code) {
            if token == "ALL" || token == "NONECN" || countries.contains(&token) {
                if !valid.contains(&token) {
                    valid.push(token);
                }
            } else {
                log::warn!("ignored invalid country_code: {}", token);
            }
        }

        if valid.iter().any(|t| t == "ALL") {
            if valid.len() > 1 {
                log::warn!("country_code ALL ignores other values");
            }
            self.country_code = "ALL".to_string();
            return;
        }
        if valid.iter().any(|t| t == "NONECN") {
            if valid.len() > 1 {
                log::warn!("country_code NONECN ignores other values");
            }
            self.country_code = "NONECN".to_string();
            return;
        }
        if valid.is_empty() {
            log::warn!("invalid country_code: {}, using CN", self.country_code);
            self.country_code = "CN".to_string();
            return;
        }
        valid.sort();
        self.country_code = valid.join(",");
    }

    pub fn country_tokens(&self) -> Vec<String> {
        Self::split_country_tokens(&self.country_code)
    }

    fn split_country_tokens(raw: &str) -> Vec<String> {
        raw.split(|c: char| c == ',' || c.is_whitespace())
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn is_all(&self) -> bool {
        self.country_code == "ALL"
    }

    pub fn selected_countries(&self) -> Vec<String> {
        let countries = CountryCodeMap::default();
        self.country_tokens()
            .into_iter()
            .filter(|t| t != "ALL" && t != "NONECN" && countries.contains(t))
            .collect()
    }

    pub fn should_filter_cn(&self) -> bool {
        self.country_code == "NONECN"
    }

    pub fn gobgp_api_addr(&self) -> String {
        format!("http://{}:{}", self.gobgp_api_host, self.gobgp_api_port)
    }

    // 按团体字中国家数字码选下一跳，未命中则用默认值
    pub fn next_hop_for_community(&self, community: &str, is_ipv6: bool) -> String {
        let code = community
            .split_once(':')
            .map(|(_, code)| code)
            .unwrap_or_default();
        let overrides = if is_ipv6 {
            &self.community_nexthop_ipv6
        } else {
            &self.community_nexthop_ipv4
        };

        if let Some(next_hop) = overrides.get(code) {
            return next_hop.clone();
        }

        if is_ipv6 {
            self.gobgp_nexthop_ipv6.clone()
        } else {
            self.gobgp_nexthop_ipv4.clone()
        }
    }

    /// 团体字：`community_asn`:`ISO3166-1 numeric`；无数字码时回落 `{ASN}:{ASN}`
    pub fn community_for_country(&self, country: &str) -> Option<String> {
        Some(
            CountryCodeMap::default()
                .community(country, &self.community_asn)
                .unwrap_or_else(|| self.fallback_community()),
        )
    }

    fn fallback_community(&self) -> String {
        format!("{}:{}", self.community_asn, self.community_asn)
    }

    // 快照里已有前缀的旧团体字，按数字码套用当前 ASN；无法解析则回落 `{ASN}:{ASN}`
    pub fn community_from_old(&self, old_community: &str) -> String {
        let Some((_, value)) = old_community.split_once(':') else {
            return self.fallback_community();
        };
        let Ok(numeric) = value.trim().parse::<u16>() else {
            return self.fallback_community();
        };
        let map = CountryCodeMap::default();
        match map.country_for_numeric(numeric) {
            Some(country) => self
                .community_for_country(country)
                .unwrap_or_else(|| self.fallback_community()),
            None => self.fallback_community(),
        }
    }

    fn validate_next_hops(&mut self) {
        if !matches!(self.gobgp_nexthop_ipv4.parse::<IpAddr>(), Ok(IpAddr::V4(_))) {
            log::warn!(
                "invalid ipv4 next hop: {}, using 0.0.0.0",
                self.gobgp_nexthop_ipv4
            );
            self.gobgp_nexthop_ipv4 = Ipv4Addr::UNSPECIFIED.to_string();
        }

        if !matches!(self.gobgp_nexthop_ipv6.parse::<IpAddr>(), Ok(IpAddr::V6(_))) {
            log::warn!(
                "invalid ipv6 next hop: {}, using ::",
                self.gobgp_nexthop_ipv6
            );
            self.gobgp_nexthop_ipv6 = Ipv6Addr::UNSPECIFIED.to_string();
        }

        Self::validate_community_next_hops(&mut self.community_nexthop_ipv4, false);
        Self::validate_community_next_hops(&mut self.community_nexthop_ipv6, true);
    }

    fn convert_country_next_hop_map(
        &self,
        map: HashMap<String, String>,
        family: &str,
    ) -> HashMap<String, String> {
        map.into_iter()
            .filter_map(|(country, next_hop)| {
                self.country_to_numeric_code(&country, family)
                    .map(|code| (code, next_hop))
            })
            .collect()
    }

    fn parse_country_next_hop(&self, item: &str, family: &str) -> Option<(String, String)> {
        let (country, next_hop) = match item.split_once('=') {
            Some(v) => v,
            None => {
                log::warn!(
                    "invalid community next hop: {}, expected COUNTRY=NEXTHOP",
                    item
                );
                return None;
            }
        };

        self.country_to_numeric_code(country, family)
            .map(|code| (code, next_hop.trim().to_string()))
    }

    fn country_to_numeric_code(&self, country: &str, family: &str) -> Option<String> {
        let country = country.trim().to_uppercase();
        CountryCodeMap::default()
            .get(&country)
            .map(|code| code.to_string())
            .or_else(|| {
                log::warn!(
                    "ignored unknown country {} for {} next hop",
                    country,
                    family
                );
                None
            })
    }

    fn validate_community_next_hops(overrides: &mut HashMap<String, String>, is_ipv6: bool) {
        overrides.retain(|code, next_hop| {
            let valid_code = code.parse::<u16>().is_ok();
            let valid_next_hop = matches!(
                (next_hop.parse::<IpAddr>(), is_ipv6),
                (Ok(IpAddr::V4(_)), false) | (Ok(IpAddr::V6(_)), true)
            );

            if !valid_code {
                log::warn!("ignored invalid country numeric next hop: {}", code);
            }
            if !valid_next_hop {
                log::warn!("ignored invalid next hop: {}={}", code, next_hop);
            }

            valid_code && valid_next_hop
        });
    }

    fn default_geo_urls() -> HashMap<String, String> {
        let mut map = HashMap::new();
        map.insert(
            "ipv4".to_string(),
            "https://github.com/sapics/ip-location-db/releases/download/latest/user-country-ipv4-cidr.csv"
                .to_string(),
        );
        map.insert(
            "ipv6".to_string(),
            "https://github.com/sapics/ip-location-db/releases/download/latest/user-country-ipv6-cidr.csv"
                .to_string(),
        );
        map
    }
}
