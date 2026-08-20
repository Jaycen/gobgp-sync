use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tonic::transport::Channel;

use crate::config::Settings;
use crate::gobgp::apipb;
use apipb::gobgp_api_client::GobgpApiClient;

// 并发执行 GoBGP API 的返回结果
pub struct ConcurrencyResult {
    pub ok: u32,
    pub fail: u32,
}

// 带团体字的路由条目
pub struct RouteEntry {
    pub prefix: String,
    pub community: String,
}

// GoBGP API 执行器
pub struct CommandExecutor;

impl CommandExecutor {
    // 并发添加路由，下一跳按团体字国家码匹配
    pub async fn add_routes(
        entries: &[RouteEntry],
        settings: &Settings,
        tag: &str,
        concurrency: usize,
    ) -> ConcurrencyResult {
        let total = entries.len();
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let mut handles: Vec<JoinHandle<bool>> = Vec::with_capacity(total);
        let client = match Self::connect(settings, tag).await {
            Some(client) => client,
            None => {
                return ConcurrencyResult {
                    ok: 0,
                    fail: total as u32,
                }
            }
        };

        let mut acquire_fail = 0u32;
        for entry in entries {
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(e) => {
                    log::error!("{}: failed to acquire add permit: {}", tag, e);
                    acquire_fail += 1;
                    continue;
                }
            };
            let p = entry.prefix.clone();
            let c = entry.community.clone();
            let next_hop = settings.next_hop_for_community(&c, p.contains(':'));
            let mut route_client = client.clone();
            let t = tag.to_string();

            handles.push(tokio::spawn(async move {
                let result = Self::add_route(&mut route_client, &p, &c, &next_hop).await;

                if result {
                    log::debug!("{}: added {} ({})", t, p, c);
                } else {
                    log::warn!("{}: add failed: {} ({})", t, p, c);
                }
                drop(permit);
                result
            }));
        }

        let mut ok = 0u32;
        let mut fail = acquire_fail;

        for handle in handles.into_iter() {
            match handle.await {
                Ok(true) => ok += 1,
                Ok(false) => fail += 1,
                Err(e) => {
                    log::error!("{}: task panicked: {}", tag, e);
                    fail += 1;
                }
            }
        }

