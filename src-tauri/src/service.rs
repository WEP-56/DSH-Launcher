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
}
