use serde::{Deserialize, Serialize};
use std::{io::Read, time::Duration};
use tauri::State;

use crate::{state::AppState, util::epoch_secs};

const MARKET_API_URL: &str = "https://dsh.aitreez.com/catalog.json";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketOwnerRaw {
    #[serde(default)]
    avatar_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketRepoRaw {
    #[serde(default)]
    name: String,
    #[serde(default)]
    full_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    url: String,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    owner: Option<MarketOwnerRaw>,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    stars: u64,
    #[serde(default)]
    pushed_at: Option<String>,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    project_type: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    validation: Option<MarketValidationRaw>,
}

#[derive(Debug, Clone, Deserialize)]
struct MarketValidationRaw {
    #[serde(default)]
    overall: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarketApiResponse {
    schema_version: u64,
    repositories: Vec<MarketRepoRaw>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketPlugin {
    pub name: String,
    pub full_name: String,
    pub spec: String,
    pub description: String,
    pub url: String,
    pub homepage: String,
    pub avatar_url: String,
    pub topics: Vec<String>,
    pub language: String,
    pub stars: u64,
    pub pushed_at: String,
    pub archived: bool,
    pub project_type: String,
    pub category: String,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketCatalog {
    pub plugins: Vec<MarketPlugin>,
    pub fetched_at: u64,
}

fn parse_market_catalog(json: &str) -> Result<MarketCatalog, String> {
    let response: MarketApiResponse = serde_json::from_str(json)
        .map_err(|error| format!("插件目录 API 响应解析失败：{error}"))?;
    if response.schema_version != 1 {
        return Err(format!(
            "插件目录 API 版本不兼容：schemaVersion={}",
            response.schema_version
        ));
    }
    let plugins = response
        .repositories
        .into_iter()
        .map(|repo| MarketPlugin {
            spec: format!("github:{}", repo.full_name),
            name: repo.name,
            full_name: repo.full_name,
            description: repo.description.unwrap_or_default(),
            url: repo.url,
            homepage: repo.homepage.unwrap_or_default(),
            avatar_url: repo.owner.map(|owner| owner.avatar_url).unwrap_or_default(),
            topics: repo.topics,
            language: repo.language.unwrap_or_default(),
            stars: repo.stars,
            pushed_at: repo.pushed_at.unwrap_or_default(),
            archived: repo.archived,
            project_type: repo.project_type.unwrap_or_else(|| "unknown".into()),
            category: repo
                .category
                .or_else(|| repo.categories.first().cloned())
                .unwrap_or_else(|| "other".into()),
            verified: repo
                .validation
                .as_ref()
                .is_some_and(|validation| validation.overall == "verified"),
        })
        .collect();
    Ok(MarketCatalog {
        plugins,
        fetched_at: epoch_secs(),
    })
}

fn fetch_market_catalog() -> Result<MarketCatalog, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(25))
        .user_agent(concat!("dsh-launcher/", env!("CARGO_PKG_VERSION")))
        .build();
    let response = agent
        .get(MARKET_API_URL)
        .set("Accept", "application/json")
        .call()
        .map_err(|error| format!("无法访问插件商店 API：{error}"))?;
    let mut json = String::new();
    response
        .into_reader()
        .take(20 * 1024 * 1024)
        .read_to_string(&mut json)
        .map_err(|error| format!("插件商店 API 响应读取失败：{error}"))?;
    parse_market_catalog(&json)
}

#[tauri::command]
pub async fn fetch_market(
    state: State<'_, AppState>,
    force: bool,
) -> Result<MarketCatalog, String> {
    const MARKET_TTL_SECS: u64 = 600;
    if !force {
        if let Ok(cache) = state.market.lock() {
            if let Some(catalog) = cache.as_ref() {
                if epoch_secs().saturating_sub(catalog.fetched_at) < MARKET_TTL_SECS {
                    return Ok(catalog.clone());
                }
            }
        }
    }
    match fetch_market_catalog() {
        Ok(catalog) => {
            if let Ok(mut cache) = state.market.lock() {
                *cache = Some(catalog.clone());
            }
            Ok(catalog)
        }
        Err(error) => {
            // 后台自动刷新失败时退回旧数据；用户手动刷新（force）则要看到错误。
            if !force {
                if let Ok(cache) = state.market.lock() {
                    if let Some(catalog) = cache.as_ref() {
                        return Ok(catalog.clone());
                    }
                }
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_market_catalog_api() {
        let json = r##"{"schemaVersion":1,"repositories":[{"repositoryId":1,"name":"dsh-web-ui","fullName":"o/dsh-web-ui","description":"d","url":"https://github.com/o/dsh-web-ui","owner":{"login":"o","avatarUrl":"https://avatars.githubusercontent.com/u/1"},"topics":["dsh-plugin"],"language":"TypeScript","stars":12,"pushedAt":"2026-08-14T00:00:00Z","archived":false,"projectType":"plugin","category":"ui","categories":["ui"],"validation":{"overall":"verified"}}]}"##;
        let catalog = parse_market_catalog(json).expect("catalog should parse");
        assert_eq!(catalog.plugins.len(), 1);
        let plugin = &catalog.plugins[0];
        assert_eq!(plugin.spec, "github:o/dsh-web-ui");
        assert_eq!(plugin.category, "ui");
        assert!(plugin.verified);
    }

    #[test]
    fn rejects_unknown_market_catalog_schema() {
        let error = parse_market_catalog(r##"{"schemaVersion":2,"repositories":[]}"##)
            .expect_err("unknown schema should be rejected");
        assert!(error.contains("schemaVersion=2"));
    }

    #[test]
    #[ignore = "network: fetches the live plugin market"]
    fn fetches_live_market_catalog() {
        let catalog = fetch_market_catalog().expect("live market fetch should succeed");
        assert!(!catalog.plugins.is_empty());
        assert!(catalog
            .plugins
            .iter()
            .all(|plugin| plugin.spec.starts_with("github:")));
    }
}
