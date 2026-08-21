use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    io::{BufRead, BufReader, Read},
    process::Child,
    sync::{Arc, Mutex},
    thread,
};
use tauri::{AppHandle, Emitter, State};

use crate::market::MarketCatalog;

pub const MAX_LOG_LINES: usize = 400;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Stopped,
    Starting,
    Ready,
    Stopping,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct LauncherStatus {
    pub phase: Phase,
    pub message: String,
    pub url: String,
    pub pid: Option<u32>,
    pub external: bool,
    pub logs: Vec<String>,
    pub busy: Option<String>,
}

pub struct RuntimeState {
    pub phase: Phase,
    pub message: String,
    pub url: String,
    pub child: Option<Child>,
    pub pid: Option<u32>,
    // 端口上的服务由用户在 Launcher 之外启动：只沿用不接管，停止/退出都不碰它。
    pub external: bool,
    pub logs: VecDeque<String>,
    pub generation: u64,
    // 互斥操作（插件安装/卸载/更新、dsh 更新）进行中的描述文字。期间前端
    // 禁用启动入口，后端 start_dsh/restart_dsh 也会拒绝手动启动（防呆：
    // 这些操作会先停服务再改写安装目录，中途启动会跑在半成品目录上）。
    pub busy: Option<String>,
    // 收到过多少行子进程输出。启动守护用它判断 dsh 是否还在干活（升级后首次
    // 启动要现装 profile 依赖，几分钟不监听端口但一直在刷日志），只要还有新
    // 输出就不能当成卡死。
    pub log_ticks: u64,
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
            busy: None,
            log_ticks: 0,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<Mutex<RuntimeState>>,
    pub market: Arc<Mutex<Option<MarketCatalog>>>,
    // 拖出标签新建窗口时，按窗口 label 暂存它要带走的标签标题。
    pub pending_tabs: Arc<Mutex<HashMap<String, String>>>,
    // 串行化会改写安装目录/profile 的互斥操作（插件安装/卸载/更新、dsh 更新），
    // 防止并发的 npm/dsh 进程互相破坏同一份 package.json 或服务停启状态。
    pub ops: Arc<Mutex<()>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            runtime: Arc::new(Mutex::new(RuntimeState::default())),
            market: Arc::new(Mutex::new(None)),
            pending_tabs: Arc::new(Mutex::new(HashMap::new())),
            ops: Arc::new(Mutex::new(())),
        }
    }
}

pub fn snapshot(runtime: &RuntimeState) -> LauncherStatus {
    LauncherStatus {
        phase: runtime.phase,
        message: runtime.message.clone(),
        url: runtime.url.clone(),
        pid: runtime.pid,
        external: runtime.external,
        logs: runtime.logs.iter().cloned().collect(),
        busy: runtime.busy.clone(),
    }
}

pub fn emit_status(app: &AppHandle, state: &AppState) {
    if let Ok(runtime) = state.runtime.lock() {
        let _ = app.emit("launcher-status", snapshot(&runtime));
    }
}

pub fn push_log(app: &AppHandle, state: &AppState, generation: u64, source: &str, line: String) {
    if let Ok(mut runtime) = state.runtime.lock() {
        if runtime.generation != generation {
            return;
        }
        runtime.logs.push_back(format!("[{source}] {line}"));
        runtime.log_ticks = runtime.log_ticks.wrapping_add(1);
        while runtime.logs.len() > MAX_LOG_LINES {
            runtime.logs.pop_front();
        }
    }
    emit_status(app, state);
}

pub fn spawn_log_reader<R>(
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

pub fn current_status(state: AppState) -> Result<LauncherStatus, String> {
    state
        .runtime
        .lock()
        .map(|runtime| snapshot(&runtime))
        .map_err(|_| "启动器状态锁已损坏".into())
}

pub fn busy_label(state: &AppState) -> Option<String> {
    state
        .runtime
        .lock()
        .ok()
        .and_then(|runtime| runtime.busy.clone())
}

/// 手动启动 dsh 被互斥操作挡下时给用户的解释。
pub fn ops_in_progress_error(state: &AppState) -> String {
    let label = busy_label(state).unwrap_or_else(|| "有插件或更新操作正在进行".into());
    format!("{label}，完成后会自动恢复服务；请等待操作结束")
}

/// RAII：互斥操作期间把 busy 描述广播给所有窗口，无论操作从哪条路径返回
/// （包括提前出错），Drop 都会清掉标记再广播一次，不会把界面卡在“维护中”。
pub struct OpsBusy {
    app: AppHandle,
    state: AppState,
}

impl OpsBusy {
    pub fn begin(app: &AppHandle, state: &AppState, label: &str) -> Self {
        if let Ok(mut runtime) = state.runtime.lock() {
            runtime.busy = Some(label.into());
        }
        emit_status(app, state);
        Self {
            app: app.clone(),
            state: state.clone(),
        }
    }
}

impl Drop for OpsBusy {
    fn drop(&mut self) {
        if let Ok(mut runtime) = self.state.runtime.lock() {
            runtime.busy = None;
        }
        emit_status(&self.app, &self.state);
    }
}

#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> Result<LauncherStatus, String> {
    current_status(state.inner().clone())
}

#[tauri::command]
pub fn clear_logs(state: State<'_, AppState>) -> Result<LauncherStatus, String> {
    let mut runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
    runtime.logs.clear();
    Ok(snapshot(&runtime))
}
