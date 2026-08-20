use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, NaiveDate};
use tokio::sync::Mutex;

use crate::config::{IpVersion, Settings};
use crate::models::geo::{GeoDataFetcher, PrefixExtractor};
use crate::models::route::RouteManager;

// 前缀 + 团体字
type PrefixEntry = Vec<(String, String)>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FamilyAction {
    Skip,
    Restore,
    RewriteAttrs,
    Extract,
}

struct SnapshotState {
    date: Option<NaiveDate>,
    country_code: String,
    ip_version: IpVersion,
    attrs: String,
}

// 相对上一版快照的差异
struct PrefixDiff {
    to_add: HashMap<String, String>,
    to_del: HashMap<String, String>,
    total: usize,
    added: usize,
    removed: usize,
    changed: usize,
    unchanged: bool,
}

// 路由调度器
pub struct RouteScheduler {
    settings: Arc<Settings>,
    route_manager: Arc<RouteManager>,
    geo_fetcher: GeoDataFetcher,
    last_ipv4_prefixes: Arc<Mutex<Option<PrefixEntry>>>,
    last_ipv6_prefixes: Arc<Mutex<Option<PrefixEntry>>>,
}

impl RouteScheduler {
    pub fn new(settings: Settings) -> Self {
        let settings = Arc::new(settings);
        let route_manager = Arc::new(RouteManager::new((*settings).clone()));

        Self {
            settings,
            route_manager,
            geo_fetcher: GeoDataFetcher::new(),
            last_ipv4_prefixes: Arc::new(Mutex::new(None)),
            last_ipv6_prefixes: Arc::new(Mutex::new(None)),
        }
    }

