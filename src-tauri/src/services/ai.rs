//! Backend-only DeepSeek configuration. Secrets never enter AppConfig or IPC.
use std::{collections::HashMap, path::PathBuf, time::Duration};

use crate::models::config::{AiStatus, AppConfig};
use reqwest::Client;

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_MODEL: &str = "deepseek-flash";
pub(crate) const GENERATION_TIMEOUT: Duration = Duration::from_secs(60 * 60);

// Intentionally no Debug/Serialize: the key must not enter logs, IPC or task records.
#[derive(Clone)]
pub struct AiOptions {
    pub base_url: String,
    pub model: String,
    pub style: String,
    pub duration: f64,
    pub(crate) api_key: String,
}

fn env_file() -> Result<Option<PathBuf>, String> {
    if let Some(path) = std::env::var_os("DEEPSEEK_ENV_FILE") {
        let path = PathBuf::from(path);
        if !path.is_absolute() || !path.is_file() {
            return Err("DEEPSEEK_ENV_FILE 必须指向已有 .env 文件的绝对路径".into());
        }
        return Ok(Some(path));
    }
    // Packaged builds never search the launch cwd or the developer's source tree.
    #[cfg(debug_assertions)]
    {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.env");
        if path.is_file() {
            return Ok(Some(path));
        }
    }
    Ok(dirs::data_dir()
        .map(|path| path.join("com.esun.video-commentary/.env"))
        .filter(|path| path.is_file()))
}

fn load_file() -> Result<HashMap<String, String>, String> {
    let Some(path) = env_file()? else {
        return Ok(HashMap::new());
    };
    let items = dotenvy::from_path_iter(path)
        .map_err(|_| "无法读取 AI .env 文件，请检查权限和格式".to_string())?;
    items
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|_| "AI .env 格式错误，请检查引号与变量定义（错误内容已隐藏）".into())
}

/// Shared backend-only env lookup for optional services. Never expose secrets via IPC.
pub(crate) fn env_value(key: &str) -> Result<Option<String>, String> {
    match std::env::var(key) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(load_file()?.get(key).cloned()),
        Err(_) => Err("后端环境变量不是有效文本".into()),
    }
}

pub(crate) fn validate_base_url(value: &str) -> Result<String, String> {
    let parsed =
        url::Url::parse(value.trim()).map_err(|_| "DEEPSEEK_BASE_URL 不是有效 URL".to_string())?;
    let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if parsed.host_str().is_none()
        || !(parsed.scheme() == "https" || parsed.scheme() == "http" && local)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed
            .path()
            .trim_end_matches('/')
            .ends_with("/chat/completions")
    {
        return Err("DEEPSEEK_BASE_URL 应为 HTTPS API 基础地址（可含 /v1），不能含账号、查询参数或 /chat/completions；仅本机测试允许 HTTP".into());
    }
    Ok(parsed.as_str().trim_end_matches('/').to_string())
}

impl AiOptions {
    fn from_values(
        mut get: impl FnMut(&str) -> Option<String>,
        style: &str,
        duration: f64,
    ) -> Result<Self, String> {
        let api_key = get("DEEPSEEK_API_KEY")
            .unwrap_or_default()
            .trim()
            .to_string();
        if api_key.is_empty() {
            return Err(
                "未配置 DEEPSEEK_API_KEY，请在后端 .env 中填写后重试；不会回退 Ollama".into(),
            );
        }
        if api_key.chars().any(char::is_whitespace) {
            return Err("DEEPSEEK_API_KEY 不能包含空白字符".into());
        }
        let base_url = validate_base_url(
            &get("DEEPSEEK_BASE_URL").unwrap_or_else(|| DEFAULT_BASE_URL.into()),
        )?;
        let model = get("DEEPSEEK_MODEL")
            .unwrap_or_else(|| DEFAULT_MODEL.into())
            .trim()
            .to_string();
        if model.is_empty() {
            return Err("DEEPSEEK_MODEL 不能为空".into());
        }
        validate_model_endpoint(&base_url, &model)?;
        Ok(Self {
            base_url,
            model,
            style: style.into(),
            duration,
            api_key,
        })
    }

