use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDate};

use crate::config::{IpVersion, Settings};

struct GeoCacheMeta {
    date: NaiveDate,
    url: String,
}

// 下载国家 CIDR CSV；任一失败则本轮失败并保留旧快照
pub struct GeoDataFetcher {
    retry: u32,
    timeout: u64,
}

impl Default for GeoDataFetcher {
    fn default() -> Self {
        Self {
            retry: 3,
            timeout: 120,
        }
    }
}

impl GeoDataFetcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// 按需下载 ipv4 / ipv6 CIDR 文本，key 为 `ipv4` / `ipv6`
    pub async fn download_geo_data(
        &self,
        settings: &Settings,
        need_ipv4: bool,
        need_ipv6: bool,
    ) -> anyhow::Result<HashMap<String, String>> {
        let mut families = Vec::new();
        if need_ipv4 {
            families.push("ipv4");
        }
        if need_ipv6 {
            families.push("ipv6");
        }

        let mut geo_data = HashMap::new();
        for family in families {
            if let Some(cached) = Self::load_cached(settings, family) {
                log::info!("using cached geo {}", family);
                geo_data.insert(family.to_string(), cached);
                continue;
            }

            let url = match settings.geo_urls.get(family) {
                Some(u) => u.clone(),
                None => {
                    log::warn!("unknown geo family: {}", family);
                    continue;
                }
            };

            log::info!("downloading geo {}", family);
            match self.download_with_retry(&url).await {
                Ok(data) => {
                    Self::save_cached(settings, family, &data);
                    geo_data.insert(family.to_string(), data);
                }
                Err(e) => {
                    log::error!("geo {} download failed: {}", family, e);
                    return Err(e);
                }
            }
        }

        Ok(geo_data)
    }

    fn geo_cache_dir(settings: &Settings) -> PathBuf {
        Path::new(&settings.snapshot_dir).join("geo")
    }

    fn manifest_path(settings: &Settings) -> PathBuf {
        Self::geo_cache_dir(settings).join("manifest")
    }

    fn load_manifest(settings: &Settings) -> HashMap<String, GeoCacheMeta> {
        let Ok(text) = std::fs::read_to_string(Self::manifest_path(settings)) else {
            return HashMap::new();
        };
        text.lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let name = parts.next()?.to_string();
                let date = NaiveDate::parse_from_str(parts.next()?, "%Y-%m-%d").ok()?;
                let url = parts.collect::<Vec<_>>().join(" ");
                Some((name, GeoCacheMeta { date, url }))
            })
            .collect()
    }

    fn write_manifest(settings: &Settings, entries: &HashMap<String, GeoCacheMeta>) {
        let dir = Self::geo_cache_dir(settings);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::warn!("failed to create geo cache dir: {}", e);
            return;
        }
        let mut lines: Vec<_> = entries
            .iter()
            .map(|(name, meta)| format!("{name} {} {}", meta.date, meta.url))
            .collect();
        lines.sort();
        if let Err(e) = std::fs::write(Self::manifest_path(settings), lines.join("\n")) {
            log::warn!("failed to write geo manifest: {}", e);
        }
    }

    fn load_cached(settings: &Settings, family: &str) -> Option<String> {
        let today = Local::now().date_naive();
        let meta = Self::load_manifest(settings).remove(family)?;
        let want = settings.geo_urls.get(family)?;
        if meta.url != *want {
            return None;
        }
        if !settings.sync_schedule.lifecycle_ok(meta.date, today) {
            return None;
        }
        let path = Self::geo_cache_dir(settings).join(family);
        if !path.is_file() {
            return None;
        }
        std::fs::read_to_string(&path).ok()
    }

    fn save_cached(settings: &Settings, family: &str, data: &str) {
        let dir = Self::geo_cache_dir(settings);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::warn!("failed to create geo cache dir: {}", e);
            return;
        }
        let path = dir.join(family);
        if let Err(e) = std::fs::write(&path, data) {
            log::warn!("failed to cache geo {}: {}", family, e);
            return;
        }
        let mut manifest = Self::load_manifest(settings);
        manifest.insert(
            family.to_string(),
            GeoCacheMeta {
                date: Local::now().date_naive(),
                url: settings.geo_urls.get(family).cloned().unwrap_or_default(),
            },
        );
        Self::write_manifest(settings, &manifest);
    }

    // 只设连接和单次读取超时，避免大文件被整体超时打断
    async fn download_with_retry(&self, url: &str) -> anyhow::Result<String> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(self.timeout))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;

        let mut last_error = None;

        for attempt in 1..=self.retry {
            log::info!("download attempt {}/{}", attempt, self.retry);
            for candidate_url in Self::download_urls(url) {
                match Self::download_once(&client, &candidate_url).await {
                    Ok(data) => return Ok(data),
                    Err(e) => {
                        log::warn!("download failed: {} - {}", candidate_url, e);
                        last_error = Some(e);
                    }
                }
            }

            if attempt < self.retry {
                log::warn!(
                    "download failed (attempt {}/{}), retry in 3s",
                    attempt,
                    self.retry
                );
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("download failed")))
    }

    fn download_urls(url: &str) -> Vec<String> {
        let mut urls = vec![url.to_string()];
        if let Some(rest) = url.strip_prefix("http://") {
            urls.push(format!("https://{}", rest));
        }
        urls
    }

    async fn download_once(client: &reqwest::Client, url: &str) -> anyhow::Result<String> {
        log::info!("requesting {}", url);
        let mut resp = client.get(url).send().await?;
        let final_url = resp.url().clone();

        if !resp.status().is_success() {
            anyhow::bail!("HTTP {}: {}", resp.status(), final_url);
        }

        if final_url.as_str() != url {
            log::info!("redirected to: {}", final_url);
        }

        log::info!("reading response");
        let mut buf = String::new();
        let mut total = 0u64;

        while let Some(chunk) = resp.chunk().await? {
            total += chunk.len() as u64;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            if total.is_multiple_of(1024 * 1024) {
                log::info!("read {} MB", total / 1024 / 1024);
            }
        }

        log::info!("read complete, {} bytes", total);
        Ok(buf)
    }
}

