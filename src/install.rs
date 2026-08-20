use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context};

use crate::config::InstallArgs;

const LABEL: &str = "com.users.gobgp-sync";
const LEGACY_LABEL: &str = "com.gobgp-sync";
const SYSTEMD_UNIT: &str = "gobgp-sync.service";
const SYSTEMD_TEMPLATE: &str = include_str!("../service/gobgp-sync.service");
const LAUNCHD_TEMPLATE: &str = include_str!("../service/com.users.gobgp-sync.plist");

struct ServicePaths {
    exe: PathBuf,
    workdir: PathBuf,
}

pub fn run(args: &InstallArgs) -> anyhow::Result<()> {
    match std::env::consts::OS {
        "linux" => install_linux(args),
        "macos" => install_macos(args),
        other => Err(anyhow!(
            "install is only supported on Linux and macOS, not {other}"
        )),
    }
}

fn install_linux(args: &InstallArgs) -> anyhow::Result<()> {
    require_root()?;
    let paths = service_paths()?;
    println!("ExecStart: {}", paths.exe.display());
    println!("WorkingDirectory: {}", paths.workdir.display());

    let unit_path = PathBuf::from("/etc/systemd/system").join(SYSTEMD_UNIT);
    write_file(
        &unit_path,
        render(&paths, SYSTEMD_TEMPLATE).as_bytes(),
        0o644,
    )?;
    println!("wrote {}", unit_path.display());

    if args.no_start {
        println!("skipped enable/start (--no-start)");
        println!("systemctl daemon-reload && systemctl enable --now gobgp-sync");
        return Ok(());
    }

    run_cmd("systemctl", &["daemon-reload"])?;
    run_cmd("systemctl", &["enable", "gobgp-sync"])?;
    run_cmd("systemctl", &["restart", "gobgp-sync"])?;
    println!("enabled and started gobgp-sync");
    Ok(())
}

fn install_macos(args: &InstallArgs) -> anyhow::Result<()> {
    let uid = current_uid()?;
    if uid == 0 {
        return Err(anyhow!(
            "install on macOS writes a user LaunchAgent; run without sudo"
        ));
    }
    let paths = service_paths()?;
    println!("Program: {}", paths.exe.display());
    println!("WorkingDirectory: {}", paths.workdir.display());

    let user = current_user()?;
    let agents = macos_launch_agents_dir()?;
    fs::create_dir_all(&agents)
        .with_context(|| format!("failed to create {}", agents.display()))?;
    let plist_path = agents.join(format!("{LABEL}.plist"));
    write_file(
        &plist_path,
        render_plist(&paths, LAUNCHD_TEMPLATE).as_bytes(),
        0o644,
    )?;
    println!("wrote {}", plist_path.display());

    let owner = format!("{user}:staff");
    let plist = plist_path.to_string_lossy();
    run_cmd("chown", &[&owner, &plist])?;

    if args.no_start {
        println!("skipped launchctl bootstrap (--no-start)");
        println!("launchctl bootstrap gui/$(id -u) {}", plist_path.display());
        return Ok(());
    }

    let domain = format!("gui/{uid}");
    // launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/xxx.plist
    // 未加载时失败可忽略；须在删除旧 plist 前先 bootout
    let legacy_plist = agents.join(format!("{LEGACY_LABEL}.plist"));
    if legacy_plist.exists() {
        silent_bootout_plist(&domain, &legacy_plist);
        let _ = fs::remove_file(&legacy_plist);
        println!("removed legacy {}", legacy_plist.display());
    }
    silent_bootout_plist(&domain, &plist_path);
    run_cmd("launchctl", &["bootstrap", &domain, &plist])?;
    println!("bootstrapped {domain}/{LABEL}");
    Ok(())
}

fn silent_bootout_plist(domain: &str, plist: &Path) {
    let path = plist.to_string_lossy();
    let _ = Command::new("launchctl")
        .args(["bootout", domain, path.as_ref()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

// 二进制在哪，单元就写哪；工作目录取 bin/ 的上一级（包根）
fn service_paths() -> anyhow::Result<ServicePaths> {
    let exe = std::env::current_exe()
        .context("failed to resolve current executable")?
        .canonicalize()
        .context("failed to resolve current executable")?;
    let bin_dir = exe
        .parent()
        .ok_or_else(|| anyhow!("cannot resolve executable directory"))?;
    let workdir = if bin_dir.file_name().is_some_and(|n| n == "bin") {
        bin_dir
            .parent()
            .ok_or_else(|| anyhow!("cannot resolve working directory"))?
            .to_path_buf()
    } else {
        bin_dir.to_path_buf()
    };
    Ok(ServicePaths { exe, workdir })
}

fn render(paths: &ServicePaths, template: &str) -> String {
    template
        .replace("__EXE__", &paths.exe.to_string_lossy())
        .replace("__PREFIX__", &paths.workdir.to_string_lossy())
}

fn render_plist(paths: &ServicePaths, template: &str) -> String {
    template
        .replace("__EXE__", &xml_escape(&paths.exe.to_string_lossy()))
        .replace("__PREFIX__", &xml_escape(&paths.workdir.to_string_lossy()))
}

fn write_file(path: &Path, contents: &[u8], mode: u32) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)
            .with_context(|| format!("failed to stat {}", path.display()))?
            .permissions();
        perms.set_mode(mode);
        fs::set_permissions(path, perms)
            .with_context(|| format!("failed to chmod {}", path.display()))?;
    }
    Ok(())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn macos_launch_agents_dir() -> anyhow::Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join("Library").join("LaunchAgents"))
}

fn current_user() -> anyhow::Result<String> {
    if let Ok(user) = std::env::var("USER") {
        if !user.is_empty() {
            return Ok(user);
        }
    }
    command_stdout("id", &["-un"])
}

fn current_uid() -> anyhow::Result<u32> {
    command_stdout("id", &["-u"])?
        .parse()
        .context("failed to parse uid")
}

fn require_root() -> anyhow::Result<()> {
    let uid = current_uid()?;
    if uid != 0 {
        return Err(anyhow!(
            "install on Linux requires root (sudo ./bin/gobgp-sync install)"
        ));
    }
    Ok(())
}

fn command_stdout(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "{program} {} failed: {}",
            args.join(" "),
            output.status
        ));
    }
    Ok(String::from_utf8(output.stdout)
        .context("command output is not utf-8")?
        .trim()
        .to_string())
}

fn run_cmd(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to run {program}"))?;
    if !status.success() {
        return Err(anyhow!("{program} {} failed: {status}", args.join(" ")));
    }
    Ok(())
}
