use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use crate::config::Settings;
use crate::models::dns::{DnsDiff, DnsManager, DnsSnapshot};
use crate::models::route::RouteManager;

pub struct DnsScheduler {
    settings: Arc<Settings>,
    dns: DnsManager,
    routes: Arc<RouteManager>,
}

impl DnsScheduler {
    pub fn new(settings: Settings) -> anyhow::Result<Self> {
        let settings = Arc::new(settings);
        let dns = DnsManager::new(Arc::clone(&settings))?;
        let routes = Arc::new(RouteManager::new((*settings).clone()));
        Ok(Self {
            settings,
            dns,
            routes,
        })
    }

    pub async fn run(&self) {
        log::info!(
            "dns scheduler started (interval={}, file={})",
            self.settings.dns_interval,
            self.settings.domains_file
        );
        self.sync_once().await;

        let interval = Duration::from_secs(self.settings.dns_interval_secs.max(1));
        loop {
            tokio::time::sleep(interval).await;
            self.sync_once().await;
        }
    }

    /// Align with country prefix sync:
    /// 1) resolve → desired snapshot
    /// 2) diff vs previous snapshot → del/add
    /// 3) reconcile desired vs GoBGP Global RIB → re-add missing
    /// 4) persist snapshot on success
    async fn sync_once(&self) {
        let Some(domains) = DnsManager::load_domains(&self.settings.domains_file) else {
            log::info!(
                "dns: domains file missing ({}), skip",
                self.settings.domains_file
            );
            return;
        };

        let old = DnsManager::load_snapshot(&self.settings.snapshot_dns_file);
        let new = if domains.is_empty() {
            log::info!("dns: domains file empty, withdrawing previous DNS routes");
            DnsSnapshot::new()
        } else {
            log::info!("dns: syncing {} domain(s)", domains.len());
            self.dns.resolve_all(&domains).await
        };

        let DnsDiff { to_add, to_del } = DnsManager::diff(&old, &new);
        let mut fail_total = 0u32;

        if !to_add.is_empty() || !to_del.is_empty() {
            log::info!(
                "dns: snapshot diff add={} del={}",
                to_add.len(),
                to_del.len()
            );
            let (ok, fail, elapsed, rate) = self.routes.batch_sync(&to_add, &to_del, "dns").await;
            log::info!(
                "dns: diff sync ok={} fail={} elapsed={:.2}s rate={:.1}/s",
                ok,
                fail,
                elapsed,
                rate
            );
            fail_total += fail;
        } else {
            log::info!("dns: snapshot unchanged");
        }

        // Like country Restore: even if snapshot diff is empty (e.g. gobgpd restarted),
        // re-inject desired prefixes missing from Global RIB.
        if fail_total == 0 {
            fail_total += self.reconcile_rib(&new).await;
        }

        if fail_total == 0 {
            if let Err(e) = DnsManager::save_snapshot(&new, &self.settings.snapshot_dns_file) {
                log::warn!("dns: failed to save snapshot: {}", e);
            }
        } else {
            log::warn!("dns: snapshot not updated due to failures");
        }
    }

    async fn reconcile_rib(&self, desired: &DnsSnapshot) -> u32 {
        let desired_entries = DnsManager::flatten_entries(desired);
        if desired_entries.is_empty() {
            return 0;
        }

        log::info!(
            "dns: reconciling snapshot {} prefixes with gobgp rib",
            desired_entries.len()
        );

        let mut rib: HashSet<String> = HashSet::new();
        if self.settings.ip_version.should_process_ipv4() {
            match self.routes.list_global_prefixes("ipv4", "dns").await {
                Ok(set) => rib.extend(set),
                Err(e) => {
                    log::error!("dns: failed to list ipv4 rib: {}", e);
                    return 1;
                }
            }
        }
        if self.settings.ip_version.should_process_ipv6() {
            match self.routes.list_global_prefixes("ipv6", "dns").await {
                Ok(set) => rib.extend(set),
                Err(e) => {
                    log::error!("dns: failed to list ipv6 rib: {}", e);
                    return 1;
                }
            }
        }

        let missing = RouteManager::missing_from_rib(&desired_entries, &rib);
        if missing.is_empty() {
            log::info!(
                "dns: snapshot {}, gobgp covers all, missing 0",
                desired_entries.len()
            );
            return 0;
        }

        log::info!(
            "dns: snapshot {}, gobgp {}, missing {}",
            desired_entries.len(),
            rib.len(),
            missing.len()
        );
        let (ok, fail, elapsed, rate) = self
            .routes
            .batch_sync(&missing, &HashMap::new(), "dns")
            .await;
        log::info!(
            "dns: rib reconcile ok={} fail={} elapsed={:.2}s rate={:.1}/s",
            ok,
            fail,
            elapsed,
            rate
        );
        fail
    }
}
