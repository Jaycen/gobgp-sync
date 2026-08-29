use std::collections::HashMap;

use serde::{Deserialize, Deserializer};

use super::schedule::SyncSchedule;

fn deserialize_opt_string_list<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Option::<Raw>::deserialize(deserializer)? {
        None => None,
        Some(Raw::One(s)) => Some(s),
        Some(Raw::Many(v)) => Some(v.join(",")),
    })
}

// TOML: [global] / [gobgp] / [geo] / [dns]
#[derive(Debug, Clone, Deserialize)]
pub struct ConfigFile {
    pub global: Option<GlobalConfig>,
    pub gobgp: Option<GobgpConfig>,
    pub geo: Option<GeoConfig>,
    pub dns: Option<DnsConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GlobalConfig {
    pub ip_version: Option<String>,
    #[serde(default, deserialize_with = "deserialize_opt_string_list")]
    pub country_code: Option<String>,
    pub sync_time: Option<String>,
    pub log_file: Option<String>,
    pub snapshot_dir: Option<String>,
    /// 团体字 ASN 半部，生成 `ASN:ISO3166数字码`
    pub community_asn: Option<String>,
    pub concurrency: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GobgpConfig {
    /// Path to gobgpd native config
    pub config: Option<String>,
    pub api_host: Option<String>,
    pub api_port: Option<u16>,
    pub nexthop_ipv4: Option<String>,
    pub nexthop_ipv6: Option<String>,
    pub community_nexthop_ipv4: Option<HashMap<String, String>>,
    pub community_nexthop_ipv6: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GeoConfig {
    /// IPv4 CIDR CSV URL（user-country 等兼容 `cidr,cc` 格式）
    pub ipv4_url: Option<String>,
    /// IPv6 CIDR CSV URL
    pub ipv6_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DnsConfig {
    pub domains_file: Option<String>,
    pub interval: Option<String>,
    /// Hold unseen resolved prefixes before withdraw, e.g. `6h` / `1d`
    pub grace: Option<String>,
    #[serde(default, deserialize_with = "deserialize_opt_string_list")]
    pub servers: Option<String>,
}

// 运行时配置
#[derive(Debug, Clone)]
pub struct Settings {
    pub ip_version: IpVersion,
    pub country_code: String,
    pub sync_time: String,
    pub sync_schedule: SyncSchedule,
    pub gobgpd_config: String,
    pub gobgp_api_host: String,
    pub gobgp_api_port: u16,
    pub gobgp_nexthop_ipv4: String,
    pub gobgp_nexthop_ipv6: String,
    pub community_nexthop_ipv4: HashMap<String, String>,
    pub community_nexthop_ipv6: HashMap<String, String>,
    pub log_file: String,
    pub snapshot_dir: String,
    pub snapshot_ipv4_file: String,
    pub snapshot_ipv6_file: String,
    /// 团体字 ASN，格式 `ASN:国家数字码`
    pub community_asn: String,
    pub concurrency: usize,
    pub domains_file: String,
    pub dns_interval: String,
    pub dns_interval_secs: u64,
    pub dns_grace: String,
    pub dns_grace_secs: u64,
    /// Upstream recursive DNS for A/AAAA (default: 223.5.5.5,119.29.29.29)
    pub dns_servers: Vec<String>,
    pub snapshot_dns_file: String,
    pub geo_urls: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IpVersion {
    Ipv4,
    Ipv6,
    Dual,
}

impl IpVersion {
    pub fn from_str(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "IPV4" => IpVersion::Ipv4,
            "IPV6" => IpVersion::Ipv6,
            "DUAL" => IpVersion::Dual,
            _ => {
                log::warn!("invalid ip_version: {}, using dual", s);
                IpVersion::Dual
            }
        }
    }

    pub fn should_process_ipv4(&self) -> bool {
        matches!(self, IpVersion::Ipv4 | IpVersion::Dual)
    }

    pub fn should_process_ipv6(&self) -> bool {
        matches!(self, IpVersion::Ipv6 | IpVersion::Dual)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            IpVersion::Ipv4 => "ipv4",
            IpVersion::Ipv6 => "ipv6",
            IpVersion::Dual => "dual",
        }
    }
}
