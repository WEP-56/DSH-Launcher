use std::process::Command;

pub fn truncate_chars(value: &str, limit: usize) -> String {
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

pub fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 把版本号拆成可比较的数字序列（忽略 v 前缀与预发布后缀的非数字部分），
/// Launcher 自身、dsh 包与插件的版本比较共用。
pub fn version_key(value: &str) -> Vec<u64> {
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
pub fn open_external(url: String) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("只允许打开 http(s) 链接".into());
    }
    #[cfg(windows)]
    {
        let mut command = Command::new("explorer.exe");
        crate::exec::hide_console(&mut command);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_diagnostic_lines_on_character_boundaries() {
        assert_eq!(truncate_chars("启动失败", 5), "启动失败");
        assert_eq!(truncate_chars("abcdef", 4), "abc…");
    }

    #[test]
    fn compares_release_tags_as_versions() {
        assert_eq!(version_key("v0.6.0"), version_key("0.6"));
        assert!(version_key("v0.6.1") > version_key("0.6.0"));
        assert!(!(version_key("0.6.0-beta") > version_key("0.6.0")));
    }
}
