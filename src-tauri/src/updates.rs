use serde::Serialize;
use std::{io::Read, time::Duration};
use tauri::{AppHandle, State};

use crate::{
    config::{read_config, LaunchMode},
    exec::{
        execute_command, npm_command, prepare_command, run_dsh_invocation, OperationResult,
        GLOBAL_INSTALL_TIMEOUT, QUERY_TIMEOUT,
    },
    service::{start_with_feedback, stop_process},
    state::{busy_label, AppState, OpsBusy, Phase},
    util::{epoch_secs, version_key},
};

const LAUNCHER_RELEASE_API: &str =
    "https://api.github.com/repos/WEP-56/DSH-Launcher/releases/latest";

#[derive(Debug, Clone, Serialize)]
pub struct PackageInfo {
    pub current_version: String,
    pub latest_version: String,
    pub source: String,
    pub checked_at: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReleaseInfo {
    pub current_version: String,
    pub latest_version: String,
    pub tag_name: String,
    pub name: String,
    pub body: String,
    pub html_url: String,
    pub published_at: String,
    pub update_available: bool,
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
pub fn get_launcher_version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

#[tauri::command]
pub async fn check_launcher_update() -> Result<ReleaseInfo, String> {
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
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|error| format!("GitHub Release 响应无效：{error}"))?;
    let text = |key: &str| {
        value
            .get(key)
            .and_then(|item| item.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let tag_name = text("tag_name");
    if tag_name.is_empty() {
        return Err("GitHub 没有可用的最新 Release".into());
    }
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let latest_version = tag_name.trim_start_matches(['v', 'V']).to_string();
    let update_available = version_key(&latest_version) > version_key(&current_version);
    Ok(ReleaseInfo {
        current_version,
        latest_version,
        tag_name,
        name: text("name"),
        body: text("body"),
        html_url: text("html_url"),
        published_at: text("published_at"),
        update_available,
    })
}

#[tauri::command]
pub async fn get_package_info(app: AppHandle) -> Result<PackageInfo, String> {
    let config = read_config(&app)?;
    let version_args = vec!["--version".to_string()];
    let current = run_dsh_invocation(&config, &version_args, QUERY_TIMEOUT)?;
    let mut npm = npm_command();
    npm.args(["view", &config.npx_package, "version", "--json"]);
    let latest = execute_command(prepare_command(npm, &config), QUERY_TIMEOUT)?;
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
pub async fn update_dsh(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<OperationResult, String> {
    let config = read_config(&app)?;
    // 与插件操作共用互斥锁：更新期间不允许并发的插件安装/卸载改同一套目录。
    let Ok(_ops) = state.ops.try_lock() else {
        let running =
            busy_label(state.inner()).unwrap_or_else(|| "另一项插件或更新操作正在进行".into());
        return Err(format!("{running}，请等它完成后再更新 dsh"));
    };
    let _busy = OpsBusy::begin(&app, state.inner(), "正在更新 dsh");
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
    let result = execute_command(prepare_command(npm, &config), GLOBAL_INSTALL_TIMEOUT);
    if was_ready {
        let _ = start_with_feedback(&app, state.inner());
    }
    result
}
