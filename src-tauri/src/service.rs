use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, State};

use crate::{
    config::{read_config, validate_config, LaunchMode, LauncherConfig},
    exec::{command_for_invocation, kill_child_tree},
    state::{
        current_status, emit_status, ops_in_progress_error, snapshot, spawn_log_reader, AppState,
        LauncherStatus, Phase,
    },
    util::truncate_chars,
};

/// `dsh web` 的命令行参数。抽出来是为了能在测试里检查参数拼装，
/// 不必真的去执行 dsh。
///
/// 这里刻意不传 `--no-open`（dsh web 默认会额外弹一个系统浏览器窗口，对
/// 内嵌页面的 Launcher 是多余的）：该开关是 `@deepseek-ai/dsh-web-app`
/// 0.1.0-rc.8 才加的，rc.7 及更早的 startup.js 里没有，而 dsh 的 commander
/// 没开 allowUnknownOption —— 对还没升级的用户传过去会让 dsh 直接以
/// “unknown option” 退出，等于把“多一个浏览器窗口”换成“根本起不来”。
/// 等 dsh 的正式版普及后再加。
fn web_args(config: &LauncherConfig) -> Vec<String> {
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
    args
}

fn command_for_config(config: &LauncherConfig) -> Result<Command, String> {
    command_for_invocation(config, &web_args(config))
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
        runtime.message =
            format!("已连接到端口 {port} 上已在运行的 dsh 服务（外部启动，Launcher 不会停止它）");
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

pub fn start_process(app: AppHandle, state: AppState) -> Result<LauncherStatus, String> {
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

/// 一直有输出但始终不监听端口时的总时长上限。升级后首次启动要现装
/// `~/.dsh/profiles/web` 的依赖，慢是正常的，但不能无限等下去。
const STARTUP_HARD_LIMIT: Duration = Duration::from_secs(900);

/// 启动超过这么久还没就绪就把“可能在装依赖”的提示写进状态栏。
const SLOW_START_HINT_AFTER: Duration = Duration::from_secs(45);

/// 启动守护对当前等待状态的判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupWait {
    /// 还在合理范围内，继续等。
    Keep,
    /// 太久没有任何输出，判为卡死。
    Silent,
    /// 一直在输出却始终不监听端口，超过总时长上限。
    Exhausted,
}

/// 用“静默了多久”而不是“总共等了多久”判断启动是否卡死。
///
/// dsh 发生破坏性升级（如 0.1.0-rc.8）后首次 `dsh web` 会现装 profile 依赖，
/// 好几分钟不监听端口但一直在刷安装日志；按固定 60 秒总时长判超时会把它
/// 强杀掉，还可能留下半装好的 profile，导致之后每次启动都失败 —— 这就是
/// issue #5 里“手动更新后貌似也无法启动了”。只要还有新输出就不算卡死，
/// 另用 STARTUP_HARD_LIMIT 兜底。
fn classify_startup_wait(
    silent_for: Duration,
    elapsed: Duration,
    silence_budget: Duration,
) -> StartupWait {
    if silent_for > silence_budget {
        StartupWait::Silent
    } else if elapsed > STARTUP_HARD_LIMIT {
        StartupWait::Exhausted
    } else {
        StartupWait::Keep
    }
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
    // 最近一次看到子进程有新输出的时刻，以及当时的输出行数。
    let mut last_active = Instant::now();
    let mut seen_ticks = 0u64;
    // “还在装依赖”的提示只播一次。
    let mut hinted = false;
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
            if runtime.log_ticks != seen_ticks {
                seen_ticks = runtime.log_ticks;
                last_active = Instant::now();
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
        let verdict =
            classify_startup_wait(last_active.elapsed(), started.elapsed(), startup_timeout);
        if verdict == StartupWait::Keep {
            // 起得慢又还在刷日志时告诉用户在等什么，否则界面只写“等待响应”，
            // 用户会以为卡死了（升级后首次启动装依赖要好几分钟）。
            if !hinted && started.elapsed() > SLOW_START_HINT_AFTER {
                hinted = true;
                if let Ok(mut runtime) = state.runtime.lock() {
                    if runtime.generation != generation || runtime.phase != Phase::Starting {
                        return;
                    }
                    runtime.message = format!(
                        "dsh 仍在启动，还在输出日志（升级后首次启动要现装依赖，可能要几分钟）：等待 {} 响应…",
                        runtime.url
                    );
                }
                emit_status(&app, &state);
            }
            continue;
        }
        {
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
                runtime.message = match verdict {
                    StartupWait::Silent => format!(
                        "dsh 启动超时（{} 秒内既没有监听端口 {port}，也没有任何新输出），已终止进程，请检查运行日志",
                        startup_timeout.as_secs()
                    ),
                    _ => format!(
                        "dsh 启动超过 {} 分钟仍未监听端口 {port}，已终止进程，请检查运行日志",
                        STARTUP_HARD_LIMIT.as_secs() / 60
                    ),
                };
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

pub fn stop_process(app: &AppHandle, state: &AppState) -> Result<LauncherStatus, String> {
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

pub fn start_with_feedback(app: &AppHandle, state: &AppState) -> Result<LauncherStatus, String> {
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
pub async fn start_dsh(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LauncherStatus, String> {
    // 防呆：插件/更新操作会先停服务再改写安装目录，期间手动启动会让 dsh
    // 跑在改到一半的目录上；操作结束后会自动恢复服务，这里直接拒绝。
    // 持有守卫到启动完成，同样挡住反过来的竞争（启动进行中来了插件操作会排队）。
    let Ok(_ops) = state.ops.try_lock() else {
        return Err(ops_in_progress_error(state.inner()));
    };
    start_with_feedback(&app, state.inner())
}

#[tauri::command]
pub async fn stop_dsh(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LauncherStatus, String> {
    stop_process(&app, state.inner())
}

#[tauri::command]
pub async fn restart_dsh(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LauncherStatus, String> {
    let Ok(_ops) = state.ops.try_lock() else {
        return Err(ops_in_progress_error(state.inner()));
    };
    stop_process(&app, state.inner())?;
    start_with_feedback(&app, state.inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn detects_live_http_service_on_busy_port() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        let server = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                use std::io::Write;
                let mut buffer = [0u8; 1024];
                let _ = Read::read(&mut stream, &mut buffer);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                );
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
    fn keeps_waiting_while_dsh_is_still_logging() {
        let budget = Duration::from_secs(60);
        // 升级后首次启动：五分钟没监听端口，但一直有新输出 —— 不能判超时。
        assert_eq!(
            classify_startup_wait(Duration::from_secs(3), Duration::from_secs(300), budget),
            StartupWait::Keep
        );
    }

    #[test]
    fn fails_when_dsh_goes_quiet_without_listening() {
        let budget = Duration::from_secs(60);
        assert_eq!(
            classify_startup_wait(Duration::from_secs(61), Duration::from_secs(61), budget),
            StartupWait::Silent
        );
    }

    #[test]
    fn stops_waiting_after_the_hard_limit_even_if_still_logging() {
        let budget = Duration::from_secs(60);
        assert_eq!(
            classify_startup_wait(
                Duration::from_secs(1),
                STARTUP_HARD_LIMIT + Duration::from_secs(1),
                budget
            ),
            StartupWait::Exhausted
        );
    }

    #[test]
    fn builds_web_args_that_older_dsh_versions_still_accept() {
        // 只检查参数拼装，不真正执行 dsh。
        let config = LauncherConfig {
            port: 3081,
            trusted_hosts: vec!["example.test".into()],
            ..Default::default()
        };
        let args = web_args(&config);
        assert_eq!(
            args,
            vec![
                "web",
                "--host",
                "127.0.0.1",
                "--port",
                "3081",
                "--trusted-host",
                "example.test",
            ]
        );
        // rc.7 及更早的 dsh 不认这个开关，传了会直接以 unknown option 退出。
        assert!(!args.contains(&"--no-open".to_string()));
    }
}
