use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::config::Settings;
use crate::models::geo::PrefixExtractor;
use crate::utils::command::{CommandExecutor, RouteEntry};

// 路由管理器
pub struct RouteManager {
    settings: Settings,
}

impl RouteManager {
    pub fn new(settings: Settings) -> Self {
        Self { settings }
    }

    /// Snapshot/desired entries missing from GoBGP Global RIB.
    pub fn missing_from_rib(
        desired: &HashMap<String, String>,
        rib: &HashSet<String>,
    ) -> HashMap<String, String> {
        desired
            .iter()
            .filter(|(prefix, _)| !rib.contains(*prefix))
            .map(|(prefix, community)| (prefix.clone(), community.clone()))
            .collect()
    }

    // 先删后加，返回 (成功数, 失败数, 耗时秒, 平均速率)
    pub async fn batch_sync(
        &self,
        to_add: &HashMap<String, String>,
        to_del: &HashMap<String, String>,
        tag: &str,
    ) -> (u32, u32, f64, f64) {
        let started = std::time::Instant::now();
        let mut ok = 0u32;
        let mut fail = 0u32;

        if !to_del.is_empty() {
            let entries: Vec<RouteEntry> = to_del
                .iter()
                .map(|(prefix, community)| RouteEntry {
                    prefix: prefix.clone(),
                    community: community.clone(),
                })
                .collect();
            let result = CommandExecutor::del_routes(
                &entries,
                &self.settings,
                tag,
                self.settings.concurrency,
            )
            .await;
            ok += result.ok;
            fail += result.fail;
        }

        if !to_add.is_empty() {
            let entries: Vec<RouteEntry> = to_add
                .iter()
                .map(|(prefix, community)| RouteEntry {
                    prefix: prefix.clone(),
                    community: community.clone(),
                })
                .collect();
            let result = CommandExecutor::add_routes(
                &entries,
                &self.settings,
                tag,
                self.settings.concurrency,
            )
            .await;
            ok += result.ok;
            fail += result.fail;
        }

        let elapsed = started.elapsed().as_secs_f64();
        let total = (ok + fail) as f64;
        let rate = if elapsed > 0.0 { total / elapsed } else { 0.0 };
        (ok, fail, elapsed, rate)
    }

    pub async fn list_global_prefixes(
        &self,
        protocol: &str,
        tag: &str,
    ) -> anyhow::Result<HashSet<String>> {
        CommandExecutor::list_global_prefixes(&self.settings, protocol == "ipv6", tag).await
    }

    // 每行 "前缀 团体字" 或仅 "前缀"
    pub fn load_snapshot(snapshot_file: &str) -> HashMap<String, String> {
        let path = Path::new(snapshot_file);
        if !path.exists() {
            return HashMap::new();
        }

        match fs::read_to_string(path) {
            Ok(content) => content
                .lines()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .filter_map(|line| {
                    // 支持两种格式：
                    // "前缀 团体字" 或 "前缀"（旧格式兼容）
                    let parts: Vec<&str> = line.splitn(2, ' ').collect();
                    let prefix = parts[0].trim().to_string();
                    if !PrefixExtractor::is_valid_cidr(&prefix) {
                        return None;
                    }
                    let community = parts.get(1).unwrap_or(&"").trim().to_string();
                    Some((prefix, community))
                })
                .collect(),
            Err(e) => {
                log::error!("failed to load snapshot: {}", e);
                HashMap::new()
            }
        }
    }

    // 每行 "前缀 团体字"
    pub fn save_snapshot(
        &self,
        entries: &HashMap<String, String>,
        snapshot_file: &str,
    ) -> anyhow::Result<()> {
        // 确保父目录存在
        if let Some(parent) = Path::new(snapshot_file).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let mut sorted: Vec<(&String, &String)> = entries.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(b.0));
        let content = sorted
            .into_iter()
            .map(|(prefix, community)| {
                if community.is_empty() {
                    prefix.clone()
                } else {
                    format!("{} {}", prefix, community)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(snapshot_file, content)?;
        log::debug!("snapshot saved: {}", snapshot_file);
        Ok(())
    }
}