    fn configured(style: &str, duration: f64) -> Result<Self, String> {
        let file = load_file()?;
        // Process environment wins, including an explicit empty value. No env mutation.
        Self::from_values(
            |key| std::env::var(key).ok().or_else(|| file.get(key).cloned()),
            style,
            duration,
        )
    }

    pub async fn for_task(_config: &AppConfig, style: &str, duration: f64) -> Result<Self, String> {
        Self::configured(style, duration)
    }
}

pub(crate) fn client(timeout: Duration) -> Result<Client, reqwest::Error> {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        // Never forward a bearer key through an unexpected redirect.
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
}

pub(crate) fn validate_model_endpoint(base: &str, model: &str) -> Result<(), String> {
    let official = url::Url::parse(base)
        .ok()
        .is_some_and(|u| u.host_str() == Some("api.deepseek.com"));
    if official && model == "DeepSeek-V4.1-Flash" {
        return Err("官方接口不接受模型ID DeepSeek-V4.1-Flash。已验证的 Flash ID 是 deepseek-flash；请修改 DEEPSEEK_MODEL，进程环境变量优先于 .env。不会自动切换模型。".into());
    }
    Ok(())
}

/// Return only fixed diagnostic labels, never echo provider messages or keys.
pub(crate) fn response_error(status: reqwest::StatusCode, body: Option<&str>) -> String {
    if status != reqwest::StatusCode::BAD_REQUEST {
        return status_error(status);
    }
    let body = body.unwrap_or_default().to_ascii_lowercase();
    let hint = if body.contains("model")
        && [
            "not found",
            "not exist",
            "invalid",
            "unsupported",
            "does not",
        ]
        .iter()
        .any(|s| body.contains(s))
    {
        "模型ID无效或此接口不支持；请在设置中检测连接并核对 DEEPSEEK_MODEL，官方Flash已验证ID为 deepseek-flash"
    } else if body.contains("max_tokens") || body.contains("max_completion_tokens") {
        "接口拒绝输出token预算，请核对该模型的输出上限"
    } else if body.contains("thinking") {
        "接口不接受 thinking 参数，请确认配置的是兼容DeepSeek的接口"
    } else if body.contains("response_format") {
        "接口不接受JSON输出模式，请确认该模型支持 response_format=json_object"
    } else if body.contains("context")
        && ["length", "limit", "maximum"]
            .iter()
            .any(|s| body.contains(s))
    {
        "请求超过模型上下文预算，未截断材料"
    } else {
        "请求被拒绝；请先在设置中检测模型ID，再核对基础地址、JSON模式及token上限；服务原文已隐藏"
    };
    format!("DeepSeek 返回 HTTP {status}：{hint}")
}

pub(crate) fn request_error(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "DeepSeek 请求超时，请稍后重试".into()
    } else if error.is_connect() {
        "无法连接 DeepSeek API，请检查地址和网络".into()
    } else {
        "DeepSeek 网络请求失败（已隐藏可能包含敏感信息的响应）".into()
    }
}

pub(crate) fn status_error(status: reqwest::StatusCode) -> String {
    let hint = match status.as_u16() {
        401 | 403 => "请检查 API Key 和访问权限",
        402 => "请检查账户余额",
        404 => "请检查 API 基础地址和模型名",
        429 => "请求限流，请稍后重试",
        500..=599 => "服务暂时不可用，请稍后重试",
        _ => "请检查接口配置；响应正文已隐藏",
    };
    format!("DeepSeek 返回 HTTP {status}：{hint}")
}

