# gobgp-sync

GoBGP 路由同步服务 — 根据国家代码从 [user-country](https://github.com/sapics/ip-location-db) CIDR 库同步 IP 前缀到 GoBGP。

数据源为日更的 `user-country` CIDR CSV（本地缓存于 `snapshot/geo/`）。下载地址可在 `[geo]` 或 `--geo-url-ipv4` / `--geo-url-ipv6` 覆盖。`country_code` 支持 `ALL` / `NONECN` / ISO 国家码。团体字为 `community_asn:ISO3166数字码`（如 CN → `3166:156`）。

## 环境要求

- Linux x86_64 或 macOS arm64
- `gobgpd` 须与 `gobgp-sync` 同放在 `bin/`（由本进程启动管理）

## 使用方法

在包根目录运行（工作目录即包根，配置/日志/快照用相对路径）：

- `bin/gobgp-sync`、`bin/gobgpd`
- `config/config.toml`、`config/gobgpd.conf`
- `logs/`、`snapshot/`

```bash
# 在包根目录，默认会读 config/config.toml 与 config/gobgpd.conf
./bin/gobgp-sync

# 覆盖部分参数
./bin/gobgp-sync -C CN -i dual
./bin/gobgp-sync -C ALL -s 03:00 -l logs/gobgp_sync.log

# 只写 systemd/launchd 单元，ExecStart 为当前这个二进制的绝对路径
sudo ./bin/gobgp-sync install
./bin/gobgp-sync install --no-start

./bin/gobgp-sync --help
```

### 配置参数


| 参数                          | 短参数  | 说明                                           | 默认值                |
| --------------------------- | ---- | -------------------------------------------- | ------------------ |
| `--ip-version`              | `-i` | IP 协议版本: `ipv4`, `ipv6`, `dual`              | `DUAL`             |
| `--country`                 | `-C` | 国家代码，逗号分隔多个；`ALL` / `NONECN`             | `CN`               |
| `--sync-time`               | `-s` | 同步周期，默认每天 `02:00`；如 `3d 02:00`、`1w Mon 02:00` | `02:00`            |
| `--gobgpd-config`           |      | GoBGP 原生配置文件路径                              | `config/gobgpd.conf` |
| `--gobgp-api-host`          |      | GoBGP gRPC API 地址                            | `127.0.0.1`        |
| `--gobgp-api-port`          |      | GoBGP gRPC API 端口                            | `50051`            |
| `--gobgp-nexthop-ipv4`      |      | 注入 IPv4 路由时传给 GoBGP 的下一跳                     | `0.0.0.0`          |
| `--gobgp-nexthop-ipv6`      |      | 注入 IPv6 路由时传给 GoBGP 的下一跳                     | `::`               |
| `--community-nexthop-ipv4`  |      | 按国家简写覆盖 IPv4 下一跳，格式 `CN=198.19.0.254`     |                    |
| `--community-nexthop-ipv6`  |      | 按国家简写覆盖 IPv6 下一跳，格式 `CN=2001:db8::fe`     |                    |
| `--community-asn`           |      | 团体字 ASN 半部，生成 `ASN:ISO3166数字码`            | `3166`             |
| `--geo-url-ipv4`            |      | IPv4 国家 CIDR CSV 下载地址                        | user-country 默认 URL |
| `--geo-url-ipv6`            |      | IPv6 国家 CIDR CSV 下载地址                        | user-country 默认 URL |
| `--log-file`                | `-l` | 日志文件路径                                       | `logs/gobgp_sync.log` |
| `--snapshot-dir`            | `-d` | 快照文件目录                                       | `snapshot`         |
| `--concurrency`             |      | 并发添加/删除路由的任务数                                | `100`              |
| `--domains-file`            |      | DNS 域名列表文件；不存在则不启 DNS 同步                  | `config/domains.txt` |
| `--dns-interval`            |      | DNS 同步周期，如 `10m` / `30s` / `1h`                | `10m`              |
| `--dns-servers`             |      | 上游递归 DNS，逗号分隔                               | `223.5.5.5,119.29.29.29` |
| `--config`                  | `-c` | TOML 配置文件路径                                  | `config/config.toml` |


> **说明**：程序自带定时调度，首次启动立即执行一次，之后按 `--sync-time` 指定的周期自动同步，不需要额外配置 cron。

### DNS → BGP

可选能力：解析域名的 A/AAAA，将主机路由注入 GoBGP（团体字固定 `65535:65535`，下一跳用默认值）。与国家前缀同步独立运行。

- 域名列表：`[dns].domains_file`（默认 `config/domains.txt`），一行一个域名，`#` 注释
- 文件不存在：不解析、不改 DNS 路由；进程其余功能照常
- 文件存在但为空：按严格空结果撤掉此前 DNS 路由
- 每轮重读域名文件，增删域名后无需重启
- 上游 DNS：`[dns].servers`（默认 `223.5.5.5,119.29.29.29`），不用系统 stub，以拿到完整多 A 记录
- 快照：`snapshot/snapshot_dns_routing.prefix`（每行 `前缀 团体字 域名`）
- 地址族跟随 `[global].ip_version`

AS、邻居、AFI/SAFI、认证、策略等全部写在 `gobgpd.conf`（GoBGP 原生 TOML；`.yaml` / `.json` / `.hcl` 会按扩展名传给 `gobgpd -t`）。本程序启动 `gobgpd -f <gobgpd.conf> --api-hosts=<sync 中的 API 地址>`，不再通过 gRPC 下发 BGP 会话配置。

### 配置分段

TOML 使用四段：`[global]`（国家范围/周期/日志/团体字 ASN）、`[gobgp]`（注入 API/下一跳）、`[geo]`（CIDR CSV 下载地址）、`[dns]`（域名解析）。

```toml
[global]
ip_version = "dual"
country_code = "CN"
sync_time = "02:00"
community_asn = "3166"
concurrency = 100
log_file = "logs/gobgp_sync.log"
snapshot_dir = "snapshot"

[gobgp]
# config = "config/gobgpd.conf"
# api_host = "127.0.0.1"
# nexthop_ipv4 = "0.0.0.0"

[gobgp.community_nexthop_ipv4]
CN = "198.19.0.254"

[geo]
# ipv4_url = "https://github.com/sapics/ip-location-db/releases/download/latest/user-country-ipv4-cidr.csv"
# ipv6_url = "https://github.com/sapics/ip-location-db/releases/download/latest/user-country-ipv6-cidr.csv"

[dns]
# domains_file = "config/domains.txt"
# interval = "10m"
# servers = "223.5.5.5,119.29.29.29"
```

下一跳匹配优先级为：国家简写覆盖、默认下一跳。团体字格式为 `community_asn:ISO3166数字码`。快照文件存在且仍在周期内时，程序会查询 GoBGP Global RIB，只追加快照中存在但 GoBGP 中缺失的路由。
---

## 安装为系统服务

`install` 只写守护进程单元，不移动、不拷贝二进制。`ExecStart` 是执行 `install` 时那个 `gobgp-sync` 的绝对路径；`WorkingDirectory` 是它上一级包根（`…/bin/gobgp-sync` → `…`），用来读相对路径的 `config/`。

```bash
# 例如放在 /opt/gobgp 或 /var/lib/gobgp，目录结构相同即可
sudo /opt/gobgp/bin/gobgp-sync install
sudo /var/lib/gobgp/bin/gobgp-sync install
```

Linux 写入 `/etc/systemd/system/gobgp-sync.service`，macOS 写入 `~/Library/LaunchAgents/com.users.gobgp-sync.plist`。

### Linux（systemd）

```bash
sudo systemctl status gobgp-sync
sudo journalctl -u gobgp-sync -f
```

快照写在工作目录，单元里 `PrivateTmp=false`。

### macOS（launchd）

plist 为 `~/Library/LaunchAgents/com.users.gobgp-sync.plist`（权限 `644`、属主 `<user>:staff`）。日志走工作目录下 `logs/gobgp_sync.log`。

```bash
launchctl print gui/$(id -u)/com.users.gobgp-sync
```

同步参数改 `config/config.toml` 后重启服务即可，不必改 ExecStart / ProgramArguments。

---

## 构建

```bash
# macOS (arm64)
cargo build --release

# Linux x86_64 (静态链接, musl)
cargo build --target x86_64-unknown-linux-musl --release
```

### 一键打包（含 gobgpd）

```bash
# 默认按当前开发机编译 gobgp-sync，并 git 拉取最新 GoBGP 源码编译 gobgpd
make package

# 固定 GoBGP tag / 交叉编译
make package GOBGP_VERSION=v4.3.0
make package TARGET=x86_64-unknown-linux-musl
```

产物：`dist/gobgp-sync_<version>_<os>_<arch>.tar.gz`（含 `bin/`、`config/`、`logs/`、`snapshot/`；Linux 另含 systemd 单元，macOS 另含 launchd plist）。  
需本机已装 **Go** 与 **git**；`gobgpd` 一律从源码编译。默认拉最新 **GoBGP v4**（与本程序 gRPC stub 一致）。

### Docker

运行层是 `scratch` 空镜像，只含静态链接的 `gobgp-sync`、`gobgpd` 和配置（无 shell，不能 `docker exec sh`）。工作目录 `/etc/gobgp-sync`。BGP 需要宿主机网络，Linux 用 `network_mode: host`。HTTPS 走 rustls 内置根证书，不依赖镜像里的 ca-certificates。

```bash
# 构建镜像（默认跟当前 Docker 架构；Apple Silicon → linux/arm64）
make docker
make docker PLATFORM=linux/arm64
make docker PLATFORM=linux/amd64
# 或
docker build -t gobgp-sync:latest --platform linux/arm64 --build-arg GOBGP_VERSION=latest .
docker build -t gobgp-sync:latest --platform linux/amd64 --build-arg GOBGP_VERSION=v4.3.0 .

# 推荐：compose（挂载 config/，日志和快照用 named volume）
docker compose up -d
docker compose logs -f

# 或直接跑
docker run --rm --network host \
  -v "$(pwd)/config:/etc/gobgp-sync/config" \
  -v gobgp-sync-logs:/etc/gobgp-sync/logs \
  -v gobgp-sync-snapshot:/etc/gobgp-sync/snapshot \
  gobgp-sync:latest
```

Docker Desktop（macOS）的 host 网络不能真正建立 BGP 邻居，只适合在 Linux 宿主机上跑。
