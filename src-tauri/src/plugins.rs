use serde::Serialize;
use std::{fs, io::Read, path::PathBuf, thread, time::Duration};
use tauri::{AppHandle, State};

use crate::{
    config::{dsh_home_for, read_config, LauncherConfig},
    exec::{
        execute_command, npm_command, run_dsh_invocation, OperationResult, PLUGIN_ACTION_TIMEOUT,
        QUERY_TIMEOUT,
    },
    service::{start_with_feedback, stop_process},
    state::{busy_label, AppState, OpsBusy, Phase},
    util::version_key,
};

#[derive(Debug, Clone, Serialize)]
pub struct InstalledPlugin {
    pub name: String,
    /// profile package.json 里记录的依赖描述（npm 版本范围或 github:owner/repo）。
    pub version: String,
    pub bundle: bool,
    /// node_modules 里实际安装的版本，读不到时为空。
    pub installed_version: String,
    /// 安装渠道：npm / github / git / local / alias。
    pub channel: String,
    /// 传给 `dsh plugin add` 即可更新到最新的描述；本地链接等不可更新时为空。
    pub update_spec: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginSearchResult {
    pub name: String,
    pub version: String,
    pub description: String,
    pub homepage: String,
    pub npm_url: String,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PluginUpdateInfo {
    pub name: String,
    pub channel: String,
    pub installed_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub detail: String,
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

/// 依赖描述 → 安装渠道。npm 生态里 `owner/repo` 简写等价于 `github:owner/repo`。
fn plugin_channel(spec: &str) -> &'static str {
    let spec = spec.trim();
    if spec.starts_with("github:") {
        "github"
    } else if spec.starts_with("git+")
        || spec.starts_with("git:")
        || spec.starts_with("git@")
        || spec.contains("://")
    {
        "git"
    } else if spec.starts_with("file:")
        || spec.starts_with("link:")
        || spec.starts_with("workspace:")
        || spec.starts_with("portal:")
    {
        "local"
    } else if spec.starts_with("npm:") {
        "alias"
    } else if spec.contains('/') && !spec.starts_with('@') {
        "github"
    } else {
        "npm"
    }
}

fn plugin_update_spec(name: &str, spec: &str, channel: &str) -> String {
    match channel {
        // npm 包重装到 latest；范围描述（^x.y.z）留在原样只会更新到范围内。
        "npm" => format!("{name}@latest"),
        // 本地链接的插件没有“更新”概念。
        "local" => String::new(),
        // github/git/alias：重新 add 原描述即可重新解析到最新提交/范围内最新版。
        _ => spec.trim().to_string(),
    }
}

fn installed_manifest(config: &LauncherConfig, name: &str) -> Option<serde_json::Value> {
    let path = dsh_home_for(config)
        .join("profiles")
        .join("web")
        .join("node_modules")
        .join(name)
        .join("package.json");
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn installed_plugins(config: &LauncherConfig) -> Result<Vec<InstalledPlugin>, String> {
    let path = profile_manifest_path(config);
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
            let spec = spec.as_str().unwrap_or("*").to_string();
            let manifest = installed_manifest(config, name);
            let bundle = manifest.as_ref().is_some_and(|manifest| {
                manifest
                    .get("dsh")
                    .and_then(|dsh| dsh.get("bundle"))
                    .is_some()
            });
            let installed_version = manifest
                .as_ref()
                .and_then(|manifest| manifest.get("version"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let channel = plugin_channel(&spec);
            plugins.push(InstalledPlugin {
                name: name.clone(),
                update_spec: plugin_update_spec(name, &spec, channel),
                channel: channel.into(),
                version: spec,
                bundle,
                installed_version,
            });
        }
    }
    Ok(plugins)
}

#[tauri::command]
pub fn list_plugins(app: AppHandle) -> Result<Vec<InstalledPlugin>, String> {
    let config = read_config(&app)?;
    installed_plugins(&config)
}

fn http_get_json(url: &str) -> Result<serde_json::Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("dsh-launcher/", env!("CARGO_PKG_VERSION")))
        .build();
    let response = agent
        .get(url)
        .set("Accept", "application/json")
        .call()
        .map_err(|error| match error {
            ureq::Error::Status(status, _) => format!("HTTP {status}"),
            other => other.to_string(),
        })?;
    let mut body = String::new();
    response
        .into_reader()
        .take(2 * 1024 * 1024)
        .read_to_string(&mut body)
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&body).map_err(|error| error.to_string())
}

fn manifest_version(value: &serde_json::Value) -> Result<String, String> {
    value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "package.json 缺少 version 字段".into())
}

fn npm_latest_version(name: &str) -> Result<String, String> {
    let encoded = name.replace('/', "%2F");
    manifest_version(&http_get_json(&format!(
        "https://registry.npmjs.org/{encoded}/latest"
    ))?)
}

