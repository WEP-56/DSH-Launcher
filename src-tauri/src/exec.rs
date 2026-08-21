use std::{
    io::Read,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::config::{LaunchMode, LauncherConfig};

/// 查询类命令（dsh --version、npm view/search）。npx 首次运行可能要现场下载包。
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(120);
/// 单个插件安装/卸载（网络差时 npm 安装可能很慢）。
pub const PLUGIN_ACTION_TIMEOUT: Duration = Duration::from_secs(300);
/// npm install -g 全量更新 dsh。
pub const GLOBAL_INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, serde::Serialize)]
pub struct OperationResult {
    pub success: bool,
    pub output: String,
}

#[cfg(windows)]
pub fn hide_console(command: &mut Command) {
    // GUI 进程派生控制台子进程（npm/taskkill/dsh 等）时，缺少该标志会在
    // release 版弹出黑色控制台窗口。
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn hide_console(_command: &mut Command) {}

#[cfg(target_os = "macos")]
pub fn adopt_login_shell_path() {
    // macOS 的 GUI 进程从 launchd 继承极简 PATH（不含 Homebrew/nvm 等目录），
    // 直接找 node/npm/dsh 会失败；用登录 shell 输出的 PATH 覆盖当前进程。
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    if let Ok(output) = Command::new(shell)
        .args(["-lc", "printf %s \"$PATH\""])
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout);
            let trimmed = path.trim();
            if !trimmed.is_empty() {
                std::env::set_var("PATH", trimmed);
            }
        }
    }

    // 登录 shell 用 `-l` 启动时只读 .zprofile；nvm/pnpm 等通常把 PATH 写在
    // .zshrc（交互 shell 才读），因此上面的探测拿不到这些目录。再按已知的
    // node/npm 生态安装位置补齐，保证 GUI 进程能找到 `npm install -g
    // @deepseek-ai/dsh` 生成的 dsh、以及 npx/npm 本身。
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        let merged = merge_existing_path_dirs(
            known_path_dirs(&home),
            &std::env::var_os("PATH").unwrap_or_default(),
        );
        std::env::set_var("PATH", merged);
    }
}

/// 收集 macOS 上 node/npm 生态常见的可执行目录（含 nvm 各版本 bin），
/// 越新的 nvm 版本越靠前。只返回候选目录，是否真实存在由合并方过滤。
#[cfg(target_os = "macos")]
fn known_path_dirs(home: &std::path::Path) -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut dirs = Vec::new();
    let home = home.to_path_buf();
    dirs.push(home.join(".npm-global").join("bin"));
    // npm prefix 直接指向 $HOME 时，全局命令在 $HOME/node_modules/.bin。
    dirs.push(home.join("node_modules").join(".bin"));
    if let Ok(entries) = std::fs::read_dir(home.join(".nvm").join("versions").join("node")) {
        let mut versions: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        versions.sort_by(|a, b| version_key(a).cmp(&version_key(b)).reverse());
        for version in versions {
            let bin = version.join("bin");
            if bin.is_dir() {
                dirs.push(bin);
            }
        }
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/opt/homebrew/opt/node@24/bin"));
    dirs.push(PathBuf::from("/opt/homebrew/opt/node@22/bin"));
    dirs.push(PathBuf::from("/opt/homebrew/opt/node@20/bin"));
    dirs.push(PathBuf::from("/opt/homebrew/opt/node@18/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/opt/local/bin")); // MacPorts
    dirs
}

/// 解析 nvm 版本目录名（去掉前导 v）为可比较的 (major, minor, patch)。
#[cfg(target_os = "macos")]
fn version_key(path: &std::path::Path) -> (u64, u64, u64) {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .trim_start_matches('v');
    let mut parts = name.split('.');
    let parse = |part: Option<&str>| part.and_then(|value| value.parse().ok()).unwrap_or(0);
    (
        parse(parts.next()),
        parse(parts.next()),
        parse(parts.next()),
    )
}

/// 把存在且尚未出现的候选目录追加到当前 PATH，返回合并结果（不覆盖原有值）。
#[cfg(target_os = "macos")]
fn merge_existing_path_dirs(
    dirs: Vec<std::path::PathBuf>,
    current: &std::ffi::OsStr,
) -> std::ffi::OsString {
    use std::env::{join_paths, split_paths};
    use std::path::PathBuf;

    let mut parts: Vec<PathBuf> = split_paths(current).collect();
    for dir in dirs {
        if dir.is_dir() && !parts.contains(&dir) {
            parts.push(dir);
        }
    }
    join_paths(&parts).unwrap_or_else(|_| current.to_os_string())
}