    fn format_sorted_map(map: &HashMap<String, String>) -> String {
        let mut items: Vec<_> = map.iter().collect();
        items.sort_by(|a, b| a.0.cmp(b.0));
        items
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    // 只决定路由表大小：国家范围和地址族
    fn scope_fingerprint(settings: &Settings) -> String {
        format!(
            "country_code={}\nip_version={}\n",
            settings.country_code,
            settings.ip_version.as_str()
        )
    }

    // 团体字和下一跳：不改前缀集合，只改写入属性
    fn attr_fingerprint(settings: &Settings) -> String {
        format!(
            "community_asn={}\nnexthop_ipv4={}\nnexthop_ipv6={}\ncommunity_nexthop_ipv4={}\ncommunity_nexthop_ipv6={}\n",
            settings.community_asn,
            settings.gobgp_nexthop_ipv4,
            settings.gobgp_nexthop_ipv6,
            Self::format_sorted_map(&settings.community_nexthop_ipv4),
            Self::format_sorted_map(&settings.community_nexthop_ipv6),
        )
    }

    fn snapshot_key_path(&self) -> String {
        format!("{}/snapshot.key", self.settings.snapshot_dir)
    }

    fn load_snapshot_state(&self) -> Option<SnapshotState> {
        let text = std::fs::read_to_string(self.snapshot_key_path()).ok()?;
        let mut date = None;
        let mut country_code = None;
        let mut ip_version = None;
        let mut attrs = String::new();
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("date=") {
                date = NaiveDate::parse_from_str(v.trim(), "%Y-%m-%d").ok();
            } else if let Some(v) = line.strip_prefix("country_code=") {
                country_code = Some(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("ip_version=") {
                ip_version = Some(IpVersion::from_str(v.trim()));
            } else if line.is_empty() {
                continue;
            } else if line.starts_with("community_asn=")
                || line.starts_with("nexthop_")
                || line.starts_with("community_nexthop_")
            {
                attrs.push_str(line);
                attrs.push('\n');
            }
        }
        Some(SnapshotState {
            date,
            country_code: country_code?,
            ip_version: ip_version?,
            attrs,
        })
    }

    fn write_snapshot_state(&self) {
        let path = self.snapshot_key_path();
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let body = format!(
            "date={}\n{}{}",
            Local::now().date_naive(),
            Self::scope_fingerprint(&self.settings),
            Self::attr_fingerprint(&self.settings)
        );
        if let Err(e) = std::fs::write(&path, body) {
            log::warn!("failed to write snapshot key: {}", e);
        }
    }

    fn family_action(
        need: bool,
        snapshot_file: &str,
        lifecycle_ok: bool,
        scope_same: bool,
        attrs_same: bool,
    ) -> FamilyAction {
        if !need {
            return FamilyAction::Skip;
        }
        if !lifecycle_ok || !scope_same {
            return FamilyAction::Extract;
        }
        if RouteManager::load_snapshot(snapshot_file).is_empty() {
            return FamilyAction::Extract;
        }
        if !attrs_same {
            return FamilyAction::RewriteAttrs;
        }
        FamilyAction::Restore
    }

    pub async fn run(&self) {
        log::info!("scheduler started");
        self.sync_operation().await;

        loop {
            let now = Local::now();
            let last = self.load_snapshot_state().and_then(|s| s.date);
            let next = self.settings.sync_schedule.next_run(now, last);
            let secs = (next - now).num_seconds().max(60) as u64;

            log::info!(
                "next run at {} in {}s ({}h{}m{}s)",
                next.format("%Y-%m-%d %H:%M"),
                secs,
                secs / 3600,
                (secs % 3600) / 60,
                secs % 60
            );

            tokio::time::sleep(Duration::from_secs(secs)).await;
            self.sync_operation().await;
        }
    }

    // country_code / ip_version 变了或周期到了：从 geo CIDR 重筛并下载
    // 团体字 / 下一跳变了：沿用 .prefix 前缀，按新配置重写后再存快照
    async fn sync_operation(&self) {
        let start = Instant::now();
        log::info!("sync started");

        let today = Local::now().date_naive();
        let state = self.load_snapshot_state();
        let lifecycle_ok = state.as_ref().is_some_and(|s| {
            s.date
                .is_some_and(|d| self.settings.sync_schedule.lifecycle_ok(d, today))
        });
        let scope_same = state.as_ref().is_some_and(|s| {
            s.country_code == self.settings.country_code && s.ip_version == self.settings.ip_version
        });
        let attrs_same = state
            .as_ref()
            .is_some_and(|s| s.attrs == Self::attr_fingerprint(&self.settings));

        let need_ipv4 = self.settings.ip_version.should_process_ipv4();
        let need_ipv6 = self.settings.ip_version.should_process_ipv6();
        let action_v4 = Self::family_action(
            need_ipv4,
            &self.settings.snapshot_ipv4_file,
            lifecycle_ok,
            scope_same,
            attrs_same,
        );
        let action_v6 = Self::family_action(
            need_ipv6,
            &self.settings.snapshot_ipv6_file,
            lifecycle_ok,
            scope_same,
            attrs_same,
        );

        let mut results = Vec::new();
        let mut persist_state = false;
        let mut persist_blocked = false;

        if let Some(prev) = &state {
            if prev.ip_version.should_process_ipv4() && !need_ipv4 {
                results.push(
                    self.sync_withdraw_family(
                        "ipv4",
                        &self.settings.snapshot_ipv4_file,
                        &self.last_ipv4_prefixes,
                    )
                    .await,
                );
            }
            if prev.ip_version.should_process_ipv6() && !need_ipv6 {
                results.push(
                    self.sync_withdraw_family(
                        "ipv6",
                        &self.settings.snapshot_ipv6_file,
                        &self.last_ipv6_prefixes,
                    )
                    .await,
                );
            }
        }

        let extract_v4 = action_v4 == FamilyAction::Extract;
        let extract_v6 = action_v6 == FamilyAction::Extract;
        if extract_v4 || extract_v6 {
            if !lifecycle_ok {
                log::info!(
                    "snapshot expired, extracting prefixes from user-country ({})",
                    self.settings.country_code
                );
            } else if !scope_same {
                log::info!(
                    "scope changed, extracting prefixes from user-country ({})",
                    self.settings.country_code
                );
            } else {
                log::info!(
                    "extracting prefixes from user-country ({})",
                    self.settings.country_code
                );
            }

            match self
                .geo_fetcher
                .download_geo_data(&self.settings, extract_v4, extract_v6)
                .await
            {
                Ok(geo_data) => {
                    let (v4, v6) = PrefixExtractor::get_prefixes_by_country_mode(
                        &self.settings,
                        &geo_data,
                        Some(&self.settings.ip_version),
                    );

                    if extract_v4 && extract_v6 {
                        log::info!("geo data ready, syncing IPv4/IPv6");
                        let (lines, saved) = self.sync_dual_with_prefixes(&v4, &v6).await;
                        results.extend(lines);
                        persist_state |= saved;
                        persist_blocked |= !saved;
                    } else if extract_v4 {
                        log::info!("IPv4: extracting from geo data");
                        let (line, saved) = self
                            .sync_with_prefixes(
                                "ipv4",
                                &v4,
                                &self.settings.snapshot_ipv4_file,
                                &self.last_ipv4_prefixes,
                            )
                            .await;
                        results.push(line);
                        persist_state |= saved;
                        persist_blocked |= !saved;
                    } else {
                        log::info!("IPv6: extracting from geo data");
                        let (line, saved) = self
                            .sync_with_prefixes(
                                "ipv6",
                                &v6,
                                &self.settings.snapshot_ipv6_file,
                                &self.last_ipv6_prefixes,
                            )
                            .await;
                        results.push(line);
                        persist_state |= saved;
                        persist_blocked |= !saved;
                    }
                }
                Err(e) => {
                    persist_blocked = true;
                    if extract_v4 {
                        results.push(format!("IPv4: geo download failed: {}", e));
                    }
                    if extract_v6 {
                        results.push(format!("IPv6: geo download failed: {}", e));
                    }
                }
            }
        }

        if action_v4 == FamilyAction::RewriteAttrs {
            log::info!("IPv4: rewriting community/nexthop from prefix snapshot");
            let (line, saved) = self
                .sync_rewrite_attrs(
                    "ipv4",
                    &self.settings.snapshot_ipv4_file,
                    &self.last_ipv4_prefixes,
                )
                .await;
            results.push(line);
            persist_state |= saved;
            persist_blocked |= !saved;
        }
        if action_v6 == FamilyAction::RewriteAttrs {
            log::info!("IPv6: rewriting community/nexthop from prefix snapshot");
            let (line, saved) = self
                .sync_rewrite_attrs(
                    "ipv6",
                    &self.settings.snapshot_ipv6_file,
                    &self.last_ipv6_prefixes,
                )
                .await;
            results.push(line);
            persist_state |= saved;
            persist_blocked |= !saved;
        }

        if action_v4 == FamilyAction::Restore && action_v6 == FamilyAction::Restore {
            let (r4, r6) = tokio::join!(
                self.sync_from_snapshot(
                    "ipv4",
                    &self.settings.snapshot_ipv4_file,
                    &self.last_ipv4_prefixes,
                ),
                self.sync_from_snapshot(
                    "ipv6",
                    &self.settings.snapshot_ipv6_file,
                    &self.last_ipv6_prefixes,
                ),
            );
            results.push(r4);
            results.push(r6);
        } else if action_v4 == FamilyAction::Restore {
            results.push(
                self.sync_from_snapshot(
                    "ipv4",
                    &self.settings.snapshot_ipv4_file,
                    &self.last_ipv4_prefixes,
                )
                .await,
            );
        } else if action_v6 == FamilyAction::Restore {
            results.push(
                self.sync_from_snapshot(
                    "ipv6",
                    &self.settings.snapshot_ipv6_file,
                    &self.last_ipv6_prefixes,
                )
                .await,
            );
        }

        if persist_state && !persist_blocked {
            self.write_snapshot_state();
        }

        for line in &results {
            for part in line.split('\n') {
                if !part.is_empty() {
                    log::info!("{}", part);
                }
            }
        }
        log::info!("elapsed {:.2}s", start.elapsed().as_secs_f64());
    }

    // dual：两边差异合并为一次 batch_sync
    async fn sync_dual_with_prefixes(
        &self,
        ipv4: &HashMap<String, String>,
        ipv6: &HashMap<String, String>,
    ) -> (Vec<String>, bool) {
        let last_v4 = {
            let guard = self.last_ipv4_prefixes.lock().await;
            Self::previous_prefixes(guard.as_ref(), &self.settings.snapshot_ipv4_file)
        };
        let last_v6 = {
            let guard = self.last_ipv6_prefixes.lock().await;
            Self::previous_prefixes(guard.as_ref(), &self.settings.snapshot_ipv6_file)
        };

        let diff4 = Self::compute_diff(ipv4, &last_v4);
        let diff6 = Self::compute_diff(ipv6, &last_v6);

        let mut fail_total = 0u32;
        let mut lines = vec![
            format!(
                "IPv4: {} prefixes, added {}, removed {}, changed {}",
                diff4.total, diff4.added, diff4.removed, diff4.changed
            ),
            format!(
                "IPv6: {} prefixes, added {}, removed {}, changed {}",
                diff6.total, diff6.added, diff6.removed, diff6.changed
            ),
        ];

        if !(diff4.unchanged && diff6.unchanged) {
            let mut to_add = diff4.to_add;
            to_add.extend(diff6.to_add);
            let mut to_del = diff4.to_del;
            to_del.extend(diff6.to_del);

            let (ok, fail, elapsed, rate) = self
                .route_manager
                .batch_sync(&to_add, &to_del, "DUAL")
                .await;
            fail_total += fail;
            lines.push(format!(
                "dual: sync ok {}, fail {}, elapsed {:.1}s, rate {:.0}/s{}",
                ok,
                fail,
                elapsed,
                rate,
                if fail > 0 {
                    ", keeping old snapshot for retry"
                } else {
                    ""
                }
            ));
        }

        if fail_total == 0 {
            let (ok4, fail4, elapsed4, rate4, missing4, _) =
                self.reconcile_prefixes_with_rib("ipv4", ipv4).await;
            let (ok6, fail6, elapsed6, rate6, missing6, _) =
                self.reconcile_prefixes_with_rib("ipv6", ipv6).await;
            fail_total += fail4 + fail6;
            if missing4 > 0 {
                lines.push(format!(
                    "IPv4: rib missing added {}\nIPv4: rib sync ok {}, fail {}, elapsed {:.1}s, rate {:.0}/s",
                    missing4, ok4, fail4, elapsed4, rate4
                ));
            }
            if missing6 > 0 {
                lines.push(format!(
                    "IPv6: rib missing added {}\nIPv6: rib sync ok {}, fail {}, elapsed {:.1}s, rate {:.0}/s",
                    missing6, ok6, fail6, elapsed6, rate6
                ));
            }
        }

        if fail_total > 0 {
            return (lines, false);
        }

        let v4_saved = self
            .route_manager
            .save_snapshot(ipv4, &self.settings.snapshot_ipv4_file)
            .map_err(|e| log::warn!("IPv4: failed to save snapshot: {}", e))
            .is_ok();
        let v6_saved = self
            .route_manager
            .save_snapshot(ipv6, &self.settings.snapshot_ipv6_file)
            .map_err(|e| log::warn!("IPv6: failed to save snapshot: {}", e))
            .is_ok();

        Self::store_cached_prefixes(&self.last_ipv4_prefixes, ipv4, false).await;
        Self::store_cached_prefixes(&self.last_ipv6_prefixes, ipv6, false).await;

        (lines, v4_saved && v6_saved)
    }

    fn compute_diff(
        prefixes: &HashMap<String, String>,
        last_set: &HashMap<String, String>,
    ) -> PrefixDiff {
        let last_keys: HashSet<_> = last_set.keys().cloned().collect();
        let current_keys: HashSet<_> = prefixes.keys().cloned().collect();
        let added = current_keys.difference(&last_keys).count();
        let removed = last_keys.difference(&current_keys).count();
        let changed_prefixes: Vec<String> = prefixes
            .iter()
            .filter(|(prefix, community)| {
                last_set.get(*prefix).is_some_and(|last| last != *community)
            })
            .map(|(prefix, _)| prefix.clone())
            .collect();
        let changed = changed_prefixes.len();
        let unchanged = prefixes == last_set;

        let to_add: HashMap<String, String> = prefixes
            .iter()
            .filter(|(prefix, community)| {
                !last_set.contains_key(*prefix)
                    || last_set.get(*prefix).is_some_and(|last| last != *community)
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let mut to_del: HashMap<String, String> = last_set
            .iter()
            .filter(|(prefix, _)| !prefixes.contains_key(*prefix))
            .map(|(prefix, community)| (prefix.clone(), community.clone()))
            .collect();
        to_del.extend(changed_prefixes.into_iter().filter_map(|prefix| {
            last_set
                .get(&prefix)
                .map(|community| (prefix, community.clone()))
        }));

        PrefixDiff {
            to_add,
            to_del,
            total: prefixes.len(),
            added,
            removed,
            changed,
            unchanged,
        }
    }

    async fn sync_from_snapshot(
        &self,
        protocol: &str,
        snapshot_file: &str,
        last_prefixes_lock: &Arc<Mutex<Option<PrefixEntry>>>,
    ) -> String {
        let tag = protocol.to_uppercase();
        let snapshot_prefixes = RouteManager::load_snapshot(snapshot_file);

        if snapshot_prefixes.is_empty() {
            return format!("{}: snapshot empty, skip", tag);
        }

        Self::store_cached_prefixes(last_prefixes_lock, &snapshot_prefixes, true).await;

        let snapshot_count = snapshot_prefixes.len();
        let (ok, fail, elapsed, rate, missing_count, gobgp_count) = self
            .reconcile_prefixes_with_rib(protocol, &snapshot_prefixes)
            .await;

        if missing_count == 0 {
            return format!(
                "{}: snapshot {}, gobgp {}, missing 0",
                tag, snapshot_count, gobgp_count,
            );
        }

        format!(
            "{}: snapshot {}, missing added {}\n{}: sync ok {}, fail {}, elapsed {:.1}s, rate {:.0}/s",
            tag, snapshot_count, missing_count, tag, ok, fail, elapsed, rate,
        )
    }

    /// Re-inject desired prefixes that are absent from GoBGP Global RIB
    /// (e.g. after gobgpd restart while the on-disk snapshot is still valid).
    async fn reconcile_prefixes_with_rib(
        &self,
        protocol: &str,
        desired: &HashMap<String, String>,
    ) -> (u32, u32, f64, f64, usize, usize) {
        let tag = protocol.to_uppercase();
        if desired.is_empty() {
            return (0, 0, 0.0, 0.0, 0, 0);
        }

        log::info!(
            "{}: reconciling snapshot {} prefixes with gobgp rib",
            tag,
            desired.len()
        );

        let existing_prefixes = match self.route_manager.list_global_prefixes(protocol, &tag).await
        {
            Ok(prefixes) => prefixes,
            Err(e) => {
                log::error!("{}: failed to list gobgp rib: {}", tag, e);
                return (0, 1, 0.0, 0.0, desired.len(), 0);
            }
        };
        let gobgp_count = existing_prefixes.len();
        let missing = RouteManager::missing_from_rib(desired, &existing_prefixes);
        let missing_count = missing.len();

        if missing.is_empty() {
            log::info!(
                "{}: snapshot {}, gobgp {}, missing 0",
                tag,
                desired.len(),
                gobgp_count
            );
            return (0, 0, 0.0, 0.0, 0, gobgp_count);
        }

        log::info!(
            "{}: snapshot {}, gobgp {}, missing {}",
            tag,
            desired.len(),
            gobgp_count,
            missing_count
        );

        let (ok, fail, elapsed, rate) = self
            .route_manager
            .batch_sync(&missing, &HashMap::new(), &tag)
            .await;
        (ok, fail, elapsed, rate, missing_count, gobgp_count)
    }

    fn previous_prefixes(
        cached_prefixes: Option<&PrefixEntry>,
        snapshot_file: &str,
    ) -> HashMap<String, String> {
        cached_prefixes
            .map(|p| p.iter().cloned().collect())
            .unwrap_or_else(|| RouteManager::load_snapshot(snapshot_file))
    }

    async fn store_cached_prefixes(
        lock: &Arc<Mutex<Option<PrefixEntry>>>,
        entries: &HashMap<String, String>,
        only_if_empty: bool,
    ) {
        let mut guard = lock.lock().await;
        if only_if_empty && guard.is_some() {
            return;
        }

        let mut sorted: PrefixEntry = entries
            .iter()
            .map(|(prefix, community)| (prefix.clone(), community.clone()))
            .collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        *guard = Some(sorted);
    }

    async fn sync_rewrite_attrs(
        &self,
        protocol: &str,
        snapshot_file: &str,
        last_prefixes_lock: &Arc<Mutex<Option<PrefixEntry>>>,
    ) -> (String, bool) {
        let tag = protocol.to_uppercase();
        let old = RouteManager::load_snapshot(snapshot_file);
        if old.is_empty() {
            return (format!("{}: snapshot empty, skip", tag), true);
        }

        let new: HashMap<String, String> = old
            .iter()
            .map(|(prefix, community)| {
                (prefix.clone(), self.settings.community_from_old(community))
            })
            .collect();

        let (ok, fail, elapsed, rate) = self.route_manager.batch_sync(&new, &old, &tag).await;

        if fail > 0 {
            return (
                format!(
                    "{}: rewrite {} prefixes\n{}: sync ok {}, fail {}, elapsed {:.1}s, rate {:.0}/s, keeping old snapshot for retry",
                    tag, new.len(), tag, ok, fail, elapsed, rate
                ),
                false,
            );
        }

        let saved = if let Err(e) = self.route_manager.save_snapshot(&new, snapshot_file) {
            log::warn!("{}: failed to save snapshot: {}", tag, e);
            false
        } else {
            true
        };
        Self::store_cached_prefixes(last_prefixes_lock, &new, false).await;

        (
            format!(
                "{}: rewrote {} prefixes\n{}: sync ok {}, fail {}, elapsed {:.1}s, rate {:.0}/s",
                tag,
                new.len(),
                tag,
                ok,
                fail,
                elapsed,
                rate
            ),
            saved,
        )
    }

    async fn sync_withdraw_family(
        &self,
        protocol: &str,
        snapshot_file: &str,
        last_prefixes_lock: &Arc<Mutex<Option<PrefixEntry>>>,
    ) -> String {
        let tag = protocol.to_uppercase();
        let old = RouteManager::load_snapshot(snapshot_file);
        if old.is_empty() {
            return format!("{}: no snapshot to withdraw", tag);
        }

        log::info!(
            "{}: withdrawing {} prefixes after ip_version change",
            tag,
            old.len()
        );
        let (ok, fail, elapsed, rate) = self
            .route_manager
            .batch_sync(&HashMap::new(), &old, &tag)
            .await;
        if fail == 0 {
            let _ = self
                .route_manager
                .save_snapshot(&HashMap::new(), snapshot_file);
            Self::store_cached_prefixes(last_prefixes_lock, &HashMap::new(), false).await;
        }

        format!(
            "{}: withdrew {}, sync ok {}, fail {}, elapsed {:.1}s, rate {:.0}/s",
            tag,
            old.len(),
            ok,
            fail,
            elapsed,
            rate
        )
    }

    async fn sync_with_prefixes(
        &self,
        protocol: &str,
        prefixes: &HashMap<String, String>,
        snapshot_file: &str,
        last_prefixes_lock: &Arc<Mutex<Option<PrefixEntry>>>,
    ) -> (String, bool) {
        let tag = protocol.to_uppercase();

        let last_set = {
            let guard = last_prefixes_lock.lock().await;
            Self::previous_prefixes(guard.as_ref(), snapshot_file)
        };
        let diff = Self::compute_diff(prefixes, &last_set);

        let mut fail_total = 0u32;
        let mut lines = Vec::new();

        if diff.unchanged {
            lines.push(format!(
                "{}: {} prefixes, added 0, removed 0, unchanged",
                tag, diff.total
            ));
        } else {
            let (ok, fail, elapsed, rate) = self
                .route_manager
                .batch_sync(&diff.to_add, &diff.to_del, &tag)
                .await;
            fail_total += fail;
            lines.push(format!(
                "{}: {} prefixes, added {}, removed {}, changed {}",
                tag, diff.total, diff.added, diff.removed, diff.changed
            ));
            lines.push(format!(
                "{}: sync ok {}, fail {}, elapsed {:.1}s, rate {:.0}/s{}",
                tag,
                ok,
                fail,
                elapsed,
                rate,
                if fail > 0 {
                    ", keeping old snapshot for retry"
                } else {
                    ""
                }
            ));
        }

        if fail_total == 0 {
            let (ok, fail, elapsed, rate, missing, _) =
                self.reconcile_prefixes_with_rib(protocol, prefixes).await;
            fail_total += fail;
            if missing > 0 {
                lines.push(format!(
                    "{}: rib missing added {}\n{}: rib sync ok {}, fail {}, elapsed {:.1}s, rate {:.0}/s",
                    tag, missing, tag, ok, fail, elapsed, rate
                ));
            }
        }

        if fail_total > 0 {
            return (lines.join("\n"), false);
        }

        let saved = if let Err(e) = self.route_manager.save_snapshot(prefixes, snapshot_file) {
            log::warn!("{}: failed to save snapshot: {}", tag, e);
            false
        } else {
            true
        };
        Self::store_cached_prefixes(last_prefixes_lock, prefixes, false).await;

        (lines.join("\n"), saved)
    }
}
