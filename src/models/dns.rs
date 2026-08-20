use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use hickory_resolver::TokioResolver;
use hickory_resolver::config::{NameServerConfig, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RecordType;

use crate::config::{IpVersion, Settings};
use crate::models::geo::PrefixExtractor;

pub const DNS_COMMUNITY: &str = "65535:65535";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainRecords {
    pub ipv4: BTreeSet<String>,
    pub ipv6: BTreeSet<String>,
}

/// Domain → resolved host routes (/32 or /128).
pub type DnsSnapshot = BTreeMap<String, DomainRecords>;

#[derive(Debug, Clone, Default)]
pub struct DnsDiff {
    pub to_add: HashMap<String, String>,
    pub to_del: HashMap<String, String>,
}

pub struct DnsManager {
    settings: Arc<Settings>,
    resolver: TokioResolver,
}

impl DnsManager {
    pub fn new(settings: Arc<Settings>) -> anyhow::Result<Self> {
        // 不用系统 stub DNS：macOS/部分 resolver 对多 A 记录常只返回 1 条（轮询），
        // DNS→BGP 需要完整 RRset，与 dig 一致，改走可配置的公共递归解析。
        let name_servers: Vec<NameServerConfig> = settings
            .dns_servers
            .iter()
            .filter_map(|s| s.parse::<IpAddr>().ok())
            .map(NameServerConfig::udp_and_tcp)
            .collect();
        anyhow::ensure!(
            !name_servers.is_empty(),
            "dns_servers has no valid IP addresses"
        );
        let mut builder = TokioResolver::builder_with_config(
            ResolverConfig::from_parts(None, vec![], name_servers),
            TokioRuntimeProvider::default(),
        );
        builder.options_mut().cache_size = 0;
        builder.options_mut().edns0 = true;
        let resolver = builder.build()?;
        log::info!(
            "dns resolver: {} (cache disabled)",
            settings.dns_servers.join(",")
        );
        Ok(Self { settings, resolver })
    }

    /// `None` = file missing (skip DNS sync entirely this round).
    /// `Some(vec)` = file present; may be empty.
    pub fn load_domains(path: &str) -> Option<Vec<String>> {
        let path = Path::new(path);
        if !path.is_file() {
            return None;
        }
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("failed to read domains file {}: {}", path.display(), e);
                return None;
            }
        };
        let mut domains = Vec::new();
        let mut seen = HashSet::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let domain = line.trim_end_matches('.').to_lowercase();
            if domain.is_empty() || !seen.insert(domain.clone()) {
                continue;
            }
            domains.push(domain);
        }
        Some(domains)
    }

    /// 每行: `前缀 团体字 域名`（与 snapshot_ipv4_routing.prefix 同类；域名便于对照）
    pub fn load_snapshot(path: &str) -> DnsSnapshot {
        let path = Path::new(path);
        if !path.exists() {
            return DnsSnapshot::new();
        }
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("failed to read DNS snapshot {}: {}", path.display(), e);
                return DnsSnapshot::new();
            }
        };

        let mut out = DnsSnapshot::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }
            let prefix = parts[0].trim();
            if !PrefixExtractor::is_valid_cidr(prefix) {
                continue;
            }
            let domain = parts
                .get(2)
                .map(|d| d.trim_end_matches('.').to_lowercase())
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| "_".to_string());
            let rec = out.entry(domain).or_default();
            if prefix.contains(':') {
                rec.ipv6.insert(prefix.to_string());
            } else {
                rec.ipv4.insert(prefix.to_string());
            }
        }
        out
    }

    /// 每行: `前缀 团体字 域名`，按前缀排序
    pub fn save_snapshot(snapshot: &DnsSnapshot, path: &str) -> anyhow::Result<()> {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let mut lines: Vec<(String, String)> = Vec::new();
        for (domain, rec) in snapshot {
            for prefix in rec.ipv4.iter().chain(rec.ipv6.iter()) {
                lines.push((prefix.clone(), domain.clone()));
            }
        }
        lines.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

        let content = lines
            .into_iter()
            .map(|(prefix, domain)| format!("{} {} {}", prefix, DNS_COMMUNITY, domain))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, content)?;
        log::debug!("dns snapshot saved: {}", path);
        Ok(())
    }

    pub fn flatten(snapshot: &DnsSnapshot) -> HashSet<String> {
        let mut out = HashSet::new();
        for rec in snapshot.values() {
            out.extend(rec.ipv4.iter().cloned());
            out.extend(rec.ipv6.iter().cloned());
        }
        out
    }

    /// Flatten to prefix → community map for BGP ops / RIB reconcile.
    pub fn flatten_entries(snapshot: &DnsSnapshot) -> HashMap<String, String> {
        Self::flatten(snapshot)
            .into_iter()
            .map(|prefix| (prefix, DNS_COMMUNITY.to_string()))
            .collect()
    }

    pub fn diff(old: &DnsSnapshot, new: &DnsSnapshot) -> DnsDiff {
        let old_set = Self::flatten(old);
        let new_set = Self::flatten(new);
        let mut to_add = HashMap::new();
        let mut to_del = HashMap::new();
        for prefix in new_set.difference(&old_set) {
            to_add.insert(prefix.clone(), DNS_COMMUNITY.to_string());
        }
        for prefix in old_set.difference(&new_set) {
            to_del.insert(prefix.clone(), DNS_COMMUNITY.to_string());
        }
        DnsDiff { to_add, to_del }
    }

    pub async fn resolve_all(&self, domains: &[String]) -> DnsSnapshot {
        let mut out = DnsSnapshot::new();
        for domain in domains {
            let rec = self.resolve_one(domain).await;
            out.insert(domain.clone(), rec);
        }
        out
    }

    async fn resolve_one(&self, domain: &str) -> DomainRecords {
        let mut rec = DomainRecords::default();
        let want_v4 = self.settings.ip_version.should_process_ipv4();
        let want_v6 = self.settings.ip_version.should_process_ipv6();

        if want_v4 {
            match self.resolver.lookup(domain, RecordType::A).await {
                Ok(lookup) => {
                    for record in lookup.answers() {
                        if let Some(std::net::IpAddr::V4(v4)) = record.data.ip_addr() {
                            rec.ipv4.insert(format!("{v4}/32"));
                        }
                    }
                }
                Err(e) => {
                    log::warn!("DNS A lookup failed for {}: {}", domain, e);
                }
            }
        }

        if want_v6 {
            match self.resolver.lookup(domain, RecordType::AAAA).await {
                Ok(lookup) => {
                    for record in lookup.answers() {
                        if let Some(std::net::IpAddr::V6(v6)) = record.data.ip_addr() {
                            rec.ipv6.insert(format!("{v6}/128"));
                        }
                    }
                }
                Err(e) => {
                    log::warn!("DNS AAAA lookup failed for {}: {}", domain, e);
                }
            }
        }

        if rec.ipv4.is_empty() && rec.ipv6.is_empty() {
            log::info!(
                "DNS: {} resolved to no {} addresses",
                domain,
                match self.settings.ip_version {
                    IpVersion::Ipv4 => "A",
                    IpVersion::Ipv6 => "AAAA",
                    IpVersion::Dual => "A/AAAA",
                }
            );
        } else {
            log::info!(
                "DNS: {} -> {} v4, {} v6",
                domain,
                rec.ipv4.len(),
                rec.ipv6.len()
            );
        }
        rec
    }
}
