use crate::ai_context::build_system_context;
use crate::config::AiConfig;
use crate::scan::{format_size, ScanEntry};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Serialize, Clone)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Option<Value>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: ApiError,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HealthCheckReply {
    NeedPaths(Vec<String>),
    Report(String),
}

const FOLDER_ANALYSIS_PROMPT: &str = r#"你是 Windows 磁盘空间分析助手 Driver Doctor。
请用简体中文说明路径用途、主要占用项、清理风险和具体清理入口。"#;

const STRUCTURE_ANALYSIS_PROMPT: &str = r#"你是 Driver Doctor 磁盘结构分析助手。
用户会提供目录结构 RAG。请按路径和大小分析主要占用、用途、风险和清理建议。"#;

const CLEANUP_PLAN_PROMPT: &str = r#"你是 Windows 磁盘清理规划助手。
请生成清理计划，包含优先级、预计释放空间、步骤和不要删除的系统路径提醒。"#;

const HEALTH_CHECK_PROMPT: &str = r#"你是 Driver Doctor 空间医生，负责 Windows 磁盘健康检查。
用户会提供盘符、系统环境和 disk_diagnostic_tree 数据。
生成报告前必须基于路径名联网检索常见来源、生成原因、官方或社区推荐清理方式。
如果当前模型或服务商没有实际联网，请明确说明“未实际联网，仅基于通用知识判断”。

最终报告必须包含：
## 占用报告
- 按影响排序列出主要文件夹和文件。
- 说明常见来源、生成原因、是否属于系统或应用关键路径。
- 引用 disk_diagnostic_tree 中的路径、大小、上下级关系和 folded summary。

## 清理意见报告
- 每项包含目标路径、预计释放空间、风险等级、推荐处理方式、具体操作步骤。
- 优先级遵守：设置转移 > 设置自动清理 > 设置关闭生成 > 卸载占用程序 > 暴力删除。

## 膨胀规避
- 说明如何避免同类目录再次膨胀。

## 执行优先级
- 给出 1 到 N 的执行顺序。"#;

fn wrap_system(base: &str, current_path: Option<&str>) -> String {
    format!("{base}\n\n{}", build_system_context(current_path))
}

fn connection_test_success_message(reply: String, config: &AiConfig) -> String {
    if web_search_requested(config) {
        format!("{reply}；联网参数已随测试请求发送")
    } else {
        reply
    }
}

pub async fn test_connection(config: &AiConfig) -> Result<String, String> {
    let reply = chat(
        config,
        "你是连接测试助手。",
        "请只回复：连接成功",
        None,
        Arc::new(AtomicBool::new(false)),
    )
    .await?;

    Ok(connection_test_success_message(reply, config))
}

pub async fn analyze_folder(
    config: &AiConfig,
    path: &Path,
    size: u64,
    current_path: Option<&str>,
    extra_context: Option<&str>,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let user_content = format!(
        "{}\n\n请分析以下文件夹：\n路径: {}\n大小: {}\n{}",
        build_system_context(current_path),
        path.display(),
        format_size(size),
        extra_context.unwrap_or("")
    );

    chat(
        config,
        &wrap_system(FOLDER_ANALYSIS_PROMPT, current_path),
        &user_content,
        current_path,
        cancel,
    )
    .await
}

pub async fn analyze_current_structure(
    config: &AiConfig,
    root: &Path,
    rag_document: &str,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let path_str = root.display().to_string();
    let user_content = format!(
        "{}\n\n## 分析任务\n分析当前目录 `{}` 的内部结构，找出占用大户并给出清理建议。\n\n## RAG 目录结构文档\n\n{}",
        build_system_context(Some(&path_str)),
        root.display(),
        rag_document
    );

    chat(
        config,
        &wrap_system(STRUCTURE_ANALYSIS_PROMPT, Some(&path_str)),
        &user_content,
        Some(&path_str),
        cancel,
    )
    .await
}