fn github_raw_manifest_url(spec: &str) -> Result<String, String> {
    let rest = spec.trim();
    let rest = rest.strip_prefix("github:").unwrap_or(rest);
    let (repo, reference) = match rest.split_once('#') {
        Some((repo, reference)) if !reference.trim().is_empty() => (repo, reference.trim()),
        Some((repo, _)) => (repo, "HEAD"),
        None => (rest, "HEAD"),
    };
    let mut parts = repo.split('/');
    let (owner, name) = (
        parts.next().unwrap_or_default().trim(),
        parts.next().unwrap_or_default().trim(),
    );
    let safe = |value: &str| {
        !value.is_empty()
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "._-".contains(ch))
    };
    if !safe(owner) || !safe(name) || parts.next().is_some() {
        return Err(format!("无法解析仓库地址：{spec}"));
    }
    Ok(format!(
        "https://raw.githubusercontent.com/{owner}/{name}/{reference}/package.json"
    ))
}

fn github_manifest_version(spec: &str) -> Result<String, String> {
    manifest_version(&http_get_json(&github_raw_manifest_url(spec)?)?)
}

fn check_plugin_update(plugin: &InstalledPlugin) -> PluginUpdateInfo {
    let mut info = PluginUpdateInfo {
        name: plugin.name.clone(),
        channel: plugin.channel.clone(),
        installed_version: plugin.installed_version.clone(),
        latest_version: String::new(),
        update_available: false,
        detail: String::new(),
    };
    let latest = match plugin.channel.as_str() {
        "npm" => npm_latest_version(&plugin.name),
        "github" => github_manifest_version(&plugin.version),
        "local" => Err("本地链接的插件不参与在线更新".into()),
        _ => Err("暂不支持检测该来源的最新版本，可直接用更新按钮重装".into()),
    };
    match latest {
        Ok(latest) => {
            info.update_available = !plugin.installed_version.is_empty()
                && version_key(&latest) > version_key(&plugin.installed_version);
            info.detail = if info.update_available {
                format!("可从 {} 更新到 {latest}", plugin.installed_version)
            } else if plugin.installed_version.is_empty() {
                "读不到本地安装版本，无法比较；可用更新按钮直接重装".into()
            } else if plugin.channel == "github" {
                "仓库 package.json 版本与本地一致；若仓库有未发版的新提交，可用更新按钮强制重装"
                    .into()
            } else {
                "已是最新".into()
            };
            info.latest_version = latest;
        }
        Err(error) => info.detail = format!("检查失败：{error}"),
    }
    info
}

#[tauri::command]
pub async fn check_plugin_updates(app: AppHandle) -> Result<Vec<PluginUpdateInfo>, String> {
    let config = read_config(&app)?;
    let plugins = installed_plugins(&config)?;
    // 每个插件各查各的（npm registry / GitHub raw），并行以免逐个 10 秒超时串行累加。
    let handles: Vec<_> = plugins
        .into_iter()
        .map(|plugin| thread::spawn(move || check_plugin_update(&plugin)))
        .collect();
    Ok(handles
        .into_iter()
        .filter_map(|handle| handle.join().ok())
        .collect())
}

