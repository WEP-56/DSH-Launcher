use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::AppHandle;

use crate::{config::read_config, util::epoch_secs};

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

pub fn handle_download_request(
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
