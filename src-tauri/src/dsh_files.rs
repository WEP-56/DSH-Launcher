use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tauri::AppHandle;

use crate::config::{dsh_home_for, read_config, LauncherConfig};

#[derive(Debug, Clone, Serialize)]
pub struct ConfigFileInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub content: String,
    pub editable: bool,
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
pub fn list_config_files(app: AppHandle) -> Result<Vec<ConfigFileInfo>, String> {
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
pub fn save_dsh_config(
    app: AppHandle,
    id: String,
    content: String,
) -> Result<ConfigFileInfo, String> {
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
pub fn open_dsh_config(app: AppHandle, id: String) -> Result<(), String> {
    let config = read_config(&app)?;
    let (_, _, path) =
        config_file_path(&config, &id).ok_or_else(|| "未知的 dsh 配置文件".to_string())?;
    let target = if path.exists() {
        path
    } else {
        path.parent().unwrap_or(Path::new(".")).to_path_buf()
    };
    #[cfg(windows)]
    {
        Command::new("explorer.exe")
            .arg(target)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(windows))]
    {
        Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