#[tauri::command]
pub async fn search_plugins(query: String) -> Result<Vec<PluginSearchResult>, String> {
    let term = if query.trim().is_empty() {
        "dsh-plugin".to_string()
    } else {
        format!("{} dsh-plugin", query.trim())
    };
    let mut npm = npm_command();
    npm.args(["search", "--json", "--searchlimit", "30", &term]);
    let result = execute_command(npm, QUERY_TIMEOUT)?;
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

fn run_plugin_actions(
    app: &AppHandle,
    state: &AppState,
    label: &str,
    actions: &[(&str, String)],
) -> Result<OperationResult, String> {
    if actions.is_empty() {
        return Err("没有需要执行的插件操作".into());
    }
    for (_, spec) in actions {
        if !is_safe_plugin_spec(spec.trim()) {
            return Err("插件标识包含不支持的字符".into());
        }
    }
    let config = read_config(app)?;
    // 串行化插件/更新操作：并发的 dsh/npm 进程会同时改写同一份 profile
    // package.json 和 node_modules。已有操作在进行时直接报错，而不是排队
    // 等几分钟后突然又停一次服务（单窗口有前端防重入，这里挡的是多窗口）。
    let Ok(_ops) = state.ops.try_lock() else {
        let running = busy_label(state).unwrap_or_else(|| "另一项插件或更新操作正在进行".into());
        return Err(format!("{running}，请等它完成后再执行其他操作"));
    };
    let _busy = OpsBusy::begin(app, state, label);
    // 外部服务不归我们停/启，插件操作只改 profile 文件，改完由用户自行重启生效。
    let was_ready = {
        let runtime = state.runtime.lock().map_err(|_| "启动器状态锁已损坏")?;
        runtime.phase == Phase::Ready && !runtime.external
    };
    if was_ready {
        stop_process(app, state)?;
    }
    let mut sections = Vec::new();
    let mut all_ok = true;
    for (verb, spec) in actions {
        let spec = spec.trim();
        let args = vec![
            "plugin".into(),
            "--profile".into(),
            "web".into(),
            (*verb).into(),
            spec.into(),
        ];
        let header = format!("$ dsh plugin --profile web {verb} {spec}");
        match run_dsh_invocation(&config, &args, PLUGIN_ACTION_TIMEOUT) {
            Ok(result) => {
                all_ok &= result.success;
                let body = if result.output.is_empty() {
                    if result.success {
                        "（完成）"
                    } else {
                        "（失败）"
                    }
                    .to_string()
                } else {
                    result.output
                };
                sections.push(format!("{header}\n{body}"));
            }
            Err(error) => {
                all_ok = false;
                sections.push(format!("{header}\n{error}"));
            }
        }
    }
    // 无论操作是否成功，只要之前在运行就把服务拉回来，
    // 不能让一次失败的插件操作把服务留在停止状态。
    if was_ready {
        if let Err(error) = start_with_feedback(app, state) {
            sections.push(format!(
                "注意：dsh 服务恢复启动失败：{error}（可回主界面手动启动）"
            ));
        }
    }
    Ok(OperationResult {
        success: all_ok,
        output: sections.join("\n\n"),
    })
}

#[tauri::command]
pub async fn install_plugin(
    app: AppHandle,
    state: State<'_, AppState>,
    spec: String,
) -> Result<OperationResult, String> {
    run_plugin_actions(&app, state.inner(), "正在安装插件", &[("add", spec)])
}

#[tauri::command]
pub async fn remove_plugin(
    app: AppHandle,
    state: State<'_, AppState>,
    spec: String,
) -> Result<OperationResult, String> {
    run_plugin_actions(&app, state.inner(), "正在卸载插件", &[("remove", spec)])
}

#[tauri::command]
pub async fn update_plugins(
    app: AppHandle,
    state: State<'_, AppState>,
    specs: Vec<String>,
) -> Result<OperationResult, String> {
    // 批量更新共用一次服务停/启，避免每个插件都重启一遍。
    let actions: Vec<(&str, String)> = specs.into_iter().map(|spec| ("add", spec)).collect();
    run_plugin_actions(&app, state.inner(), "正在更新插件", &actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_plugin_specs_by_channel() {
        assert_eq!(plugin_channel("github:owner/repo"), "github");
        assert_eq!(plugin_channel("github:owner/repo#dev"), "github");
        assert_eq!(plugin_channel("owner/repo"), "github");
        assert_eq!(plugin_channel("^0.11.0"), "npm");
        assert_eq!(plugin_channel("latest"), "npm");
        assert_eq!(plugin_channel("git+https://example.com/x.git"), "git");
        assert_eq!(plugin_channel("https://example.com/x.tgz"), "git");
        assert_eq!(plugin_channel("file:../local-plugin"), "local");
        assert_eq!(plugin_channel("workspace:*"), "local");
        assert_eq!(plugin_channel("npm:other-name@^1"), "alias");
    }

    #[test]
    fn builds_update_specs_per_channel() {
        assert_eq!(
            plugin_update_spec("dsh-better-sidebar", "^0.11.0", "npm"),
            "dsh-better-sidebar@latest"
        );
        assert_eq!(
            plugin_update_spec("dsh-at-file", "github:o/dsh-at-file", "github"),
            "github:o/dsh-at-file"
        );
        assert_eq!(plugin_update_spec("local-dev", "file:../x", "local"), "");
    }

    #[test]
    fn builds_github_raw_manifest_urls() {
        assert_eq!(
            github_raw_manifest_url("github:o/r").unwrap(),
            "https://raw.githubusercontent.com/o/r/HEAD/package.json"
        );
        assert_eq!(
            github_raw_manifest_url("github:o/r#dev").unwrap(),
            "https://raw.githubusercontent.com/o/r/dev/package.json"
        );
        assert_eq!(
            github_raw_manifest_url("o/r").unwrap(),
            "https://raw.githubusercontent.com/o/r/HEAD/package.json"
        );
        assert!(github_raw_manifest_url("github:only-owner").is_err());
        assert!(github_raw_manifest_url("github:o/r/extra").is_err());
    }

    #[test]
    fn rejects_unsafe_plugin_specs() {
        assert!(is_safe_plugin_spec("github:owner/repo#main"));
        assert!(is_safe_plugin_spec("@deepseek-ai/dsh-plugin@latest"));
        assert!(!is_safe_plugin_spec("pkg; rm -rf /"));
        assert!(!is_safe_plugin_spec(""));
    }
}
