use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Local, NaiveDateTime};
use hickory_resolver::config::{NameServerConfig, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::TokioResolver;

use crate::config::{IpVersion, Settings};
use crate::models::geo::PrefixExtractor;

pub const DNS_COMMUNITY: &str = "65535:65535";
const LAST_SEEN_FMT: &str = "%Y-%m-%dT%H:%M:%S";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainRecords {
    pub ipv4: BTreeMap<String, DateTime<Local>>,
    pub ipv6: BTreeMap<String, DateTime<Local>>,
}

/// Domain → resolved host routes (/32 or /128) with last-seen time.
pub type DnsSnapshot = BTreeMap<String, DomainRecords>;

#[derive(Debug, Clone)]
pub enum LookupFamily {
    Skipped,
    /// Lookup failed or returned no addresses.
    Miss,
    Hit(BTreeSet<String>),
}

#[derive(Debug, Clone)]
pub struct DomainLookup {
    pub ipv4: LookupFamily,
    pub ipv6: LookupFamily,
}

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

    /// 每行: `前缀 团体字 域名 [last_seen]`
    /// 无时间戳的旧快照按「刚刚看到」处理，给满一个宽限期。
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

        let now = Local::now();
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
            let last_seen = parts.get(3).and_then(|s| parse_last_seen(s)).unwrap_or(now);
            let rec = out.entry(domain).or_default();
            if prefix.contains(':') {
                rec.ipv6.insert(prefix.to_string(), last_seen);
            } else {
                rec.ipv4.insert(prefix.to_string(), last_seen);
            }
        }
        out
    }

    /// 每行: `前缀 团体字 域名 last_seen`，按前缀排序
    pub fn save_snapshot(snapshot: &DnsSnapshot, path: &str) -> anyhow::Result<()> {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let mut lines: Vec<(String, String, DateTime<Local>)> = Vec::new();
        for (domain, rec) in snapshot {
            for (prefix, seen) in rec.ipv4.iter().chain(rec.ipv6.iter()) {
                lines.push((prefix.clone(), domain.clone(), *seen));
            }
        }
        lines.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

        let content = lines
            .into_iter()
            .map(|(prefix, domain, seen)| {
                format!(
                    "{} {} {} {}",
                    prefix,
                    DNS_COMMUNITY,
                    domain,
                    seen.format(LAST_SEEN_FMT)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, content)?;
        log::debug!("dns snapshot saved: {}", path);
        Ok(())
    }

    pub fn flatten(snapshot: &DnsSnapshot) -> HashSet<String> {
        let mut out = HashSet::new();
        for rec in snapshot.values() {
            out.extend(rec.ipv4.keys().cloned());
            out.extend(rec.ipv6.keys().cloned());
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

    /// Merge this round's lookups with the previous snapshot.
    /// Domains not in `domains` are dropped (immediate withdraw).
    pub fn apply_grace(
        old: &DnsSnapshot,
        lookups: &BTreeMap<String, DomainLookup>,
        domains: &[String],
        grace_secs: u64,
    ) -> DnsSnapshot {
        let now = Local::now();
        let mut out = DnsSnapshot::new();
        let mut held = 0u32;
        let mut expired = 0u32;

        for domain in domains {
            let lookup = lookups.get(domain);
            let old_rec = old.get(domain);
            let mut rec = DomainRecords::default();
            let miss = LookupFamily::Miss;
            merge_family(
                &mut rec.ipv4,
                old_rec.map(|r| &r.ipv4),
                lookup.map(|l| &l.ipv4).unwrap_or(&miss),
                now,
                grace_secs,
                &mut held,
                &mut expired,
            );
            merge_family(
                &mut rec.ipv6,
                old_rec.map(|r| &r.ipv6),
                lookup.map(|l| &l.ipv6).unwrap_or(&miss),
                now,
                grace_secs,
                &mut held,
                &mut expired,
            );
            if !rec.ipv4.is_empty() || !rec.ipv6.is_empty() {
                out.insert(domain.clone(), rec);
            }
        }

        if held > 0 || expired > 0 {
            log::info!(
                "dns: grace hold={} expire={} window={}s",
                held,
                expired,
                grace_secs
            );
        }
        out
    }

    pub async fn resolve_all(&self, domains: &[String]) -> BTreeMap<String, DomainLookup> {
        let mut out = BTreeMap::new();
        for domain in domains {
            let rec = self.resolve_one(domain).await;
            out.insert(domain.clone(), rec);
        }
        out
    }

    async fn resolve_one(&self, domain: &str) -> DomainLookup {
        let want_v4 = self.settings.ip_version.should_process_ipv4();
        let want_v6 = self.settings.ip_version.should_process_ipv6();

        let ipv4 = if want_v4 {
            self.lookup_family(domain, RecordType::A, false).await
        } else {
            LookupFamily::Skipped
        };
        let ipv6 = if want_v6 {
            self.lookup_family(domain, RecordType::AAAA, true).await
        } else {
            LookupFamily::Skipped
        };

        let v4_n = match &ipv4 {
            LookupFamily::Hit(s) => s.len(),
            _ => 0,
        };
        let v6_n = match &ipv6 {
            LookupFamily::Hit(s) => s.len(),
            _ => 0,
        };
        if v4_n == 0 && v6_n == 0 {
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
            log::info!("DNS: {} -> {} v4, {} v6", domain, v4_n, v6_n);
        }

        DomainLookup { ipv4, ipv6 }
    }

    async fn lookup_family(&self, domain: &str, rtype: RecordType, ipv6: bool) -> LookupFamily {
        match self.resolver.lookup(domain, rtype).await {
            Ok(lookup) => {
                let mut set = BTreeSet::new();
                for record in lookup.answers() {
                    match record.data.ip_addr() {
                        Some(std::net::IpAddr::V4(v4)) if !ipv6 => {
                            set.insert(format!("{v4}/32"));
                        }
                        Some(std::net::IpAddr::V6(v6)) if ipv6 => {
                            set.insert(format!("{v6}/128"));
                        }
                        _ => {}
                    }
                }
                if set.is_empty() {
                    LookupFamily::Miss
                } else {
                    LookupFamily::Hit(set)
                }
            }
            Err(e) => {
                log::warn!(
                    "DNS {} lookup failed for {}: {}",
                    if ipv6 { "AAAA" } else { "A" },
                    domain,
                    e
                );
                LookupFamily::Miss
            }
        }
    }
}

fn parse_last_seen(raw: &str) -> Option<DateTime<Local>> {
    let naive = NaiveDateTime::parse_from_str(raw, LAST_SEEN_FMT).ok()?;
    match naive.and_local_timezone(Local) {
        chrono::LocalResult::Single(dt) => Some(dt),
        chrono::LocalResult::Ambiguous(a, _) => Some(a),
        chrono::LocalResult::None => None,
    }
}

fn within_grace(last_seen: DateTime<Local>, now: DateTime<Local>, grace_secs: u64) -> bool {
    now.signed_duration_since(last_seen).num_seconds() < grace_secs as i64
}

fn merge_family(
    dest: &mut BTreeMap<String, DateTime<Local>>,
    old: Option<&BTreeMap<String, DateTime<Local>>>,
    lookup: &LookupFamily,
    now: DateTime<Local>,
    grace_secs: u64,
    held: &mut u32,
    expired: &mut u32,
) {
    match lookup {
        LookupFamily::Skipped => {}
        LookupFamily::Hit(live) => {
            for prefix in live {
                dest.insert(prefix.clone(), now);
            }
            if let Some(old) = old {
                for (prefix, seen) in old {
                    if live.contains(prefix) {
                        continue;
                    }
                    if within_grace(*seen, now, grace_secs) {
                        dest.insert(prefix.clone(), *seen);
                        *held += 1;
                    } else {
                        *expired += 1;
                    }
                }
            }
        }
        LookupFamily::Miss => {
            if let Some(old) = old {
                for (prefix, seen) in old {
                    if within_grace(*seen, now, grace_secs) {
                        dest.insert(prefix.clone(), *seen);
                        *held += 1;
                    } else {
                        *expired += 1;
                    }
                }
            }
        }
    }
}
