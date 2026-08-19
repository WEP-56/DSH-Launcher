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
    #[cfg(windows)]
    use super::*;
    #[cfg(windows)]
    use std::path::Path;

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
}