#[cfg(windows)]
pub fn resolve_windows_executable(raw: &str) -> Result<String, String> {
    use std::path::{Path, PathBuf};

    let supplied = PathBuf::from(raw);
    if supplied.is_file() {
        return supplied
            .canonicalize()
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| format!("无法解析 CLI 路径 {raw}：{error}"));
    }

    let has_path_separator = raw.contains(['\\', '/']);
    let extension = supplied.extension().and_then(|value| value.to_str());
    let candidates = if extension.is_some() {
        vec![raw.to_string()]
    } else if has_path_separator {
        vec![format!("{raw}.cmd"), format!("{raw}.exe"), raw.to_string()]
    } else {
        // npm creates both POSIX and Windows shims. Resolve the .cmd shim
        // explicitly so its %~dp0 points at the global npm directory.
        vec![format!("{raw}.cmd"), format!("{raw}.exe")]
    };

    for candidate in &candidates {
        if has_path_separator && Path::new(candidate).is_file() {
            return Ok(candidate.clone());
        }
        let mut where_command = Command::new("where.exe");
        hide_console(&mut where_command);
        let output = where_command.arg(candidate).output();
        if let Ok(output) = output {
            if output.status.success() {
                if let Some(path) = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                {
                    return Ok(path.to_string());
                }
            }
        }
    }

    // GUI 进程的 PATH 可能缺少 npm 全局目录（用户改过 PATH、装完 Node 没有重新
    // 登录等），where.exe 找不到时再探测几个常见的全局安装位置兜底。
    if !has_path_separator {
        for dir in known_global_bin_dirs() {
            for candidate in &candidates {
                let path = dir.join(candidate);
                if path.is_file() {
                    return Ok(path.to_string_lossy().into_owned());
                }
            }
        }
    }

    Err(format!(
        "找不到命令 {raw}。请确认它能在普通终端中运行，或填写 .cmd/.exe 的完整路径。"
    ))
}

#[cfg(windows)]
fn known_global_bin_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut result = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        result.push(PathBuf::from(appdata).join("npm"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        result.push(local.join("pnpm"));
        result.push(local.join("Volta").join("bin"));
    }
    if let Some(programs) = std::env::var_os("ProgramFiles") {
        result.push(PathBuf::from(programs).join("nodejs"));
    }
    result
}

pub fn command_for_invocation(config: &LauncherConfig, args: &[String]) -> Result<Command, String> {
    #[cfg(windows)]
    let mut command = match config.launch_mode {
        LaunchMode::Command => Command::new(resolve_windows_executable(config.executable.trim())?),
        LaunchMode::Npx => Command::new(resolve_windows_executable("npx")?),
    };
    #[cfg(not(windows))]
    let mut command = match config.launch_mode {
        LaunchMode::Command => Command::new(config.executable.trim()),
        LaunchMode::Npx => Command::new("npx"),
    };
    if matches!(config.launch_mode, LaunchMode::Npx) {
        command.args(["--yes", &config.npx_package]);
    }
    command.args(args);
    hide_console(&mut command);
    Ok(command)
}

pub fn prepare_command(mut command: Command, config: &LauncherConfig) -> Command {
    command.current_dir(&config.working_directory);
    if !config.dsh_home.trim().is_empty() {
        command.env("DSH_HOME", config.dsh_home.trim());
    }
    command
}

fn drain_stream<R>(stream: Option<R>) -> thread::JoinHandle<String>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = Vec::new();
        if let Some(mut stream) = stream {
            let _ = stream.read_to_end(&mut buffer);
        }
        String::from_utf8_lossy(&buffer).into_owned()
    })
}

/// 运行命令并收集输出。超时后终止整棵进程树并返回失败结果，
/// 避免挂死的 npm/dsh 把调用方（以及被暂停的 Web 服务）永远卡住。
pub fn execute_command(mut command: Command, timeout: Duration) -> Result<OperationResult, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = drain_stream(child.stdout.take());
    let stderr = drain_stream(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    break None;
                }
                thread::sleep(Duration::from_millis(120));
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    let timed_out = status.is_none();
    if timed_out {
        kill_child_tree(child);
    }
    // 进程结束（或被终止）后管道关闭，读取线程随之结束。
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    let mut combined = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if timed_out {
        let notice = format!("命令超过 {} 秒未结束，已强制终止。", timeout.as_secs());
        combined = if combined.is_empty() {
            notice
        } else {
            format!("{notice}\n{combined}")
        };
    }
    Ok(OperationResult {
        success: status.is_some_and(|status| status.success()),
        output: combined,
    })
}

pub fn run_dsh_invocation(
    config: &LauncherConfig,
    args: &[String],
    timeout: Duration,
) -> Result<OperationResult, String> {
    execute_command(
        prepare_command(command_for_invocation(config, args)?, config),
        timeout,
    )
}

pub fn npm_command() -> Command {
    #[cfg(windows)]
    let mut command =
        Command::new(resolve_windows_executable("npm").unwrap_or_else(|_| "npm.cmd".into()));
    #[cfg(not(windows))]
    let mut command = Command::new("npm");
    hide_console(&mut command);
    command
}

