use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    Command,
    Npx,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloseBehavior {
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
pub struct LauncherConfig {
    pub launch_mode: LaunchMode,
    pub executable: String,
    pub npx_package: String,
    pub working_directory: String,
    pub dsh_home: String,
    pub port: u16,
    pub trusted_hosts: Vec<String>,
    pub auto_start: bool,
    pub open_on_ready: bool,
    pub close_behavior: CloseBehavior,
    pub stop_dsh_on_exit: bool,
    pub download_directory: String,
    pub download_ask: bool,
    pub download_choose_location: bool,
    pub auto_check_updates: bool,
    pub window_width: u32,
    pub window_height: u32,
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

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join("config.json"))
        .map_err(|error| format!("无法确定配置目录：{error}"))
}

pub fn read_config(app: &AppHandle) -> Result<LauncherConfig, String> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(LauncherConfig::default());
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("配置文件格式无效：{error}"))
}

pub fn validate_config(config: &LauncherConfig) -> Result<(), String> {
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

pub fn is_safe_package_spec(value: &str) -> bool {
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

pub fn save_config_file(app: &AppHandle, config: &LauncherConfig) -> Result<(), String> {
    validate_config(config)?;
    let path = config_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录：{error}"))?;
    }
    let text = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    fs::write(&path, text).map_err(|error| format!("无法写入 {}：{error}", path.display()))
}

pub fn dsh_home_for(config: &LauncherConfig) -> PathBuf {
    if !config.dsh_home.trim().is_empty() {
        return PathBuf::from(config.dsh_home.trim());
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dsh")
}

#[tauri::command]
pub fn load_config(app: AppHandle) -> Result<LauncherConfig, String> {
    read_config(&app)
}

#[tauri::command]
pub fn default_config() -> LauncherConfig {
    LauncherConfig::default()
}

#[tauri::command]
pub fn save_config(app: AppHandle, config: LauncherConfig) -> Result<LauncherConfig, String> {
    save_config_file(&app, &config)?;
    Ok(config)
}

#[tauri::command]
pub fn save_window_size(app: AppHandle, width: u32, height: u32) -> Result<(), String> {
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
}