pub async fn analyze_deep_structure(
    config: &AiConfig,
    root: &Path,
    rag_document: &str,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let path_str = root.display().to_string();
    let user_content = format!(
        "{}\n\n## 分析任务\n对以下文件夹做深度结构分析，识别各层级占用构成。\n\n## RAG 深度结构文档\n\n{}",
        build_system_context(Some(&path_str)),
        rag_document
    );

    chat(
        config,
        &wrap_system(STRUCTURE_ANALYSIS_PROMPT, Some(&path_str)),
        &user_content,
        Some(&path_str),
        cancel,
    )
    .await
}

pub async fn generate_cleanup_plan(
    config: &AiConfig,
    entries: &[ScanEntry],
    current_path: Option<&str>,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let mut list = String::from("全盘扫描结果（按大小降序）：\n\n");
    for (i, e) in entries.iter().enumerate() {
        list.push_str(&format!(
            "{}. {} - {} ({})\n",
            i + 1,
            e.name,
            format_size(e.size),
            e.path.display()
        ));
    }

    let user_content = format!("{}\n\n{list}", build_system_context(current_path));

    chat(
        config,
        &wrap_system(CLEANUP_PLAN_PROMPT, current_path),
        &user_content,
        current_path,
        cancel,
    )
    .await
}

pub async fn generate_cleanup_plan_from_health_report(
    config: &AiConfig,
    root: &Path,
    health_report: &str,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let path_str = root.display().to_string();
    let user_content = format!(
        "{}\n\n## 任务\n基于下面的健康检查报告，生成单独的清理计划。\n只选择相对安全且推荐的清理条目，优先使用设置转移、自动清理、关闭生成、卸载程序，谨慎给出删除建议。\n每个条目必须尽量包含 Windows 绝对路径，方便 Driver Doctor 生成临时 BAT 入口。\n\n## 输出格式\n请使用 Markdown，必须包含：\n## 清理计划\n| 优先级 | 路径 | 风险 | 推荐方式 | BAT入口说明 |\n| --- | --- | --- | --- | --- |\n\n## 执行说明\n- 说明 BAT 只作为用户确认后的入口，不应默认静默删除。\n\n## 健康检查报告\n\n{}",
        build_system_context(Some(&path_str)),
        health_report
    );

    chat(
        config,
        &wrap_system(CLEANUP_PLAN_PROMPT, Some(&path_str)),
        &user_content,
        Some(&path_str),
        cancel,
    )
    .await
}

pub async fn generate_health_check_step(
    config: &AiConfig,
    root: &Path,
    rag_document: &str,
    round: u32,
    force_report: bool,
    cancel: Arc<AtomicBool>,
) -> Result<HealthCheckReply, String> {
    let path_str = root.display().to_string();
    let mode = if force_report {
        "必须返回 REPORT，不要再请求路径。"
    } else {
        "如果当前数据足够生成可靠报告，返回 REPORT；如果还需要深挖，只返回 NEED_PATHS。"
    };
    let user_content = format!(
        "{}\n\n## 健康检查交互协议\n当前轮次: {round}\n{mode}\n\n返回格式只能二选一：\n\nNEED_PATHS\n[\"C:\\\\需要深挖的路径\", \"C:\\\\另一个路径\"]\n\n或：\n\nREPORT\n<完整 Markdown 诊断报告>\n\n请求深化时最多列出 6 个路径，必须来自已提供的 major_items 路径，优先选高占用且用途不明确的目录。\n\n## 当前累计 disk_diagnostic_tree\n\n{}",
        build_system_context(Some(&path_str)),
        rag_document
    );

    let raw = chat(
        config,
        &wrap_system(HEALTH_CHECK_PROMPT, Some(&path_str)),
        &user_content,
        Some(&path_str),
        cancel,
    )
    .await?;

    parse_health_check_reply(&raw)
}

pub fn parse_health_check_reply(text: &str) -> Result<HealthCheckReply, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("健康检查 AI 返回空内容".into());
    }
    if let Some(rest) = trimmed.strip_prefix("REPORT") {
        let report = rest.trim();
        if report.is_empty() {
            return Err("健康检查 REPORT 内容为空".into());
        }
        return Ok(HealthCheckReply::Report(report.to_string()));
    }

    if trimmed.starts_with("NEED_PATHS") {
        let start = trimmed
            .find('[')
            .ok_or_else(|| "NEED_PATHS 缺少 JSON 数组".to_string())?;
        let end = trimmed
            .rfind(']')
            .ok_or_else(|| "NEED_PATHS 缺少 JSON 数组结束".to_string())?;
        let paths: Vec<String> = serde_json::from_str(&trimmed[start..=end])
            .map_err(|e| format!("解析 NEED_PATHS 失败: {e}"))?;
        let paths = paths
            .into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .take(6)
            .collect();
        return Ok(HealthCheckReply::NeedPaths(paths));
    }

    Ok(HealthCheckReply::Report(trimmed.to_string()))
}

