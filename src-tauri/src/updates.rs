use serde::Serialize;
use std::{io::Read, time::Duration};
use tauri::{AppHandle, State};

use crate::{
    config::{read_config, LaunchMode, LauncherConfig},
    exec::{
        execute_command, npm_command, prepare_command, run_capture, run_dsh_invocation,
        OperationResult, GLOBAL_INSTALL_TIMEOUT, QUERY_TIMEOUT,
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

/// 解析 `npm view <pkg> versions --json` 的 stdout。
/// 包里只有一个版本时 npm 输出的是单个字符串而不是数组，两种都要认。
fn parse_versions(stdout: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|error| format!("npm 返回的版本列表无法解析：{error}"))?;
    let versions = match value {
        serde_json::Value::String(single) => vec![single],
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| match item {
                serde_json::Value::String(version) => Some(version),
                _ => None,
            })
            .collect(),
        _ => return Err("npm 返回的版本列表格式不认识".into()),
    };
    if versions.is_empty() {
        return Err("npm 没有返回任何已发布版本".into());
    }
    Ok(versions)
}

/// 从全部已发布版本里挑出真正最新的一个。
///
/// 不能只信 `latest` dist-tag：dsh 发 rc 版本时并不移动 `latest`
/// （0.1.0-rc.8、0.1.1-rc.1 发出来之后 `latest` 仍指向 0.1.0-rc.7），
/// 于是 `npm view <pkg> version` 与 `npm install <pkg>@latest` 都会一直
/// 停在旧版本 —— 这就是 issue #5 “拉不到最新的 dsh 版本”。
///
/// 有正式版时只在正式版里挑，不会把用户推到预发布版上；
/// 一个正式版都没有（dsh 目前如此）才回落到最新的预发布版。
pub fn newest_version(versions: &[String]) -> Option<String> {
    let newest = |only_release: bool| {
        versions
            .iter()
            .filter(|version| !only_release || version_key(version).is_release())
            .max_by(|left, right| version_key(left).cmp(&version_key(right)))
            .cloned()
    };
    newest(true).or_else(|| newest(false))
}

/// 向 npm 查询包的全部已发布版本，并挑出最新的那个。
/// 走 `npm view` 而不是直连 registry.npmjs.org，是为了沿用用户配置的镜像源。
fn resolve_latest_version(config: &LauncherConfig) -> Result<String, String> {
    let mut npm = npm_command();
    npm.args(["view", &config.npx_package, "versions", "--json"]);
    let capture = run_capture(prepare_command(npm, config), QUERY_TIMEOUT)?;
    if capture.timed_out {
        return Err(format!(
            "查询 npm 版本超过 {} 秒未结束，已强制终止",
            QUERY_TIMEOUT.as_secs()
        ));
    }
    if !capture.success {
        let reason = capture.stderr.trim();
        return Err(if reason.is_empty() {
            "npm 查询版本列表失败".into()
        } else {
            format!("npm 查询版本列表失败：{reason}")
        });
    }
    let versions = parse_versions(&capture.stdout)?;
    newest_version(&versions).ok_or_else(|| "版本列表里没有可用的版本号".into())
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
    let latest = resolve_latest_version(&config);
    let current_version = if current.success {
        extract_version(&current.output)
    } else {
        "不可用".into()
    };
    let (latest_version, latest_detail) = match &latest {
        Ok(version) => (version.clone(), format!("npm 已发布的最新版本：{version}")),
        Err(error) => ("查询失败".into(), error.clone()),
    };
    Ok(PackageInfo {
        current_version,
        latest_version,
        source: match config.launch_mode {
            LaunchMode::Command => config.executable,
            LaunchMode::Npx => format!("npx {}", config.npx_package),
        },
        checked_at: epoch_secs().to_string(),
        detail: [current.output, latest_detail]
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
    // 装解析出来的确切版本号，而不是 `@latest`：dsh 的 rc 版本不移动 `latest`
    // dist-tag，装 `@latest` 会一直停在旧版本（issue #5）。解析失败时退回
    // `@latest`，至少保持原来的行为可用，并把原因写进结果里。
    let (spec, notice) = match resolve_latest_version(&config) {
        Ok(version) => (format!("{}@{version}", config.npx_package), None),
        Err(error) => (
            format!("{}@latest", config.npx_package),
            Some(format!(
                "无法确定最新版本（{error}），退回安装 latest 标签指向的版本。"
            )),
        ),
    };
    let mut npm = npm_command();
    npm.args(["install", "--global", &spec]);
    let result = execute_command(prepare_command(npm, &config), GLOBAL_INSTALL_TIMEOUT);
    if was_ready {
        let _ = start_with_feedback(&app, state.inner());
    }
    let Some(notice) = notice else {
        return result;
    };
    result.map(|mut result| {
        result.output = if result.output.is_empty() {
            notice
        } else {
            format!("{notice}\n{}", result.output)
        };
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dsh 截至 issue #5 时的真实版本列表：`latest` dist-tag 仍指向
    /// 0.1.0-rc.7，但 registry 上已经有 0.1.0-rc.8 和 0.1.1-rc.1。
    const DSH_VERSIONS: &[&str] = &[
        "0.0.1-rc.1",
        "0.0.1-rc.2",
        "0.0.1-rc.5",
        "0.1.0-rc.2",
        "0.1.0-rc.3",
        "0.1.0-rc.6",
        "0.1.0-rc.7",
        "0.1.0-rc.8",
        "0.1.1-rc.1",
    ];

    fn owned(versions: &[&str]) -> Vec<String> {
        versions.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn picks_newest_prerelease_when_no_release_exists() {
        assert_eq!(
            newest_version(&owned(DSH_VERSIONS)).as_deref(),
            Some("0.1.1-rc.1")
        );
    }

    #[test]
    fn prefers_releases_over_newer_prereleases() {
        let mut versions = owned(DSH_VERSIONS);
        versions.push("0.1.0".into());
        // 有正式版就不再把用户推到 0.1.1-rc.1 这样的预发布版上。
        assert_eq!(newest_version(&versions).as_deref(), Some("0.1.0"));
    }

    #[test]
    fn compares_prerelease_numbers_numerically() {
        let versions = owned(&["0.1.0-rc.8", "0.1.0-rc.9", "0.1.0-rc.10"]);
        assert_eq!(newest_version(&versions).as_deref(), Some("0.1.0-rc.10"));
    }

    #[test]
    fn reports_no_version_for_an_empty_list() {
        assert_eq!(newest_version(&[]), None);
    }

    #[test]
    fn parses_both_array_and_single_string_version_output() {
        assert_eq!(
            parse_versions("[\n  \"0.1.0-rc.7\",\n  \"0.1.0-rc.8\"\n]").expect("array"),
            vec!["0.1.0-rc.7".to_string(), "0.1.0-rc.8".to_string()]
        );
        // 包里只有一个版本时 npm 输出裸字符串。
        assert_eq!(
            parse_versions("\"0.1.0-rc.8\"").expect("single string"),
            vec!["0.1.0-rc.8".to_string()]
        );
    }

    #[test]
    fn rejects_unparseable_version_output() {
        assert!(parse_versions("npm error code E404").is_err());
        assert!(parse_versions("[]").is_err());
        assert!(parse_versions("{}").is_err());
    }
}
