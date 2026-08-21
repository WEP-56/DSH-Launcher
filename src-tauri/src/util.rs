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

/// 预发布标识里的一段。数字段优先级低于字母段（语义化版本规则），
/// 所以 Numeric 必须声明在 Alpha 之前，派生的 Ord 才是对的。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PreIdent {
    Numeric(u64),
    Alpha(String),
}

/// 同一个主版本号下预发布版与正式版的先后。Pre 声明在 Release 之前，
/// 派生的 Ord 就自动满足 “0.1.0-rc.8 < 0.1.0”。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PreRelease {
    Pre(Vec<PreIdent>),
    Release,
}

/// 版本号的比较键，按语义化版本的优先级规则排序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionKey {
    /// 主版本段（忽略 v 前缀，末尾的 0 已去掉，所以 0.6 == 0.6.0）。
    core: Vec<u64>,
    pre: PreRelease,
}

impl VersionKey {
    /// 是否为正式版（没有 `-rc.8` 之类的预发布标识）。
    pub fn is_release(&self) -> bool {
        matches!(self.pre, PreRelease::Release)
    }
}

impl Ord for VersionKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| self.pre.cmp(&other.pre))
    }
}

impl PartialOrd for VersionKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 把版本号解析成可比较的键（忽略 v 前缀与 `+build` 元数据）。
/// Launcher 自身、dsh 包与插件的版本比较共用。
///
/// 预发布标识必须单独处理：早先的实现把 `-rc.8` 也当成主版本段，
/// 于是 `0.1.0`（末尾 0 被去掉成 [0,1]）反而小于 `0.1.0-rc.8`（[0,1,0,0,8]），
/// dsh 一旦发出正式版就会被判成“比 rc 旧”。
pub fn version_key(value: &str) -> VersionKey {
    let value = value.trim().trim_start_matches(['v', 'V']);
    // `+build` 元数据不参与优先级比较。
    let value = value.split('+').next().unwrap_or_default();
    let (core, pre) = match value.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (value, None),
    };
    let mut core: Vec<u64> = core
        .split('.')
        .map(|part| {
            part.chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect();
    while core.last() == Some(&0) {
        core.pop();
    }
    let pre = match pre {
        // 空的预发布标识（末尾多一个 `-`）没有意义，按正式版处理。
        None => PreRelease::Release,
        Some(pre) if pre.trim().is_empty() => PreRelease::Release,
        Some(pre) => PreRelease::Pre(
            pre.split('.')
                .map(|ident| match ident.parse::<u64>() {
                    Ok(number) => PreIdent::Numeric(number),
                    Err(_) => PreIdent::Alpha(ident.to_string()),
                })
                .collect(),
        ),
    };
    VersionKey { core, pre }
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

    #[test]
    fn orders_dsh_prereleases_by_semver_precedence() {
        // issue #5：dsh 的 rc 版本必须能正确排序，否则挑不出真正的最新版。
        assert!(version_key("0.1.0-rc.8") > version_key("0.1.0-rc.7"));
        assert!(version_key("0.1.1-rc.1") > version_key("0.1.0-rc.8"));
        // 数字段按数值比，不按字典序（rc.10 比 rc.9 新）。
        assert!(version_key("0.1.0-rc.10") > version_key("0.1.0-rc.9"));
        // 正式版优先级高于同一个 core 的任何预发布版。
        assert!(version_key("0.1.0") > version_key("0.1.0-rc.8"));
        assert!(version_key("0.1.0") > version_key("0.1.0-rc.99"));
        // 标识段更少的排前面（语义化版本规则）。
        assert!(version_key("0.1.0-rc.1") > version_key("0.1.0-rc"));
        // 数字段优先级低于字母段。
        assert!(version_key("0.1.0-alpha") > version_key("0.1.0-1"));
        // build 元数据不参与比较。
        assert_eq!(version_key("0.1.0+abc"), version_key("0.1.0"));
    }

    #[test]
    fn tells_releases_apart_from_prereleases() {
        assert!(version_key("0.7.0").is_release());
        assert!(!version_key("0.1.0-rc.8").is_release());
        // 末尾多余的 `-` 不算预发布。
        assert!(version_key("0.1.0-").is_release());
    }
}