enum CountryLineFilter<'a> {
    Any,
    Exclude(&'a str),
    Only(&'a HashSet<String>),
}

pub struct PrefixExtractor;

impl PrefixExtractor {
    pub fn is_valid_cidr(prefix: &str) -> bool {
        let (ip_part, len_part) = match prefix.split_once('/') {
            Some(p) => p,
            None => return false,
        };

        if ip_part.contains('*') || ip_part.contains(' ') {
            return false;
        }

        let prefix_len: u32 = match len_part.parse() {
            Ok(n) => n,
            Err(_) => return false,
        };

        if ip_part.contains(':') {
            prefix_len <= 128
        } else {
            let octets: Vec<&str> = ip_part.split('.').collect();
            if octets.len() != 4 {
                return false;
            }
            for octet in &octets {
                match octet.parse::<u32>() {
                    Ok(n) if n <= 255 => {}
                    _ => return false,
                }
            }
            prefix_len <= 32
        }
    }

    fn keep_country(cc: &str, filter: &CountryLineFilter<'_>) -> bool {
        match filter {
            CountryLineFilter::Any => true,
            CountryLineFilter::Exclude(code) => cc != *code,
            CountryLineFilter::Only(set) => set.contains(cc),
        }
    }

    // CIDR CSV: cidr,country_code（与 ip-location-db user-country 等格式兼容）
    fn extract_cidr_csv(
        text: &str,
        filter: &CountryLineFilter<'_>,
        settings: &Settings,
    ) -> HashMap<String, String> {
        let mut prefixes = HashMap::new();

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let (cidr, cc) = match line.split_once(',') {
                Some(p) => p,
                None => continue,
            };
            let cidr = cidr.trim();
            let cc = cc.trim().to_uppercase();
            if cidr.is_empty() || cc.is_empty() {
                continue;
            }

            if !Self::keep_country(&cc, filter) {
                continue;
            }

            if Self::is_valid_cidr(cidr) {
                let community = settings.community_for_country(&cc).unwrap_or_default();
                prefixes.insert(cidr.to_string(), community);
            } else {
                log::warn!("skipping invalid cidr: {}", cidr);
            }
        }

        prefixes
    }

    // ALL / NONECN / 国家码
    pub fn get_prefixes_by_country_mode(
        settings: &Settings,
        geo_data: &HashMap<String, String>,
        ip_version: Option<&IpVersion>,
    ) -> (HashMap<String, String>, HashMap<String, String>) {
        let mut ipv4_prefixes = HashMap::new();
        let mut ipv6_prefixes = HashMap::new();

        let process_ipv4 = ip_version.map(|v| v.should_process_ipv4()).unwrap_or(true);
        let process_ipv6 = ip_version.map(|v| v.should_process_ipv6()).unwrap_or(true);

        let countries: HashSet<String> = settings.selected_countries().into_iter().collect();
        let any = CountryLineFilter::Any;
        let exclude = CountryLineFilter::Exclude("CN");
        let only = CountryLineFilter::Only(&countries);

        let filter = if settings.is_all() {
            &any
        } else if settings.should_filter_cn() {
            &exclude
        } else {
            &only
        };

        if process_ipv4 {
            if let Some(data) = geo_data.get("ipv4") {
                ipv4_prefixes.extend(Self::extract_cidr_csv(data, filter, settings));
            }
        }
        if process_ipv6 {
            if let Some(data) = geo_data.get("ipv6") {
                ipv6_prefixes.extend(Self::extract_cidr_csv(data, filter, settings));
            }
        }

        (ipv4_prefixes, ipv6_prefixes)
    }
}