        ConcurrencyResult { ok, fail }
    }

    // 并发删除路由，不携带团体字以免属性匹配过严
    pub async fn del_routes(
        entries: &[RouteEntry],
        settings: &Settings,
        tag: &str,
        concurrency: usize,
    ) -> ConcurrencyResult {
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let total = entries.len();
        let mut handles: Vec<JoinHandle<bool>> = Vec::with_capacity(total);
        let client = match Self::connect(settings, tag).await {
            Some(client) => client,
            None => {
                return ConcurrencyResult {
                    ok: 0,
                    fail: total as u32,
                }
            }
        };

        let mut acquire_fail = 0u32;
        for entry in entries {
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(e) => {
                    log::error!("{}: failed to acquire delete permit: {}", tag, e);
                    acquire_fail += 1;
                    continue;
                }
            };
            let p = entry.prefix.clone();
            let c = entry.community.clone();
            let next_hop = settings.next_hop_for_community(&c, p.contains(':'));
            let mut route_client = client.clone();
            let t = tag.to_string();

            handles.push(tokio::spawn(async move {
                let result = Self::delete_route(&mut route_client, &p, &next_hop).await;

                if result {
                    log::debug!("{}: deleted {} ({})", t, p, c);
                } else {
                    log::warn!("{}: delete failed: {} ({})", t, p, c);
                }
                drop(permit);
                result
            }));
        }

        let mut ok = 0u32;
        let mut fail = acquire_fail;

        for handle in handles.into_iter() {
            match handle.await {
                Ok(true) => ok += 1,
                Ok(false) => fail += 1,
                Err(e) => {
                    log::error!("{}: task panicked: {}", tag, e);
                    fail += 1;
                }
            }
        }

        ConcurrencyResult { ok, fail }
    }

    // 连接失败时由调用方把整批任务记为失败
    async fn connect(settings: &Settings, tag: &str) -> Option<GobgpApiClient<Channel>> {
        match GobgpApiClient::connect(settings.gobgp_api_addr()).await {
            Ok(client) => Some(client),
            Err(e) => {
                log::error!("{}: failed to connect gobgp api: {}", tag, e);
                None
            }
        }
    }

    async fn add_route(
        client: &mut GobgpApiClient<Channel>,
        prefix: &str,
        community: &str,
        next_hop: &str,
    ) -> bool {
        let path = match Self::build_path(prefix, community, next_hop) {
            Ok(path) => path,
            Err(e) => {
                log::error!("failed to build add path: {} - {}", prefix, e);
                return false;
            }
        };

        let request = apipb::AddPathRequest {
            table_type: apipb::TableType::Global as i32,
            vrf_id: String::new(),
            path: Some(path),
        };

        match client.add_path(request).await {
            Ok(_) => true,
            Err(e) => {
                log::error!("add path failed: {} - {}", prefix, e);
                false
            }
        }
    }

    // uuid 留空，靠 prefix/family/next-hop 删除，不携带团体字
    async fn delete_route(
        client: &mut GobgpApiClient<Channel>,
        prefix: &str,
        next_hop: &str,
    ) -> bool {
        let path = match Self::build_path(prefix, "", next_hop) {
            Ok(path) => path,
            Err(e) => {
                log::error!("failed to build delete path: {} - {}", prefix, e);
                return false;
            }
        };

        let request = apipb::DeletePathRequest {
            table_type: apipb::TableType::Global as i32,
            vrf_id: String::new(),
            family: path.family.clone(),
            path: Some(path),
            uuid: Vec::new(),
        };

        match client.delete_path(request).await {
            Ok(_) => true,
            Err(e) => {
                log::error!("delete path failed: {} - {}", prefix, e);
                false
            }
        }
    }

    pub async fn list_global_prefixes(
        settings: &Settings,
        is_ipv6: bool,
        tag: &str,
    ) -> anyhow::Result<HashSet<String>> {
        let mut client = Self::connect(settings, tag)
            .await
            .ok_or_else(|| anyhow::anyhow!("failed to connect gobgp api"))?;

        let family = apipb::Family {
            afi: if is_ipv6 {
                apipb::family::Afi::Ip6 as i32
            } else {
                apipb::family::Afi::Ip as i32
            },
            safi: apipb::family::Safi::Unicast as i32,
        };

        let request = apipb::ListPathRequest {
            table_type: apipb::TableType::Global as i32,
            name: String::new(),
            family: Some(family),
            prefixes: Vec::new(),
            sort_type: apipb::list_path_request::SortType::Prefix as i32,
            enable_filtered: false,
            enable_nlri_binary: false,
            enable_attribute_binary: false,
            enable_only_binary: false,
        };

        let mut stream = client.list_path(request).await?.into_inner();
        let mut prefixes = HashSet::new();

        while let Some(response) = stream.message().await? {
            if let Some(destination) = response.destination {
                if !destination.prefix.is_empty() && !destination.paths.is_empty() {
                    prefixes.insert(destination.prefix);
                }
            }
        }

        Ok(prefixes)
    }

    fn build_path(prefix: &str, community: &str, next_hop: &str) -> anyhow::Result<apipb::Path> {
        let (ip, prefix_len) = Self::parse_cidr(prefix)?;
        let is_ipv6 = matches!(ip, IpAddr::V6(_));
        let afi = if is_ipv6 {
            apipb::family::Afi::Ip6
        } else {
            apipb::family::Afi::Ip
        };

        let family = apipb::Family {
            afi: afi as i32,
            safi: apipb::family::Safi::Unicast as i32,
        };

        let mut pattrs = vec![apipb::Attribute {
            attr: Some(apipb::attribute::Attr::Origin(apipb::OriginAttribute {
                origin: 0,
            })),
        }];

        pattrs.push(apipb::Attribute {
            attr: Some(apipb::attribute::Attr::NextHop(apipb::NextHopAttribute {
                next_hop: next_hop.to_string(),
            })),
        });

        if !community.trim().is_empty() {
            pattrs.push(apipb::Attribute {
                attr: Some(apipb::attribute::Attr::Communities(
                    apipb::CommunitiesAttribute {
                        communities: vec![Self::community_to_u32(community)?],
                    },
                )),
            });
        }

        Ok(apipb::Path {
            nlri: Some(apipb::Nlri {
                nlri: Some(apipb::nlri::Payload::Prefix(apipb::IpAddressPrefix {
                    prefix_len,
                    prefix: ip.to_string(),
                })),
            }),
            pattrs,
            is_withdraw: false,
            no_implicit_withdraw: false,
            family: Some(family),
        })
    }

    // 将 ASN:VALUE 转为 32-bit 整数
    fn community_to_u32(community: &str) -> anyhow::Result<u32> {
        let (asn, value) = community
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("community must be <asn>:<value>"))?;
        let asn: u32 = asn.parse()?;
        let value: u32 = value.parse()?;

        if asn > u16::MAX as u32 || value > u16::MAX as u32 {
            anyhow::bail!("community fields must be in 0..=65535");
        }

        Ok((asn << 16) | value)
    }

    // 解析 CIDR
    fn parse_cidr(prefix: &str) -> anyhow::Result<(IpAddr, u32)> {
        let (ip, prefix_len) = prefix
            .split_once('/')
            .ok_or_else(|| anyhow::anyhow!("prefix must be CIDR"))?;
        let ip: IpAddr = ip.parse()?;
        let prefix_len: u32 = prefix_len.parse()?;

        let max_len = match ip {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };

        if prefix_len > max_len {
            anyhow::bail!("prefix length {} exceeds max {}", prefix_len, max_len);
        }

        Ok((ip, prefix_len))
    }
}