pub async fn check_ai() -> AiStatus {
    let options = match AiOptions::configured("", 0.0) {
        Ok(options) => options,
        Err(error) => {
            return AiStatus {
                available: false,
                model: String::new(),
                error: Some(error),
            }
        }
    };
    // Explicit user-triggered check sends no subtitles or prose, and incurs no generation.
    let result = async {
        let response = client(Duration::from_secs(15))
            .map_err(|_| "无法创建 AI HTTP 客户端".to_string())?
            .get(format!("{}/models", options.base_url))
            .bearer_auth(&options.api_key)
            .send()
            .await
            .map_err(|e| request_error(&e))?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|_| "模型列表响应不是有效 JSON".to_string())?;
        if !body
            .get("data")
            .and_then(|v| v.as_array())
            .is_some_and(|models| {
                models
                    .iter()
                    .any(|m| m.get("id").and_then(|v| v.as_str()) == Some(options.model.as_str()))
            })
        {
            return Err("配置的 DEEPSEEK_MODEL 不在接口返回的模型列表中".into());
        }
        Ok(())
    }
    .await;
    AiStatus {
        available: result.is_ok(),
        model: options.model,
        error: result.err(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn configured(values: &[(&str, &str)]) -> Result<AiOptions, String> {
        AiOptions::from_values(
            |key| {
                values
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.to_string())
            },
            "剧情解说",
            60.0,
        )
    }
    #[test]
    fn defaults_and_custom_backend_configuration() {
        let options = configured(&[("DEEPSEEK_API_KEY", "test-only")]).unwrap();
        assert_eq!(options.base_url, DEFAULT_BASE_URL);
        assert_eq!(options.model, DEFAULT_MODEL);
        let options = configured(&[
            ("DEEPSEEK_API_KEY", "test-only"),
            ("DEEPSEEK_BASE_URL", "https://example.com/v1/"),
            ("DEEPSEEK_MODEL", "custom-model"),
        ])
        .unwrap();
        assert_eq!(options.base_url, "https://example.com/v1");
        assert_eq!(options.model, "custom-model");
        assert!(configured(&[]).is_err());
        assert!(configured(&[("DEEPSEEK_API_KEY", "")]).is_err());
    }
    #[test]
    fn official_model_mismatch_fails_locally_but_custom_ids_are_not_rewritten() {
        assert!(
            validate_model_endpoint("https://api.deepseek.com/v1", "DeepSeek-V4.1-Flash").is_err()
        );
        assert!(validate_model_endpoint("https://api.deepseek.com", "deepseek-flash").is_ok());
        assert!(validate_model_endpoint("https://example.com/v1", "DeepSeek-V4.1-Flash").is_ok());
    }

    #[test]
    fn bad_request_diagnostics_never_echo_provider_text() {
        let result = response_error(
            reqwest::StatusCode::BAD_REQUEST,
            Some("invalid model sensitive-key-and-prose"),
        );
        assert!(result.contains("模型ID"));
        assert!(!result.contains("sensitive"));
        assert!(response_error(
            reqwest::StatusCode::BAD_REQUEST,
            Some("max_tokens too large")
        )
        .contains("token"));
    }

    #[test]
    fn unsafe_urls_fail_instead_of_silently_changing_destinations() {
        for base in [
            "file:///tmp/api",
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com?key=secret",
            "https://example.com/#secret",
            "https://api.deepseek.com/chat/completions",
        ] {
            assert!(validate_base_url(base).is_err());
        }
        assert_eq!(
            validate_base_url("http://127.0.0.1:1234/v1/").unwrap(),
            "http://127.0.0.1:1234/v1"
        );
    }
    #[test]
    fn dotenv_parser_and_missing_key_errors_do_not_echo_secrets() {
        let text = "DEEPSEEK_API_KEY='test-only'\nDEEPSEEK_MODEL=custom-model\n";
        let values = dotenvy::from_read_iter(text.as_bytes())
            .collect::<Result<HashMap<_, _>, _>>()
            .unwrap();
        assert_eq!(values["DEEPSEEK_API_KEY"], "test-only");
        assert!(!configured(&[("DEEPSEEK_API_KEY", "secret with space")])
            .err()
            .unwrap()
            .contains("secret"));
    }
}
