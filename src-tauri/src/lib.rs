use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{BufRead, BufReader, Read},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    webview::DownloadEvent,
    AppHandle, Emitter, Manager, State, TitleBarStyle, WebviewUrl, WebviewWindowBuilder,
};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult};

const MAX_LOG_LINES: usize = 400;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LaunchMode {
    Command,
    Npx,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CloseBehavior {
    Tray,
    Exit,
}

impl Default for CloseBehavior {
    fn default() -> Self {
        Self::Tray
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct LauncherConfig {
    launch_mode: LaunchMode,
    executable: String,
    npx_package: String,
    working_directory: String,
    dsh_home: String,
    port: u16,
    trusted_hosts: Vec<String>,
    auto_start: bool,
    open_on_ready: bool,
    close_behavior: CloseBehavior,
    stop_dsh_on_exit: bool,
    download_directory: String,
    download_ask: bool,
    download_choose_location: bool,
    auto_check_updates: bool,
    window_width: u32,
    window_height: u32,
}

impl Default for LauncherConfig {
    fn default() -> Self {
        let working_directory = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .to_string_lossy()
            .into_owned();
        let download_directory = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .to_string_lossy()
            .into_owned();
        Self {
            launch_mode: LaunchMode::Command,
            executable: "dsh".into(),
            npx_package: "@deepseek-ai/dsh".into(),
            working_directory,
            dsh_home: String::new(),
            port: 3080,
            trusted_hosts: Vec::new(),
            auto_start: true,
            open_on_ready: true,
            close_behavior: CloseBehavior::Tray,
            stop_dsh_on_exit: true,
            download_directory,
            download_ask: false,
            download_choose_location: false,
            auto_check_updates: true,
            window_width: 880,
            window_height: 760,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Stopped,
    Starting,
    Ready,
    Stopping,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
struct LauncherStatus {
    phase: Phase,
    message: String,
    url: String,
    pid: Option<u32>,
    external: bool,
    logs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct PackageInfo {
    current_version: String,
    latest_version: String,
    source: String,
    checked_at: String,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigFileInfo {
    id: String,
    name: String,
    path: String,
    content: String,
    editable: bool,
}

#[derive(Debug, Clone, Serialize)]
struct InstalledPlugin {
    name: String,
    version: String,
    bundle: bool,
}

#[derive(Debug, Clone, Serialize)]
struct PluginSearchResult {
    name: String,
    version: String,
    description: String,
    homepage: String,
    npm_url: String,
    keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct OperationResult {
    success: bool,
    output: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketOwnerRaw {
    #[serde(default)]
    avatar_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketRepoRaw {
    #[serde(default)]
    name: String,
    #[serde(default)]
    full_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    owner: Option<MarketOwnerRaw>,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    stars: u64,
    #[serde(default)]
    pushed_at: Option<String>,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    project_type: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    validation: Option<MarketValidationRaw>,
}

#[derive(Debug, Clone, Serialize)]
struct ReleaseInfo {
    current_version: String,
    latest_version: String,
    tag_name: String,
    name: String,
    body: String,
    html_url: String,
    published_at: String,
    update_available: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct MarketValidationRaw {
    #[serde(default)]
    overall: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketApiResponse {
    schema_version: u64,
    repositories: Vec<MarketRepoRaw>,
}

#[derive(Debug, Clone, Serialize)]
struct MarketPlugin {
    name: String,
    full_name: String,
    spec: String,
    description: String,
    url: String,
    homepage: String,
    avatar_url: String,
    topics: Vec<String>,
    language: String,
    stars: u64,
    pushed_at: String,
    archived: bool,
    project_type: String,
    category: String,
    verified: bool,
}

#[derive(Debug, Clone, Serialize)]
struct MarketCatalog {
    plugins: Vec<MarketPlugin>,
    fetched_at: u64,
}

struct RuntimeState {
    phase: Phase,
    message: String,
    url: String,
    child: Option<Child>,
    pid: Option<u32>,
    // 端口上的服务由用户在 Launcher 之外启动：只沿用不接管，停止/退出都不碰它。
    external: bool,
    logs: VecDeque<String>,
    generation: u64,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            phase: Phase::Stopped,
            message: "dsh 尚未启动".into(),
            url: "http://127.0.0.1:3080".into(),
            child: None,
            pid: None,
            external: false,
            logs: VecDeque::new(),
            generation: 0,
        }
    }
}

#[derive(Clone)]
struct AppState {
    runtime: Arc<Mutex<RuntimeState>>,
    market: Arc<Mutex<Option<MarketCatalog>>>,
    // 拖出标签新建窗口时，按窗口 label 暂存它要带走的标签标题。
    pending_tabs: Arc<Mutex<HashMap<String, String>>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            runtime: Arc::new(Mutex::new(RuntimeState::default())),
            market: Arc::new(Mutex::new(None)),
            pending_tabs: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

fn snapshot(runtime: &RuntimeState) -> LauncherStatus {
    LauncherStatus {
        phase: runtime.phase,
        message: runtime.message.clone(),
        url: runtime.url.clone(),
        pid: runtime.pid,
        external: runtime.external,
        logs: runtime.logs.iter().cloned().collect(),
    }
}

fn emit_status(app: &AppHandle, state: &AppState) {
    if let Ok(runtime) = state.runtime.lock() {
        let _ = app.emit("launcher-status", snapshot(&runtime));
    }
}

fn push_log(app: &AppHandle, state: &AppState, generation: u64, source: &str, line: String) {
    if let Ok(mut runtime) = state.runtime.lock() {
        if runtime.generation != generation {
            return;
        }
        runtime.logs.push_back(format!("[{source}] {line}"));
        while runtime.logs.len() > MAX_LOG_LINES {
            runtime.logs.pop_front();
        }
    }
    emit_status(app, state);
}

fn spawn_log_reader<R>(
    app: AppHandle,
    state: AppState,
    generation: u64,
    source: &'static str,
    stream: R,
) where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            push_log(&app, &state, generation, source, line);
        }
    });
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("config.json"))
        .map_err(|error| format!("无法确定配置目录：{error}"))
}

fn read_config(app: &AppHandle) -> Result<LauncherConfig, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(LauncherConfig::default());
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("配置文件格式无效：{error}"))
}

fn validate_config(config: &LauncherConfig) -> Result<(), String> {
    if config.port < 1024 {
        return Err("端口必须在 1024 到 65535 之间".into());
    }
    let workspace = Path::new(&config.working_directory);
    if !workspace.is_dir() {
        return Err(format!("工作目录不存在：{}", workspace.display()));
    }
    if matches!(config.launch_mode, LaunchMode::Command) && config.executable.trim().is_empty() {
        return Err("dsh 命令不能为空".into());
    }
    if matches!(config.launch_mode, LaunchMode::Npx) && !is_safe_package_spec(&config.npx_package) {
        return Err("npm 包名包含不支持的字符".into());
    }
    if config.download_directory.trim().is_empty()
        || !Path::new(config.download_directory.trim()).is_absolute()
    {
        return Err("下载目录必须是绝对路径".into());
    }
    if config
        .trusted_hosts
        .iter()
        .any(|host| !is_safe_authority(host))
    {
        return Err("可信主机只能包含字母、数字、点、短横线、方括号、冒号和下划线".into());
    }
    Ok(())
}

fn is_safe_package_spec(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "@/._-".contains(ch))
}

fn is_safe_authority(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ".-_:[]".contains(ch))
}

fn save_config_file(app: &AppHandle, config: &LauncherConfig) -> Result<(), String> {
    validate_config(config)?;
    let path = config_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录：{error}"))?;
    }
    let text = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    fs::write(&path, text).map_err(|error| format!("无法写入 {}：{error}", path.display()))
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    // GUI 进程派生控制台子进程（npm/taskkill/dsh 等）时，缺少该标志会在
    // release 版弹出黑色控制台窗口。
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

#[cfg(target_os = "macos")]
fn adopt_login_shell_path() {
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
fn command_for_config(config: &LauncherConfig) -> Result<Command, String> {
    let mut args = vec![
        "web".to_string(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        config.port.to_string(),
    ];
    for host in &config.trusted_hosts {
        args.extend(["--trusted-host".to_string(), host.clone()]);
    }
    command_for_invocation(config, &args)
}

#[cfg(windows)]
fn resolve_windows_executable(raw: &str) -> Result<String, String> {
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
fn known_global_bin_dirs() -> Vec<PathBuf> {
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

#[cfg(not(windows))]
fn command_for_config(config: &LauncherConfig) -> Result<Command, String> {
    let mut args = vec![
        "web".to_string(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        config.port.to_string(),
    ];
    for host in &config.trusted_hosts {
        args.extend(["--trusted-host".to_string(), host.clone()]);
    }
    command_for_invocation(config, &args)
}

fn command_for_invocation(config: &LauncherConfig, args: &[String]) -> Result<Command, String> {
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

fn prepare_command(mut command: Command, config: &LauncherConfig) -> Command {
    command.current_dir(&config.working_directory);
    if !config.dsh_home.trim().is_empty() {
        command.env("DSH_HOME", config.dsh_home.trim());
    }
    command
}

fn execute_command(mut command: Command) -> Result<OperationResult, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command.output().map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(OperationResult {
        success: output.status.success(),
        output: combined,
    })
}

fn run_dsh_invocation(config: &LauncherConfig, args: &[String]) -> Result<OperationResult, String> {
    execute_command(prepare_command(
        command_for_invocation(config, args)?,
        config,
    ))
}

fn npm_command() -> Command {
    #[cfg(windows)]
    let mut command =
        Command::new(resolve_windows_executable("npm").unwrap_or_else(|_| "npm.cmd".into()));
    #[cfg(not(windows))]
    let mut command = Command::new("npm");
    hide_console(&mut command);
    command
}

fn wait_for_port_free(port: u16) -> bool {
    // 重启场景里旧进程刚被结束，给系统一点时间释放监听端口。
    for _ in 0..10 {
        match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            Ok(listener) => {
                drop(listener);
                return true;
            }
            Err(_) => thread::sleep(Duration::from_millis(200)),
        }
    }
    false
}

fn http_service_alive(port: u16) -> bool {
    // 端口有监听者时，用一次真实的 HTTP 往返确认对面是活着的 Web 服务，
    // 而不是残留的半死进程或非 HTTP 程序；任何状态码都算有响应。
    let url = format!("http://127.0.0.1:{port}/");
    match ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(1500))
        .build()
        .get(&url)
        .call()
    {
        Ok(_) | Err(ureq::Error::Status(..)) => true,
        Err(ureq::Error::Transport(_)) => false,
    }
}

fn attach_external(
    app: AppHandle,
    state: AppState,
    generation: u64,
    port: u16,
) -> Result<LauncherStatus, String> {
    {
        let mut runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
        if runtime.generation != generation {
            drop(runtime);
            return current_status(state);
        }
        runtime.phase = Phase::Ready;
        runtime.external = true;
        runtime.pid = None;
        runtime.message = format!("已连接到端口 {port} 上已在运行的 dsh 服务（外部启动，Launcher 不会停止它）");
        runtime
            .logs
            .push_back("[launcher] 检测到端口已有 Web 服务在运行，直接沿用外部 dsh".into());
    }
    emit_status(&app, &state);
    let monitor_state = state.clone();
    thread::spawn(move || monitor_external(app, monitor_state, generation, port));
    current_status(state)
}

fn monitor_external(app: AppHandle, state: AppState, generation: u64, port: u16) {
    // 外部服务不归 Launcher 管，但它退出后要把界面从“运行中”翻回可启动状态，
    // 不能让用户对着一个连不上的 iframe。连续三次探测失败才算真的没了。
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut failures = 0;
    loop {
        thread::sleep(Duration::from_millis(900));
        match state.runtime.lock() {
            Ok(runtime) if runtime.generation == generation => {}
            _ => return,
        }
        if TcpStream::connect_timeout(&address, Duration::from_millis(400)).is_ok() {
            failures = 0;
            continue;
        }
        failures += 1;
        if failures < 3 {
            continue;
        }
        if let Ok(mut runtime) = state.runtime.lock() {
            if runtime.generation != generation {
                return;
            }
            runtime.phase = Phase::Stopped;
            runtime.external = false;
            runtime.message = "外部 dsh 服务已停止，可以在这里重新启动".into();
        }
        emit_status(&app, &state);
        return;
    }
}

fn start_process(app: AppHandle, state: AppState) -> Result<LauncherStatus, String> {
    let config = read_config(&app)?;
    validate_config(&config)?;
    let generation;
    {
        let mut runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
        if matches!(
            runtime.phase,
            Phase::Starting | Phase::Ready | Phase::Stopping
        ) {
            return Ok(snapshot(&runtime));
        }
        runtime.generation += 1;
        generation = runtime.generation;
        runtime.phase = Phase::Starting;
        runtime.external = false;
        runtime.message = "正在启动 dsh web…".into();
        runtime.url = format!("http://127.0.0.1:{}", config.port);
        runtime.logs.clear();
    }
    emit_status(&app, &state);

    // 端口被占用不再直接报错：先探测占用者是否是可用的 Web 服务（多半是用户
    // 提前手动启动的 dsh web）。是就直接沿用——不登记子进程、不接管、退出时
    // 也不结束它；不是（比如刚结束的进程还没释放端口）才照旧等待释放。
    let port_free = match TcpListener::bind((Ipv4Addr::LOCALHOST, config.port)) {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(_) => false,
    };
    if !port_free {
        if http_service_alive(config.port) {
            return attach_external(app, state, generation, config.port);
        }
        if !wait_for_port_free(config.port) {
            return Err(format!(
                "端口 {} 已被占用，且占用者不像一个可用的 Web 服务。请结束占用进程，或在“管理”里换一个端口。",
                config.port
            ));
        }
    }

    let mut command = command_for_config(&config)?;
    command
        .current_dir(&config.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !config.dsh_home.trim().is_empty() {
        command.env("DSH_HOME", config.dsh_home.trim());
    }
    #[cfg(unix)]
    {
        // 独立进程组：停止时可以对整组发信号，不会留下孤儿 node 进程。
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|error| {
        if matches!(config.launch_mode, LaunchMode::Command) {
            format!("无法启动 dsh：{error}。请确认已执行 npm install -g @deepseek-ai/dsh，或切换为 npx。")
        } else {
            format!("无法启动 npx：{error}。请确认 Node.js 已安装并在 PATH 中。")
        }
    })?;
    let pid = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    {
        let mut runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
        if runtime.generation != generation {
            // 启动期间被 stop 抢先（generation 已推进）：不登记这个子进程，直接收尾。
            drop(runtime);
            kill_child_tree(child);
            return current_status(state);
        }
        runtime.pid = Some(pid);
        runtime.child = Some(child);
        runtime.message = format!("等待 {} 响应…", runtime.url);
    }

    if let Some(stdout) = stdout {
        spawn_log_reader(app.clone(), state.clone(), generation, "stdout", stdout);
    }
    if let Some(stderr) = stderr {
        spawn_log_reader(app.clone(), state.clone(), generation, "stderr", stderr);
    }

    let startup_timeout = match config.launch_mode {
        // npx 首次运行可能要现场下载整个包。
        LaunchMode::Npx => Duration::from_secs(180),
        LaunchMode::Command => Duration::from_secs(60),
    };
    let monitor_app = app.clone();
    let monitor_state = state.clone();
    let port = config.port;
    thread::spawn(move || {
        monitor_process(
            monitor_app,
            monitor_state,
            generation,
            port,
            startup_timeout,
        )
    });
    emit_status(&app, &state);
    current_status(state)
}

fn monitor_process(
    app: AppHandle,
    state: AppState,
    generation: u64,
    port: u16,
    startup_timeout: Duration,
) {
    let started = Instant::now();
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let mut ready = false;
    loop {
        thread::sleep(Duration::from_millis(if ready { 800 } else { 250 }));
        let exited = {
            let mut runtime = match state.runtime.lock() {
                Ok(runtime) => runtime,
                Err(_) => return,
            };
            if runtime.generation != generation {
                return;
            }
            match runtime
                .child
                .as_mut()
                .and_then(|child| child.try_wait().ok())
                .flatten()
            {
                Some(exit) => {
                    runtime.child = None;
                    runtime.pid = None;
                    if runtime.phase == Phase::Stopping {
                        runtime.phase = Phase::Stopped;
                        runtime.message = "dsh 已停止".into();
                    } else {
                        runtime.phase = Phase::Failed;
                        let detail = runtime
                            .logs
                            .iter()
                            .rev()
                            .find(|line| !line.trim().is_empty())
                            .map(|line| truncate_chars(line, 240));
                        let prefix = if ready {
                            "dsh 进程意外退出"
                        } else {
                            "dsh 进程已退出"
                        };
                        runtime.message = match detail {
                            Some(detail) => format!("{prefix}（{exit}）：{detail}"),
                            None => format!("{prefix}（{exit}）"),
                        };
                    }
                    true
                }
                None => false,
            }
        };
        if exited {
            emit_status(&app, &state);
            return;
        }
        if ready {
            // 就绪后继续守护子进程，崩溃时把状态从“运行中”翻成失败，
            // 而不是留着一个连不上的 iframe。
            continue;
        }
        if TcpStream::connect_timeout(&address, Duration::from_millis(180)).is_ok() {
            if let Ok(mut runtime) = state.runtime.lock() {
                if runtime.generation == generation && runtime.phase == Phase::Starting {
                    runtime.phase = Phase::Ready;
                    runtime.message = format!("正在监听 {}", runtime.url);
                }
            }
            emit_status(&app, &state);
            ready = true;
            continue;
        }
        if started.elapsed() > startup_timeout {
            // 超时后必须终止子进程；否则它继续占着端口，下次启动会把它的
            // Child 悄悄丢掉，留下一个失控的孤儿进程。
            let child = {
                let mut runtime = match state.runtime.lock() {
                    Ok(runtime) => runtime,
                    Err(_) => return,
                };
                if runtime.generation != generation {
                    return;
                }
                runtime.phase = Phase::Failed;
                runtime.message = format!(
                    "dsh 启动超时（{} 秒内未监听端口 {port}），已终止进程，请检查运行日志",
                    startup_timeout.as_secs()
                );
                runtime.pid = None;
                runtime.child.take()
            };
            if let Some(child) = child {
                kill_child_tree(child);
            }
            emit_status(&app, &state);
            return;
        }
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut truncated = value
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[tauri::command]
fn get_launcher_version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

const LAUNCHER_RELEASE_API: &str = "https://api.github.com/repos/WEP-56/DSH-Launcher/releases/latest";

fn release_version_key(value: &str) -> Vec<u64> {
    let mut parts: Vec<u64> = value
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .map(|part| {
            part.chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect();
    while parts.last() == Some(&0) {
        parts.pop();
    }
    parts
}

#[tauri::command]
async fn check_launcher_update() -> Result<ReleaseInfo, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("dsh-launcher/", env!("CARGO_PKG_VERSION")))
        .build();
    let response = agent
        .get(LAUNCHER_RELEASE_API)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| format!("无法访问 GitHub Release：{error}"))?;
    let mut body = String::new();
    response
        .into_reader()
        .take(4 * 1024 * 1024)
        .read_to_string(&mut body)
        .map_err(|error| format!("GitHub Release 响应无效：{error}"))?;
    let value: serde_json::Value = serde_json::from_str(&body)
        .map_err(|error| format!("GitHub Release 响应无效：{error}"))?;
    let tag_name = value.get("tag_name").and_then(|item| item.as_str()).unwrap_or_default().to_string();
    if tag_name.is_empty() {
        return Err("GitHub 没有可用的最新 Release".into());
    }
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let latest_version = tag_name.trim_start_matches(['v', 'V']).to_string();
    let update_available = release_version_key(&latest_version) > release_version_key(&current_version);
    Ok(ReleaseInfo {
        current_version,
        latest_version,
        tag_name,
        name: value.get("name").and_then(|item| item.as_str()).unwrap_or_default().to_string(),
        body: value.get("body").and_then(|item| item.as_str()).unwrap_or_default().to_string(),
        html_url: value.get("html_url").and_then(|item| item.as_str()).unwrap_or_default().to_string(),
        published_at: value.get("published_at").and_then(|item| item.as_str()).unwrap_or_default().to_string(),
        update_available,
    })
}

fn kill_child_tree(mut child: Child) {
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
        // dsh 以独立进程组启动（见 start_process），负 pid 对整组发信号，
        // 保证 npx → node → dsh 的整棵进程树一起结束。
        unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn stop_process(app: &AppHandle, state: &AppState) -> Result<LauncherStatus, String> {
    let (child, status) = {
        let mut runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
        runtime.generation += 1;
        runtime.pid = None;
        match runtime.child.take() {
            Some(child) => {
                runtime.phase = Phase::Stopping;
                runtime.message = "正在停止 dsh…".into();
                (Some(child), snapshot(&runtime))
            }
            None => {
                // 没有登记过子进程：要么本来就没启动，要么沿用的是外部服务。
                // 外部服务只“断开”，绝不结束一个不是我们启动的进程。
                let was_external = runtime.external;
                runtime.external = false;
                runtime.phase = Phase::Stopped;
                runtime.message = if was_external {
                    "已断开与外部 dsh 服务的连接（该服务仍在运行）".into()
                } else {
                    "dsh 尚未启动".into()
                };
                (None, snapshot(&runtime))
            }
        }
    };
    emit_status(app, state);
    let Some(child) = child else {
        return Ok(status);
    };

    kill_child_tree(child);
    let mut runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
    runtime.phase = Phase::Stopped;
    runtime.message = "dsh 已停止".into();
    let status = snapshot(&runtime);
    drop(runtime);
    emit_status(app, state);
    Ok(status)
}

#[tauri::command]
fn load_config(app: AppHandle) -> Result<LauncherConfig, String> {
    read_config(&app)
}

#[tauri::command]
fn default_config() -> LauncherConfig {
    LauncherConfig::default()
}

#[tauri::command]
fn save_config(app: AppHandle, config: LauncherConfig) -> Result<LauncherConfig, String> {
    save_config_file(&app, &config)?;
    Ok(config)
}

#[tauri::command]
fn save_window_size(app: AppHandle, width: u32, height: u32) -> Result<(), String> {
    // Keep persisted dimensions within the same practical limits as the window
    // definition, and avoid writing transient zero-sized resize events.
    if !(620..=4096).contains(&width) || !(560..=4096).contains(&height) {
        return Ok(());
    }
    let mut config = read_config(&app)?;
    config.window_width = width;
    config.window_height = height;
    save_config_file(&app, &config)
}

#[tauri::command]
fn get_status(state: State<'_, AppState>) -> Result<LauncherStatus, String> {
    current_status(state.inner().clone())
}

fn current_status(state: AppState) -> Result<LauncherStatus, String> {
    state
        .runtime
        .lock()
        .map(|runtime| snapshot(&runtime))
        .map_err(|_| "启动器状态锁已损坏".into())
}

fn start_with_feedback(app: &AppHandle, state: &AppState) -> Result<LauncherStatus, String> {
    let result = start_process(app.clone(), state.clone());
    if let Err(error) = &result {
        if let Ok(mut runtime) = state.runtime.lock() {
            runtime.phase = Phase::Failed;
            runtime.message = error.clone();
        }
        // 广播失败状态，否则界面会一直停在“正在启动”。
        emit_status(app, state);
    }
    result
}

// 涉及子进程、网络或窗口创建的命令必须是 async：Tauri 2 的同步命令在主线程
// 上执行，任何阻塞都会冻结所有窗口的事件循环（拖拽、缩放、关闭全部失效）。
#[tauri::command]
async fn start_dsh(app: AppHandle, state: State<'_, AppState>) -> Result<LauncherStatus, String> {
    start_with_feedback(&app, state.inner())
}

#[tauri::command]
async fn stop_dsh(app: AppHandle, state: State<'_, AppState>) -> Result<LauncherStatus, String> {
    stop_process(&app, state.inner())
}

#[tauri::command]
async fn restart_dsh(app: AppHandle, state: State<'_, AppState>) -> Result<LauncherStatus, String> {
    stop_process(&app, state.inner())?;
    start_with_feedback(&app, state.inner())
}

#[tauri::command]
fn clear_logs(state: State<'_, AppState>) -> Result<LauncherStatus, String> {
    let mut runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
    runtime.logs.clear();
    Ok(snapshot(&runtime))
}

fn dsh_home_for(config: &LauncherConfig) -> PathBuf {
    if !config.dsh_home.trim().is_empty() {
        return PathBuf::from(config.dsh_home.trim());
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dsh")
}

fn config_file_path(config: &LauncherConfig, id: &str) -> Option<(String, String, PathBuf)> {
    let home = dsh_home_for(config);
    let (name, relative) = match id {
        "credentials" => (
            "全局凭据 · .credentials.yaml",
            PathBuf::from(".credentials.yaml"),
        ),
        "settings" => ("全局设置 · settings.yaml", PathBuf::from("settings.yaml")),
        "home-patch" => (
            "全局补丁 · cordis.patch.yml",
            PathBuf::from("cordis.patch.yml"),
        ),
        "web-manifest" => (
            "Web profile · package.json",
            PathBuf::from("profiles/web/package.json"),
        ),
        "web-patch" => (
            "Web profile · cordis.patch.yml",
            PathBuf::from("profiles/web/cordis.patch.yml"),
        ),
        "web-workspace" => (
            "Web profile · pnpm-workspace.yaml",
            PathBuf::from("profiles/web/pnpm-workspace.yaml"),
        ),
        _ => return None,
    };
    let path = home.join(&relative);
    Some((id.to_string(), name.to_string(), path))
}

#[tauri::command]
fn list_config_files(app: AppHandle) -> Result<Vec<ConfigFileInfo>, String> {
    let config = read_config(&app)?;
    let mut files = Vec::new();
    for id in [
        "credentials",
        "settings",
        "home-patch",
        "web-manifest",
        "web-patch",
        "web-workspace",
    ] {
        if let Some((id, name, path)) = config_file_path(&config, id) {
            let content = if path.exists() {
                fs::read_to_string(&path)
                    .map_err(|error| format!("无法读取 {}：{error}", path.display()))?
            } else {
                String::new()
            };
            files.push(ConfigFileInfo {
                id,
                name,
                path: path.to_string_lossy().into_owned(),
                content,
                editable: path.extension().is_some(),
            });
        }
    }
    Ok(files)
}

#[tauri::command]
fn save_dsh_config(app: AppHandle, id: String, content: String) -> Result<ConfigFileInfo, String> {
    let config = read_config(&app)?;
    let (id, name, path) =
        config_file_path(&config, &id).ok_or_else(|| "未知的 dsh 配置文件".to_string())?;
    if content.len() > 2_000_000 {
        return Err("配置文件超过 2 MB，未保存".into());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录：{error}"))?;
    }
    fs::write(&path, &content).map_err(|error| format!("无法写入 {}：{error}", path.display()))?;
    Ok(ConfigFileInfo {
        id,
        name,
        path: path.to_string_lossy().into_owned(),
        content,
        editable: true,
    })
}

#[tauri::command]
fn open_dsh_config(app: AppHandle, id: String) -> Result<(), String> {
    let config = read_config(&app)?;
    let (_, _, path) =
        config_file_path(&config, &id).ok_or_else(|| "未知的 dsh 配置文件".to_string())?;
    #[cfg(windows)]
    {
        let target = if path.exists() {
            path
        } else {
            path.parent().unwrap_or(Path::new(".")).to_path_buf()
        };
        Command::new("explorer.exe")
            .arg(target)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(windows))]
    {
        let target = if path.exists() {
            path
        } else {
            path.parent().unwrap_or(Path::new(".")).to_path_buf()
        };
        Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn extract_version(value: &str) -> String {
    value
        .split_whitespace()
        .map(|part| part.trim_matches(['"', '\'', ',', '[', ']']))
        .find(|part| part.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .unwrap_or("未知")
        .to_string()
}

#[tauri::command]
async fn get_package_info(app: AppHandle) -> Result<PackageInfo, String> {
    let config = read_config(&app)?;
    let version_args = vec!["--version".to_string()];
    let current = run_dsh_invocation(&config, &version_args)?;
    let mut npm = npm_command();
    npm.args(["view", &config.npx_package, "version", "--json"]);
    let latest = execute_command(prepare_command(npm, &config))?;
    let current_version = if current.success {
        extract_version(&current.output)
    } else {
        "不可用".into()
    };
    let latest_version = if latest.success {
        extract_version(&latest.output)
    } else {
        "查询失败".into()
    };
    Ok(PackageInfo {
        current_version,
        latest_version,
        source: match config.launch_mode {
            LaunchMode::Command => config.executable,
            LaunchMode::Npx => format!("npx {}", config.npx_package),
        },
        checked_at: epoch_secs().to_string(),
        detail: [current.output, latest.output]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
    })
}

#[tauri::command]
async fn update_dsh(app: AppHandle, state: State<'_, AppState>) -> Result<OperationResult, String> {
    let config = read_config(&app)?;
    // 外部服务不归我们停/启，更新时不做无意义的断开-重连。
    let was_ready = {
        let runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
        runtime.phase == Phase::Ready && !runtime.external
    };
    // npm install -g 会重写正在运行的安装目录，先停服务，更新完再恢复。
    if was_ready {
        stop_process(&app, state.inner())?;
    }
    let mut npm = npm_command();
    npm.args([
        "install",
        "--global",
        &format!("{}@latest", config.npx_package),
    ]);
    let result = execute_command(prepare_command(npm, &config));
    if was_ready {
        let _ = start_with_feedback(&app, state.inner());
    }
    result
}

fn is_safe_plugin_spec(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "@/._-:# +~=".contains(ch))
}

fn profile_manifest_path(config: &LauncherConfig) -> PathBuf {
    dsh_home_for(config)
        .join("profiles")
        .join("web")
        .join("package.json")
}

#[tauri::command]
fn list_plugins(app: AppHandle) -> Result<Vec<InstalledPlugin>, String> {
    let config = read_config(&app)?;
    let path = profile_manifest_path(&config);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("profile package.json 无效：{error}"))?;
    let dependencies = value
        .get("dependencies")
        .and_then(serde_json::Value::as_object);
    let mut plugins = Vec::new();
    if let Some(dependencies) = dependencies {
        for (name, spec) in dependencies {
            let package_path = dsh_home_for(&config)
                .join("profiles")
                .join("web")
                .join("node_modules")
                .join(name)
                .join("package.json");
            let bundle = package_path.exists()
                && fs::read_to_string(&package_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                    .and_then(|manifest| manifest.get("dsh")?.get("bundle").cloned())
                    .is_some();
            plugins.push(InstalledPlugin {
                name: name.clone(),
                version: spec.as_str().unwrap_or("*").to_string(),
                bundle,
            });
        }
    }
    Ok(plugins)
}

#[tauri::command]
async fn search_plugins(query: String) -> Result<Vec<PluginSearchResult>, String> {
    let term = if query.trim().is_empty() {
        "dsh-plugin".to_string()
    } else {
        format!("{} dsh-plugin", query.trim())
    };
    let mut npm = npm_command();
    npm.args(["search", "--json", "--searchlimit", "30", &term]);
    let result = execute_command(npm)?;
    if !result.success {
        return Err(result.output);
    }
    let values: serde_json::Value = serde_json::from_str(&result.output)
        .map_err(|error| format!("npm 搜索结果无效：{error}"))?;
    let mut results = Vec::new();
    for value in values.as_array().into_iter().flatten() {
        let keywords = value
            .get("keywords")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        let name = value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let is_dsh = keywords.iter().any(|keyword| {
            ["dsh-plugin", "deepseek-harness", "dsh"]
                .iter()
                .any(|needle| keyword.to_ascii_lowercase().contains(needle))
        }) || name.starts_with("@deepseek-ai/dsh-");
        if !is_dsh {
            continue;
        }
        let links = value.get("links");
        results.push(PluginSearchResult {
            name,
            version: value
                .get("version")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("未知")
                .to_string(),
            description: value
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("无描述")
                .to_string(),
            homepage: links
                .and_then(|links| links.get("homepage"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            npm_url: links
                .and_then(|links| links.get("npm"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            keywords,
        });
    }
    Ok(results)
}

const MARKET_API_URL: &str = "https://dsh.aitreez.com/catalog.json";

fn parse_market_catalog(json: &str) -> Result<MarketCatalog, String> {
    let response: MarketApiResponse = serde_json::from_str(json)
        .map_err(|error| format!("插件目录 API 响应解析失败：{error}"))?;
    if response.schema_version != 1 {
        return Err(format!(
            "插件目录 API 版本不兼容：schemaVersion={}",
            response.schema_version
        ));
    }
    let plugins = response
        .repositories
        .into_iter()
        .map(|repo| MarketPlugin {
            spec: format!("github:{}", repo.full_name),
            name: repo.name,
            full_name: repo.full_name,
            description: repo.description.unwrap_or_default(),
            url: repo.url,
            homepage: repo.homepage.unwrap_or_default(),
            avatar_url: repo.owner.map(|owner| owner.avatar_url).unwrap_or_default(),
            topics: repo.topics,
            language: repo.language.unwrap_or_default(),
            stars: repo.stars,
            pushed_at: repo.pushed_at.unwrap_or_default(),
            archived: repo.archived,
            project_type: repo.project_type.unwrap_or_else(|| "unknown".into()),
            category: repo
                .category
                .or_else(|| repo.categories.first().cloned())
                .unwrap_or_else(|| "other".into()),
            verified: repo
                .validation
                .as_ref()
                .is_some_and(|validation| validation.overall == "verified"),
        })
        .collect();
    Ok(MarketCatalog {
        plugins,
        fetched_at: epoch_secs(),
    })
}

fn fetch_market_catalog() -> Result<MarketCatalog, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(25))
        .user_agent(concat!("dsh-launcher/", env!("CARGO_PKG_VERSION")))
        .build();
    let response = agent
        .get(MARKET_API_URL)
        .set("Accept", "application/json")
        .call()
        .map_err(|error| format!("无法访问插件商店 API：{error}"))?;
    let mut json = String::new();
    response
        .into_reader()
        .take(20 * 1024 * 1024)
        .read_to_string(&mut json)
        .map_err(|error| format!("插件商店 API 响应读取失败：{error}"))?;
    parse_market_catalog(&json)
}

#[tauri::command]
async fn fetch_market(state: State<'_, AppState>, force: bool) -> Result<MarketCatalog, String> {
    const MARKET_TTL_SECS: u64 = 600;
    if !force {
        if let Ok(cache) = state.market.lock() {
            if let Some(catalog) = cache.as_ref() {
                if epoch_secs().saturating_sub(catalog.fetched_at) < MARKET_TTL_SECS {
                    return Ok(catalog.clone());
                }
            }
        }
    }
    match fetch_market_catalog() {
        Ok(catalog) => {
            if let Ok(mut cache) = state.market.lock() {
                *cache = Some(catalog.clone());
            }
            Ok(catalog)
        }
        Err(error) => {
            // 后台自动刷新失败时退回旧数据；用户手动刷新（force）则要看到错误。
            if !force {
                if let Ok(cache) = state.market.lock() {
                    if let Some(catalog) = cache.as_ref() {
                        return Ok(catalog.clone());
                    }
                }
            }
            Err(error)
        }
    }
}

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("只允许打开 http(s) 链接".into());
    }
    #[cfg(windows)]
    {
        let mut command = Command::new("explorer.exe");
        hide_console(&mut command);
        command
            .arg(&url)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&url)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(&url)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn run_plugin_action(
    app: &AppHandle,
    state: &AppState,
    verb: &str,
    spec: &str,
) -> Result<OperationResult, String> {
    if !is_safe_plugin_spec(spec) {
        return Err("插件标识包含不支持的字符".into());
    }
    let config = read_config(app)?;
    // 外部服务不归我们停/启，插件操作只改 profile 文件，改完由用户自行重启生效。
    let was_ready = {
        let runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
        runtime.phase == Phase::Ready && !runtime.external
    };
    if was_ready {
        stop_process(app, state)?;
    }
    let args = vec![
        "plugin".into(),
        "--profile".into(),
        "web".into(),
        verb.into(),
        spec.into(),
    ];
    // 无论安装/卸载是否成功，只要之前在运行就把服务拉回来，
    // 不能让一次失败的插件操作把服务留在停止状态。
    let result = run_dsh_invocation(&config, &args);
    if was_ready {
        let _ = start_with_feedback(app, state);
    }
    result
}

#[tauri::command]
async fn install_plugin(
    app: AppHandle,
    state: State<'_, AppState>,
    spec: String,
) -> Result<OperationResult, String> {
    run_plugin_action(&app, state.inner(), "add", &spec)
}

#[tauri::command]
async fn remove_plugin(
    app: AppHandle,
    state: State<'_, AppState>,
    spec: String,
) -> Result<OperationResult, String> {
    run_plugin_action(&app, state.inner(), "remove", &spec)
}

// 标题栏的逻辑高度（px），前端 .titlebar 与这里必须一致；拖拽落点命中
// 其他窗口的这段区域时视为“拖入标签栏”。
const TITLEBAR_LOGICAL_HEIGHT: f64 = 35.0;
const TAB_DRAG_PREVIEW_LABEL: &str = "tab-drag-preview";

fn next_window_label() -> String {
    static WINDOW_COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "control-{}",
        WINDOW_COUNTER.fetch_add(1, Ordering::Relaxed) + 1
    )
}

fn reveal_window_fallback(window: tauri::WebviewWindow) {
    // 窗口以隐藏状态创建，由前端首帧就绪后 show()，避免启动白屏。这里兜底：
    // 前端脚本万一没跑起来，也要在几秒内把窗口亮出来，不能凭空消失。
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(3));
        if !window.is_visible().unwrap_or(true) {
            let _ = window.show();
        }
    });
}

fn fallback_download_directory() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn safe_download_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| {
            if character.is_control() || r#"<>:\"/|?*"#.contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    if trimmed.is_empty() {
        "download".into()
    } else {
        trimmed.to_string()
    }
}

fn unique_download_path(directory: &Path, file_name: &str) -> PathBuf {
    let candidate = directory.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let extension = Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    for index in 1..100_000 {
        let candidate = directory.join(format!("{stem} ({index}){extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    directory.join(format!("{stem}-{}{extension}", epoch_secs()))
}

fn confirm_download(file_name: &str) -> bool {
    matches!(
        MessageDialog::new()
            .set_title("DSH Launcher")
            .set_description(format!("是否下载“{file_name}”？"))
            .set_buttons(MessageButtons::OkCancel)
            .show(),
        MessageDialogResult::Ok
    )
}

fn choose_download_path(directory: &Path, file_name: &str) -> Option<PathBuf> {
    FileDialog::new()
        .set_directory(directory)
        .set_file_name(file_name)
        .save_file()
}

fn handle_download_request(
    app: &AppHandle,
    url: &tauri::Url,
    destination: &mut PathBuf,
) -> bool {
    let config = read_config(app).unwrap_or_default();
    let suggested_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .map(safe_download_file_name)
        .unwrap_or_else(|| "download".into());
    let default_directory = {
        let configured = PathBuf::from(config.download_directory.trim());
        if configured.is_absolute() {
            configured
        } else {
            fallback_download_directory()
        }
    };
    if config.download_choose_location {
        return choose_download_path(&default_directory, &suggested_name)
            .map(|path| {
                if !path.is_absolute() {
                    return false;
                }
                *destination = path;
                true
            })
            .unwrap_or(false);
    }
    if let Err(error) = fs::create_dir_all(&default_directory) {
        eprintln!("无法创建下载目录 {}：{error}", default_directory.display());
        return false;
    }
    if config.download_ask && !confirm_download(&suggested_name) {
        return false;
    }
    *destination = unique_download_path(&default_directory, &suggested_name);
    eprintln!("下载 {} 到 {}", url, destination.display());
    true
}

fn spawn_launcher_window_named(
    app: &AppHandle,
    state: &AppState,
    label: String,
    initial_tab: Option<String>,
    position: Option<(f64, f64)>,
    size: Option<(f64, f64)>,
) -> Result<(), String> {
    if let Some(title) = initial_tab {
        if let Ok(mut pending) = state.pending_tabs.lock() {
            pending.insert(label.clone(), title);
        }
    }
    // Use the app URL so Tauri resolves the dev server in development and
    // the bundled frontend in production. External URLs do not reliably get
    // the launcher initialization scripts and window APIs.
    let (width, height) = size.unwrap_or((880.0, 760.0));
    let mut builder =
        WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
            .title("DSH Launcher")
            .inner_size(width, height)
            .min_inner_size(620.0, 560.0)
            .resizable(true)
            .visible(false);
    // macOS：启用原生装饰 + 叠加标题栏，露出标准红黄绿按钮（左侧）。
    // Windows/Linux：保持无边框自定义标题栏（与作者原始行为完全一致）。
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .decorations(true)
            .title_bar_style(TitleBarStyle::Overlay);
    }
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.decorations(false);
    }
    builder = match position {
        Some((x, y)) => builder.position(x, y),
        None => builder.center(),
    };
    let download_app = app.clone();
    let window = builder
        .on_download(move |_webview, event| match event {
            DownloadEvent::Requested { url, destination } => {
                handle_download_request(&download_app, &url, destination)
            }
            DownloadEvent::Finished { url, path, success } => {
                if success {
                    if let Some(path) = path {
                        eprintln!("下载完成 {url}：{}", path.display());
                    }
                } else {
                    eprintln!("下载失败 {url}");
                }
                true
            }
            _ => true,
        })
        .build()
        .map_err(|error| format!("无法创建 Launcher 窗口：{error}"))?;
    reveal_window_fallback(window);
    Ok(())
}

fn spawn_launcher_window(
    app: &AppHandle,
    state: &AppState,
    initial_tab: Option<String>,
    position: Option<(f64, f64)>,
    size: Option<(f64, f64)>,
) -> Result<(), String> {
    spawn_launcher_window_named(
        app,
        state,
        next_window_label(),
        initial_tab,
        position,
        size,
    )
}

fn setup_tab_drag_preview(app: &tauri::App) -> tauri::Result<()> {
    let preview = WebviewWindowBuilder::new(
        app,
        TAB_DRAG_PREVIEW_LABEL,
        WebviewUrl::App("drag-preview.html".into()),
    )
    .title("标签拖拽预览")
    .inner_size(190.0, 30.0)
    .resizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false);
    // `.transparent(true)` 需要 Tauri 的 `macos-private-api` feature，在 macOS 上
    // 该 API 默认未暴露（且开启私有 API 会影响上架/公证），故仅在非 macOS 启用。
    #[cfg(not(target_os = "macos"))]
    let preview = preview.transparent(true);
    let preview = preview.build()?;
    preview.set_ignore_cursor_events(true)?;
    Ok(())
}

fn position_tab_drag_preview(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    let preview = app
        .get_webview_window(TAB_DRAG_PREVIEW_LABEL)
        .ok_or("标签拖拽预览窗口不可用")?;
    let cursor = app
        .cursor_position()
        .map_err(|error| format!("无法获取光标位置：{error}"))?;
    preview
        .set_position(tauri::PhysicalPosition::new(
            (cursor.x + 14.0).round() as i32,
            (cursor.y + 10.0).round() as i32,
        ))
        .map_err(|error| error.to_string())?;
    Ok(preview)
}

#[tauri::command]
fn show_tab_drag_preview(app: AppHandle, title: String) -> Result<(), String> {
    let preview = position_tab_drag_preview(&app)?;
    let title = serde_json::to_string(&title).map_err(|error| error.to_string())?;
    preview
        .eval(&format!("window.setTabTitle?.({title})"))
        .map_err(|error| error.to_string())?;
    preview.show().map_err(|error| error.to_string())
}

#[tauri::command]
fn move_tab_drag_preview(app: AppHandle) -> Result<(), String> {
    position_tab_drag_preview(&app).map(|_| ())
}

#[tauri::command]
fn hide_tab_drag_preview(app: AppHandle) -> Result<(), String> {
    if let Some(preview) = app.get_webview_window(TAB_DRAG_PREVIEW_LABEL) {
        preview.hide().map_err(|error| error.to_string())?;
    }
    Ok(())
}

// 必须是 async：同步命令在主线程执行，而在主线程上同步创建 WebView 会在
// Windows 上死锁（wry#583）——新窗口停在白屏，且事件循环被卡住，所有窗口的
// 拖拽/最小化/关闭全部失效。
#[tauri::command]
async fn new_launcher_window(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    spawn_launcher_window(&app, state.inner(), None, None, None)
}

#[tauri::command]
fn take_initial_tab(window: tauri::WebviewWindow, state: State<'_, AppState>) -> Option<String> {
    state
        .pending_tabs
        .lock()
        .ok()
        .and_then(|mut pending| pending.remove(window.label()))
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum TabDropOutcome {
    Adopted,
    Detached,
    None,
}

#[derive(Debug, Clone, Serialize)]
struct AdoptedTab {
    title: String,
}

// 标签被拖出窗口后松手：落点在其他 Launcher 窗口的标签栏上就让它收编，
// 否则在光标处新建一个窗口带走这个标签。必须 async（见 new_launcher_window）。
#[tauri::command]
async fn drop_tab(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    title: String,
    remaining: u32,
) -> Result<TabDropOutcome, String> {
    let cursor = app
        .cursor_position()
        .map_err(|error| format!("无法获取光标位置：{error}"))?;
    for (label, target) in app.webview_windows() {
        if label == window.label() || !label.starts_with("control") {
            continue;
        }
        if target.is_minimized().unwrap_or(false) || !target.is_visible().unwrap_or(false) {
            continue;
        }
        let (Ok(position), Ok(size)) = (target.outer_position(), target.outer_size()) else {
            continue;
        };
        let scale = target.scale_factor().unwrap_or(1.0);
        // 比标题栏多放宽几个像素，拖拽不必像素级精确。
        let strip_bottom = position.y as f64 + (TITLEBAR_LOGICAL_HEIGHT + 8.0) * scale;
        let inside_x = cursor.x >= position.x as f64
            && cursor.x <= position.x as f64 + size.width as f64;
        let inside_y = cursor.y >= position.y as f64 && cursor.y <= strip_bottom;
        if inside_x && inside_y {
            let _ = target.set_focus();
            app.emit_to(label.as_str(), "adopt-tab", AdoptedTab { title })
                .map_err(|error| error.to_string())?;
            // 被拆出的窗口只剩这一枚标签时，并入目标窗口后应立即销毁源窗口。
            // 稍作延迟，让命令结果先回到前端；主窗口不能关闭，否则会触发托盘退出逻辑。
            if window.label() != "control" && remaining <= 1 {
                let source = window.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(80));
                    let _ = source.close();
                });
            }
            return Ok(TabDropOutcome::Adopted);
        }
    }
    if remaining <= 1 {
        // 窗口里唯一的标签拖到空白处没有意义（新窗口=原窗口），只支持并入其他窗口。
        return Ok(TabDropOutcome::None);
    }
    let monitor_scale = app
        .monitor_from_point(cursor.x, cursor.y)
        .ok()
        .flatten()
        .map(|monitor| monitor.scale_factor())
        .unwrap_or(1.0);
    // 新窗口的标签栏正好落在光标下方一点，观感接近浏览器的拖出。
    let position = (
        cursor.x / monitor_scale - 120.0,
        cursor.y / monitor_scale - 16.0,
    );
    // 拖出的窗口沿用来源窗口的尺寸。
    let size = window
        .inner_size()
        .ok()
        .map(|inner| {
            let scale = window.scale_factor().unwrap_or(1.0);
            (
                (inner.width as f64 / scale).max(620.0),
                (inner.height as f64 / scale).max(560.0),
            )
        });
    spawn_launcher_window(&app, state.inner(), Some(title), Some(position), size)?;
    Ok(TabDropOutcome::Detached)
}

#[tauri::command]
fn open_workspace(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    {
        let runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
        if runtime.phase != Phase::Ready {
            return Err("dsh Web 服务尚未就绪".into());
        }
    }
    if let Some(control) = app.get_webview_window("control") {
        control.show().map_err(|error| error.to_string())?;
        control.set_focus().map_err(|error| error.to_string())?;
    }
    let _ = app.emit("show-workspace", ());
    Ok(())
}

fn show_control(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("control") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开 DSH", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "启动设置", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart", "重启服务", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &settings, &restart, &quit])?;
    let mut tray = TrayIconBuilder::new().menu(&menu).tooltip("DSH Launcher");
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.on_menu_event(|app, event| match event.id().as_ref() {
        "open" => {
            let state = app.state::<AppState>();
            if open_workspace(app.clone(), state).is_err() {
                show_control(app);
            }
        }
        "settings" => show_control(app),
        "restart" => {
            let app_handle = app.clone();
            thread::spawn(move || {
                let state = app_handle.state::<AppState>().inner().clone();
                let _ = stop_process(&app_handle, &state);
                let _ = start_with_feedback(&app_handle, &state);
            });
        }
        "quit" => {
            let config = read_config(app).unwrap_or_default();
            if config.stop_dsh_on_exit {
                let state = app.state::<AppState>();
                let _ = stop_process(app, state.inner());
            }
            let app = app.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(120));
                app.exit(0);
            });
        }
        _ => {}
    })
    .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            load_config,
            default_config,
            save_config,
            save_window_size,
            get_status,
            start_dsh,
            stop_dsh,
            restart_dsh,
            clear_logs,
            open_workspace,
            list_config_files,
            save_dsh_config,
            open_dsh_config,
            get_package_info,
            update_dsh,
            get_launcher_version,
            check_launcher_update,
            list_plugins,
            search_plugins,
            fetch_market,
            open_external,
            install_plugin,
            remove_plugin,
            new_launcher_window,
            take_initial_tab,
            show_tab_drag_preview,
            move_tab_drag_preview,
            hide_tab_drag_preview,
            drop_tab
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            adopt_login_shell_path();
            setup_tray(app)?;
            // 主窗口也通过 builder 创建，这样它和分离出来的窗口都能挂载下载回调。
            spawn_launcher_window_named(
                &app.handle(),
                app.state::<AppState>().inner(),
                "control".into(),
                None,
                None,
                None,
            )?;
            setup_tab_drag_preview(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // 只有主窗口关闭时驻留托盘；新建的窗口正常销毁，
                // 否则每次“关闭”都只是隐藏，越积越多。
                if window.label() == "control" {
                    let config = read_config(&window.app_handle()).unwrap_or_default();
                    if matches!(config.close_behavior, CloseBehavior::Tray) {
                        api.prevent_close();
                        let _ = window.hide();
                    } else {
                        if config.stop_dsh_on_exit {
                            let state = window.app_handle().state::<AppState>();
                            let _ = stop_process(&window.app_handle(), state.inner());
                        }
                        // Let WebView2 finish its native window teardown before ending
                        // the tray-backed event loop. Immediate app.exit() can race
                        // Chromium's window-class cleanup (error 1412 on Windows).
                        let app = window.app_handle().clone();
                        thread::spawn(move || {
                            thread::sleep(Duration::from_millis(120));
                            app.exit(0);
                        });
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run DSH Launcher");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_authorities_without_shell_metacharacters() {
        assert!(is_safe_authority("localhost:3080"));
        assert!(is_safe_authority("[::1]:3080"));
        assert!(!is_safe_authority("host && calc"));
        assert!(!is_safe_authority(""));
    }

    #[test]
    fn validates_package_specs() {
        assert!(is_safe_package_spec("@deepseek-ai/dsh"));
        assert!(is_safe_package_spec("@deepseek-ai/dsh@latest"));
        assert!(!is_safe_package_spec("pkg; echo nope"));
    }

    #[test]
    fn truncates_diagnostic_lines_on_character_boundaries() {
        assert_eq!(truncate_chars("启动失败", 5), "启动失败");
        assert_eq!(truncate_chars("abcdef", 4), "abc…");
    }

    #[test]
    fn compares_release_tags_as_versions() {
        assert_eq!(release_version_key("v0.6.0"), release_version_key("0.6"));
        assert!(release_version_key("v0.6.1") > release_version_key("0.6.0"));
        assert!(!(release_version_key("0.6.0-beta") > release_version_key("0.6.0")));
    }

    #[test]
    fn detects_live_http_service_on_busy_port() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        let server = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                use std::io::Write;
                let mut buffer = [0u8; 1024];
                let _ = Read::read(&mut stream, &mut buffer);
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            }
        });
        assert!(http_service_alive(port));
        let _ = server.join();
    }

    #[test]
    fn treats_free_port_as_no_service() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        assert!(!http_service_alive(port));
    }

    #[test]
    fn ignores_listeners_that_close_without_responding() {
        // 模拟“端口被占但不是 Web 服务”：接受连接后立即断开。
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        let server = thread::spawn(move || {
            let _ = listener.accept();
        });
        assert!(!http_service_alive(port));
        let _ = server.join();
    }

    #[test]
    fn parses_market_catalog_api() {
        let json = r##"{"schemaVersion":1,"repositories":[{"repositoryId":1,"name":"dsh-web-ui","fullName":"o/dsh-web-ui","description":"d","url":"https://github.com/o/dsh-web-ui","owner":{"login":"o","avatarUrl":"https://avatars.githubusercontent.com/u/1"},"topics":["dsh-plugin"],"language":"TypeScript","stars":12,"pushedAt":"2026-08-14T00:00:00Z","archived":false,"projectType":"plugin","category":"ui","categories":["ui"],"validation":{"overall":"verified"}}]}"##;
        let catalog = parse_market_catalog(json).expect("catalog should parse");
        assert_eq!(catalog.plugins.len(), 1);
        let plugin = &catalog.plugins[0];
        assert_eq!(plugin.spec, "github:o/dsh-web-ui");
        assert_eq!(plugin.category, "ui");
        assert!(plugin.verified);
    }

    #[test]
    fn rejects_unknown_market_catalog_schema() {
        let error = parse_market_catalog(r##"{"schemaVersion":2,"repositories":[]}"##)
            .expect_err("unknown schema should be rejected");
        assert!(error.contains("schemaVersion=2"));
    }

    #[test]
    #[ignore = "network: fetches the live plugin market"]
    fn fetches_live_market_catalog() {
        let catalog = fetch_market_catalog().expect("live market fetch should succeed");
        assert!(!catalog.plugins.is_empty());
        assert!(catalog
            .plugins
            .iter()
            .all(|plugin| plugin.spec.starts_with("github:")));
    }

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
}