async fn wait_cancelled(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn build_chat_body(config: &AiConfig, mut messages: Vec<Message>) -> Result<Value, String> {
    let search_enabled = web_search_requested(config);
    if search_enabled {
        messages.insert(
            0,
            Message {
                role: "system".into(),
                content: "本次请求已启用 web_search 联网搜索能力。生成磁盘诊断或清理报告前，必须检索关键路径来源、生成原因和清理方案；不要声称未实际联网。".into(),
            },
        );
    }
    let mut body = json!({
        "model": config.model,
        "messages": messages,
        "temperature": 0.3
    });

    if search_enabled {
        apply_native_search_template(&mut body, &config.base_url)?;
    }
    if config.web_search_enabled {
        apply_custom_request_json(&mut body, &config.custom_request_json)?;
    }

    Ok(body)
}

fn choice_text(choice: &Choice) -> Result<String, String> {
    let content = choice.message.content.trim().to_string();
    if !content.is_empty() {
        return Ok(choice.message.content.clone());
    }

    let finish_reason = choice.finish_reason.as_deref().unwrap_or("unknown");
    let has_tool_calls = choice
        .message
        .tool_calls
        .as_ref()
        .is_some_and(|value| !value.is_null());
    Err(format!(
        "API 未返回文本内容（finish_reason: {finish_reason}, tool_calls: {has_tool_calls}）"
    ))
}

fn web_search_requested(config: &AiConfig) -> bool {
    config.web_search_enabled || provider_has_native_search(&config.base_url)
}

fn apply_native_search_template(body: &mut Value, base_url: &str) -> Result<(), String> {
    let host = provider_host(base_url);
    if is_mimo_host(&host) {
        append_array_field(
            body,
            "tools",
            vec![json!({
                "type": "web_search",
                "force_search": true
            })],
        )?;
        merge_object(body, &json!({ "tool_choice": "auto" }))?;
    } else if host.contains("openrouter.ai") {
        append_array_field(body, "plugins", vec![json!({ "id": "web" })])?;
    } else if host.contains("api.x.ai") || host.contains("x.ai") {
        append_array_field(body, "tools", vec![json!({ "type": "web_search" })])?;
    }
    Ok(())
}

fn provider_has_native_search(base_url: &str) -> bool {
    let host = provider_host(base_url);
    is_mimo_host(&host)
        || host.contains("openrouter.ai")
        || host.contains("api.x.ai")
        || host.contains("x.ai")
}

fn is_mimo_host(host: &str) -> bool {
    host.contains("mimo.mi.com") || host.contains("xiaomimimo.com") || host.contains("mimo-v2.com")
}

fn provider_host(base_url: &str) -> String {
    base_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn native_search_profile_label(base_url: &str) -> &'static str {
    let host = provider_host(base_url);
    if is_mimo_host(&host) {
        "MiMo 原生 web_search"
    } else if host.contains("openrouter.ai") {
        "OpenRouter web 插件"
    } else if host.contains("api.x.ai") || host.contains("x.ai") {
        "xAI web_search"
    } else if host.contains("perplexity.ai") {
        "Perplexity Sonar 通常模型自带联网"
    } else if host.contains("deepseek.com") {
        "DeepSeek API 未检测到原生联网模板"
    } else {
        "未知服务商，仅应用自定义 JSON"
    }
}

fn apply_custom_request_json(body: &mut Value, json_text: &str) -> Result<(), String> {
    if json_text.trim().is_empty() {
        return Ok(());
    }
    let custom: Value =
        serde_json::from_str(json_text).map_err(|e| format!("自定义请求 JSON 无效: {e}"))?;

    if custom.get("$set").is_none() && custom.get("$append").is_none() {
        merge_object(body, &custom)?;
        return Ok(());
    }

    if let Some(set) = custom.get("$set") {
        reject_protected_fields(set)?;
        merge_object(body, set)?;
    }
    if let Some(append) = custom.get("$append") {
        let Some(map) = append.as_object() else {
            return Err("$append 必须是对象".into());
        };
        for (key, value) in map {
            let Some(items) = value.as_array() else {
                return Err(format!("$append.{key} 必须是数组"));
            };
            append_array_field(body, key, items.clone())?;
        }
    }
    Ok(())
}

fn reject_protected_fields(value: &Value) -> Result<(), String> {
    if let Some(map) = value.as_object() {
        for key in map.keys() {
            if matches!(key.as_str(), "model" | "messages") {
                return Err(format!("自定义请求 JSON 不允许覆盖 {key}"));
            }
        }
    }
    Ok(())
}

fn merge_object(target: &mut Value, source: &Value) -> Result<(), String> {
    reject_protected_fields(source)?;
    let Some(target_map) = target.as_object_mut() else {
        return Err("请求体不是对象".into());
    };
    let Some(source_map) = source.as_object() else {
        return Err("自定义请求 JSON 顶层必须是对象".into());
    };
    deep_merge_map(target_map, source_map);
    Ok(())
}

fn deep_merge_map(target: &mut Map<String, Value>, source: &Map<String, Value>) {
    for (key, value) in source {
        match (target.get_mut(key), value) {
            (Some(Value::Object(target_obj)), Value::Object(source_obj)) => {
                deep_merge_map(target_obj, source_obj);
            }
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

fn append_array_field(body: &mut Value, key: &str, items: Vec<Value>) -> Result<(), String> {
    let Some(map) = body.as_object_mut() else {
        return Err("请求体不是对象".into());
    };
    match map.get_mut(key) {
        Some(Value::Array(existing)) => {
            for item in items {
                if !existing.iter().any(|value| value == &item) {
                    existing.push(item);
                }
            }
        }
        Some(_) => return Err(format!("{key} 已存在但不是数组")),
        None => {
            map.insert(key.to_string(), Value::Array(items));
        }
    }
    Ok(())
}

async fn chat(
    config: &AiConfig,
    system: &str,
    user: &str,
    _current_path: Option<&str>,
    cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    if config.api_key.trim().is_empty() {
        return Err("请先在设置中配置 API Key".into());
    }

    let base = config.base_url.trim_end_matches('/');
    let url = format!("{base}/chat/completions");

    let client = Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())?;

    let body = build_chat_body(
        config,
        vec![
            Message {
                role: "system".into(),
                content: system.into(),
            },
            Message {
                role: "user".into(),
                content: user.into(),
            },
        ],
    )?;

    tokio::select! {
        res = async {
            let resp = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", config.api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("网络请求失败: {e}"))?;

            let status = resp.status();
            let text = resp.text().await.map_err(|e| e.to_string())?;

            if !status.is_success() {
                if let Ok(err) = serde_json::from_str::<ErrorResponse>(&text) {
                    return Err(format!("API 错误: {}", err.error.message));
                }
                return Err(format!("API 返回 {status}: {text}"));
            }

            let parsed: ChatResponse =
                serde_json::from_str(&text).map_err(|e| format!("解析响应失败: {e}"))?;
            parsed
                .choices
                .first()
                .ok_or_else(|| "API 未返回内容".to_string())
                .and_then(choice_text)
        } => res,
        _ = wait_cancelled(cancel) => Err("分析已打断。".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_message() -> Vec<Message> {
        vec![Message {
            role: "user".into(),
            content: "search".into(),
        }]
    }

    #[test]
    fn mimo_profile_adds_web_search_tool_when_enabled() {
        let config = AiConfig {
            base_url: "https://mimo.mi.com/v1".into(),
            api_key: "key".into(),
            model: "mimo-v2.5-pro".into(),
            web_search_enabled: true,
            custom_request_json: String::new(),
        };

        let body = build_chat_body(&config, test_message()).unwrap();

        assert_eq!(body["tools"][0]["type"], "web_search");
        assert_eq!(body["tools"][0]["force_search"], true);
        assert_eq!(body["tool_choice"], "auto");
    }

    #[test]
    fn mimo_profile_adds_web_search_tool_even_without_checkbox() {
        let config = AiConfig {
            base_url: "https://api.xiaomimimo.com/v1".into(),
            api_key: "key".into(),
            model: "mimo-v2.5-pro".into(),
            web_search_enabled: false,
            custom_request_json: String::new(),
        };

        let body = build_chat_body(&config, test_message()).unwrap();

        assert_eq!(body["tools"][0]["type"], "web_search");
        assert_eq!(body["tools"][0]["force_search"], true);
    }

    #[test]
    fn custom_request_json_sets_and_appends_without_replacing_messages() {
        let config = AiConfig {
            base_url: "https://relay.example.com/v1".into(),
            api_key: "key".into(),
            model: "relay-model".into(),
            web_search_enabled: true,
            custom_request_json: r#"{
              "$set": { "reasoning_effort": "low" },
              "$append": { "tools": [{ "type": "web_search" }] }
            }"#
            .into(),
        };

        let body = build_chat_body(&config, test_message()).unwrap();

        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(body["tools"][0]["type"], "web_search");
        assert_eq!(body["messages"][1]["content"], "search");
    }

    #[test]
    fn custom_request_json_cannot_override_messages() {
        let config = AiConfig {
            base_url: "https://relay.example.com/v1".into(),
            api_key: "key".into(),
            model: "relay-model".into(),
            web_search_enabled: true,
            custom_request_json: r#"{ "$set": { "messages": [] } }"#.into(),
        };

        let err = build_chat_body(&config, Vec::new()).unwrap_err();

        assert!(err.contains("messages"));
    }

    #[test]
    fn connection_test_mentions_web_params_for_native_search_provider() {
        let config = AiConfig {
            base_url: "https://api.xiaomimimo.com/v1".into(),
            api_key: "key".into(),
            model: "mimo-v2.5-pro".into(),
            web_search_enabled: false,
            custom_request_json: String::new(),
        };

        let message = connection_test_success_message("连接成功".into(), &config);

        assert!(message.contains("联网参数"));
    }

    #[test]
    fn connection_test_stays_plain_without_search_params() {
        let config = AiConfig {
            base_url: "https://api.deepseek.com/v1".into(),
            api_key: "key".into(),
            model: "deepseek-chat".into(),
            web_search_enabled: false,
            custom_request_json: String::new(),
        };

        let message = connection_test_success_message("连接成功".into(), &config);

        assert_eq!(message, "连接成功");
    }

    #[test]
    fn empty_choice_content_reports_finish_reason_and_tool_calls() {
        let choice = Choice {
            message: ResponseMessage {
                content: String::new(),
                tool_calls: Some(json!([{"type": "web_search"}])),
            },
            finish_reason: Some("tool_calls".into()),
        };

        let err = choice_text(&choice).unwrap_err();

        assert!(err.contains("finish_reason: tool_calls"));
        assert!(err.contains("tool_calls: true"));
    }

    #[test]
    fn parses_need_paths_reply() {
        let reply =
            parse_health_check_reply("NEED_PATHS\n[\"C:\\\\Users\", \"C:\\\\ProgramData\"]")
                .unwrap();

        assert_eq!(
            reply,
            HealthCheckReply::NeedPaths(vec![
                "C:\\Users".to_string(),
                "C:\\ProgramData".to_string()
            ])
        );
    }

    #[test]
    fn parses_report_reply() {
        let reply = parse_health_check_reply("REPORT\n## 占用报告\n内容").unwrap();

        assert_eq!(
            reply,
            HealthCheckReply::Report("## 占用报告\n内容".to_string())
        );
    }

    #[test]
    fn rejects_empty_health_check_reply() {
        let err = parse_health_check_reply("  \n ").unwrap_err();

        assert!(err.contains("空"));
    }

    #[test]
    fn rejects_empty_report_reply() {
        let err = parse_health_check_reply("REPORT\n\n").unwrap_err();

        assert!(err.contains("空"));
    }
}