pub fn kill_child_tree(mut child: Child) {
    let pid = child.id();
    #[cfg(windows)]
    {
        let mut taskkill = Command::new("taskkill");
        hide_console(&mut taskkill);
        let _ = taskkill
            .args(["/PID", &pid.to_string(), "/T"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // Give dsh and any descendants a short chance to tear down their own
        // windows before using the forceful fallback.
        thread::sleep(Duration::from_millis(180));
        if child.try_wait().ok().flatten().is_none() {
            let mut force_kill = Command::new("taskkill");
            hide_console(&mut force_kill);
            let _ = force_kill
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    #[cfg(unix)]
    {
        // dsh 以独立进程组启动（见 service::start_process），负 pid 对整组发信号，
        // 保证 npx → node → dsh 的整棵进程树一起结束。
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    #[cfg(any(windows, target_os = "macos"))]
    use super::*;
    #[cfg(windows)]
    use std::path::Path;
    #[cfg(target_os = "macos")]
    use std::{ffi::OsStr, fs, path::PathBuf};

    #[cfg(windows)]
    #[test]
    fn resolves_windows_node_shims_to_absolute_cmd_paths() {
        let path = resolve_windows_executable("npx").expect("npx.cmd should be on PATH");
        assert!(Path::new(&path).is_absolute());
        assert!(path.to_ascii_lowercase().ends_with("npx.cmd"));
    }

    #[cfg(windows)]
    #[test]
    fn executes_resolved_windows_cmd_shims_directly() {
        let path = resolve_windows_executable("npx").expect("npx.cmd should be on PATH");
        let output = Command::new(path)
            .arg("--version")
            .output()
            .expect("Rust should execute an absolute .cmd shim");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn terminates_commands_that_exceed_their_timeout() {
        let mut command = Command::new("ping");
        command.args(["-n", "30", "127.0.0.1"]);
        let started = Instant::now();
        let result = execute_command(command, Duration::from_secs(1)).expect("spawn ping");
        assert!(!result.success);
        assert!(result.output.contains("已强制终止"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn collects_nvm_bins_under_home_newest_first() {
        let base = std::env::temp_dir().join(format!("dsh-home-{}", std::process::id()));
        let older = base
            .join(".nvm")
            .join("versions")
            .join("node")
            .join("v22.0.0")
            .join("bin");
        let newer = base
            .join(".nvm")
            .join("versions")
            .join("node")
            .join("v24.1.0")
            .join("bin");
        let home_bin = base.join("node_modules").join(".bin");
        fs::create_dir_all(&older).expect("create fake older nvm bin");
        fs::create_dir_all(&newer).expect("create fake newer nvm bin");
        fs::create_dir_all(&home_bin).expect("create fake home bin");
        let dirs = known_path_dirs(&base);
        let idx_older = dirs
            .iter()
            .position(|dir| dir == &older)
            .expect("older nvm bin should be listed");
        let idx_newer = dirs
            .iter()
            .position(|dir| dir == &newer)
            .expect("newer nvm bin should be listed");
        assert!(
            idx_newer < idx_older,
            "newest nvm version should come first"
        );
        assert!(
            dirs.contains(&home_bin),
            "npm prefix at HOME should be listed"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn merges_existing_dirs_without_duplicating_current_path() {
        let base = std::env::temp_dir().join(format!("dsh-path-{}", std::process::id()));
        let a = base.join("a");
        let b = base.join("b");
        let missing = base.join("missing");
        fs::create_dir_all(&a).expect("create dir a");
        fs::create_dir_all(&b).expect("create dir b");
        let current = std::env::join_paths([&a, &b]).expect("join current path");
        // 已存在的 a、b 不再追加；不存在的目录被过滤。
        let merged =
            merge_existing_path_dirs(vec![a.clone(), b.clone(), missing.clone()], &current);
        assert_eq!(merged, current, "existing dirs must not be duplicated");
        // 新出现且存在的目录被追加。
        let c = base.join("c");
        fs::create_dir_all(&c).expect("create dir c");
        let merged = merge_existing_path_dirs(vec![a, c.clone()], &current);
        let parts: Vec<PathBuf> = std::env::split_paths(&merged).collect();
        assert!(parts.contains(&c), "new dir should be appended");
        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "host-specific: asserts the real login machine layout"]
    fn host_path_covers_dsh_and_node() {
        // 把附加的候选目录并入 PATH 后，dsh（Node shim）与 node 都必须在 PATH
        // 里可解析，否则 GUI 进程直接 spawn("dsh") 会 ENOENT。
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let merged = merge_existing_path_dirs(
                known_path_dirs(&home),
                // 模拟 launchd 给 GUI 进程的极简 PATH（进程现场实测值）。
                OsStr::new("/usr/bin:/bin:/usr/sbin:/sbin"),
            );
            std::env::set_var("PATH", &merged);
            for name in ["dsh", "node"] {
                let output = Command::new("sh")
                    .args(["-c", &format!("command -v {name}")])
                    .output()
                    .expect("sh should run");
                assert!(
                    output.status.success(),
                    "{name} not resolvable via merged PATH: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                eprintln!(
                    "resolved {name}: {}",
                    String::from_utf8_lossy(&output.stdout).trim()
                );
            }
        }
    }
}
