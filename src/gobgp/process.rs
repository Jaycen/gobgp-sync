use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use tokio::process::{Child, Command};

use super::apipb::gobgp_api_client::GobgpApiClient;
use crate::config::Settings;

// 仅查找可执行文件同目录的 gobgpd，不查 PATH
pub fn resolve_gobgpd_next_to(exe: &Path) -> anyhow::Result<PathBuf> {
    let parent = exe
        .parent()
        .ok_or_else(|| anyhow!("cannot resolve executable directory: {}", exe.display()))?;
    let path = parent.join("gobgpd");
    if !path.is_file() {
        return Err(anyhow!(
            "gobgpd not found next to executable: {} (PATH is not used)",
            path.display()
        ));
    }
    Ok(path)
}

// 由本程序托管的 gobgpd 子进程
pub struct GobgpProcess {
    child: Child,
}

impl GobgpProcess {
    // 启动同目录 gobgpd，加载独立配置，并用 --api-hosts 对齐 sync 的 gRPC 地址
    pub async fn start(settings: &Settings) -> anyhow::Result<Self> {
        let exe = std::env::current_exe().context("failed to resolve current executable")?;
        let path = resolve_gobgpd_next_to(&exe)?;
        let args = gobgpd_args(
            Path::new(&settings.gobgpd_config),
            &settings.gobgp_api_host,
            settings.gobgp_api_port,
        );
        let mut cmd = Command::new(&path);
        cmd.args(&args).kill_on_drop(true);
        // 独立进程组，避免终端 Ctrl+C 先打到 gobgpd，导致父进程误判为异常退出
        #[cfg(unix)]
        cmd.process_group(0);
        let child = cmd
            .spawn()
            .with_context(|| format!("failed to start gobgpd: {}", path.display()))?;
        log::info!("started gobgpd: {} {}", path.display(), args.join(" "));
        Ok(Self { child })
    }

    // 轮询 gRPC 直至可连接，最长 10 秒
    pub async fn wait_api_ready(settings: &Settings) -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if GobgpApiClient::connect(settings.gobgp_api_addr())
                .await
                .is_ok()
            {
                log::info!("gobgpd api ready: {}", settings.gobgp_api_addr());
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "gobgpd api not ready within 10s: {}",
                    settings.gobgp_api_addr()
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // 先 SIGTERM，超时后 SIGKILL
    pub async fn stop(mut self) -> anyhow::Result<()> {
        if let Some(pid) = self.child.id() {
            let _ = StdCommand::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status();
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await;
        let _ = self.child.kill().await;
        Ok(())
    }

    // 阻塞等待 gobgpd 异常退出，随后结束本进程
    pub async fn wait_unexpected_exit(&mut self) {
        match self.child.wait().await {
            Ok(status) => log::error!("gobgpd exited: {}", status),
            Err(e) => log::error!("failed to wait for gobgpd: {}", e),
        }
        std::process::exit(1);
    }
}

// 组装 gobgpd 启动参数
pub fn gobgpd_args(config_path: &Path, api_host: &str, api_port: u16) -> Vec<String> {
    let mut args = vec!["-f".to_string(), config_path.to_string_lossy().into_owned()];
    if let Some(config_type) = config_type_from_path(config_path) {
        args.push("-t".to_string());
        args.push(config_type.to_string());
    }
    args.push(format!("--api-hosts={api_host}:{api_port}"));
    args
}

// 按扩展名推断 -t，toml/.conf/无扩展名走默认 toml
fn config_type_from_path(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("yaml") | Some("yml") => Some("yaml"),
        Some("json") => Some("json"),
        Some("hcl") => Some("hcl"),
        _ => None,
    }
}
