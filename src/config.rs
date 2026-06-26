use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    #[serde(default)]
    pub web_search_enabled: bool,
    #[serde(default = "default_custom_request_json")]
    pub custom_request_json: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            web_search_enabled: false,
            custom_request_json: default_custom_request_json(),
        }
    }
}

fn default_custom_request_json() -> String {
    "{\n  \"$append\": {\n    \"tools\": [\n      {\n        \"type\": \"web_search\",\n        \"force_search\": true,\n        \"limit\": 3\n      }\n    ]\n  }\n}"
        .replace(
            "{\n        \"type\": \"web_search\",\n        \"force_search\": true,\n        \"limit\": 3\n      }",
            "{\n        \"type\": \"function\",\n        \"function\": {\n          \"name\": \"web_search\",\n          \"description\": \"Search the web for current information\",\n          \"parameters\": {\n            \"type\": \"object\",\n            \"properties\": {\n              \"query\": {\n                \"type\": \"string\",\n                \"description\": \"The search query\"\n              }\n            },\n            \"required\": [\"query\"]\n          }\n        }\n      }",
        )
        .replace(
            "  \"$append\": {",
            "  \"$set\": {\n    \"tool_choice\": \"auto\"\n  },\n  \"$append\": {",
        )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutConfig {
    #[serde(default)]
    pub sidebar_width: Option<f32>,
    #[serde(default)]
    pub ai_panel_width: Option<f32>,
    #[serde(default)]
    pub directory_col_widths: Option<[f32; 6]>,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            sidebar_width: None,
            ai_panel_width: None,
            directory_col_widths: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub ai: AiConfig,
    #[serde(default)]
    pub last_scan_path: String,
    #[serde(default)]
    pub layout: LayoutConfig,
}

impl AppConfig {
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("driver-doctor")
            .join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            fs::read_to_string(&path)
                .ok()
                .and_then(|s| toml::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(content) = toml::to_string_pretty(self) {
            let _ = fs::write(path, content);
        }
    }
}
