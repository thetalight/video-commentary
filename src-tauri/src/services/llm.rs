//! Thin Rig adapter. No tool loop or nested retries: the business controller owns them.
use super::director::{self, DirectorOptions};
use rig_core::{
    client::CompletionClient,
    completion::{AssistantContent, CompletionModel, CompletionResponse, FinishReason},
    message::{ImageDetail, ImageMediaType, UserContent},
    providers::ollama,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

fn visible_json<T: DeserializeOwned>(response: CompletionResponse) -> Result<T, String> {
    let finish = response.finish_reason();
    if matches!(finish, Some(FinishReason::Length)) {
        return Err("模型输出被长度上限截断；未发布残缺稿，请缩小本次工作范围后重试".into());
    }
    if matches!(
        finish,
        Some(FinishReason::ContentFilter | FinishReason::ToolCalls)
    ) {
        return Err("模型未返回可用导演稿（内容过滤或意外工具调用）".into());
    }
    let text: String = response
        .choice
        .into_iter()
        .filter_map(|part| match part {
            AssistantContent::Text(text) => Some(text.text),
            _ => None, // Never treat reasoning or tool arguments as a script.
        })
        .collect();
    if text.trim().is_empty() {
        return Err("模型响应正文为空（思考内容不作为导演稿）；请检查模型配置".into());
    }
    serde_json::from_str(&text).map_err(|e| format!("Rig 结构化输出校验失败：{e}；未发布此版本"))
}

pub async fn structured_with_local_images<T: DeserializeOwned>(
    options: &DirectorOptions,
    system: &str,
    prompt: &str,
    images: Vec<Vec<u8>>,
    schema: Value,
) -> Result<T, String> {
    use base64::Engine;
    if options.provider != "ollama" {
        return Err("视觉评审只允许本机 Ollama".into());
    }
    if images.is_empty() || images.len() > 12 {
        return Err("本地视觉评审每批需要1到12张图片".into());
    }
    let base = options.base_url.trim().trim_end_matches('/');
    if !matches!(base, "http://127.0.0.1:11434" | "http://localhost:11434") {
        return Err("本地视觉模型地址必须是本机 Ollama".into());
    }
    let http = director::director_client("ollama", director::director_request_timeout("ollama"))
        .map_err(|e| e.to_string())?;
    let client = ollama::Client::builder()
        .api_key(rig_core::client::Nothing)
        .base_url(base)
        .http_client(http)
        .build()
        .map_err(|e| e.to_string())?;
    let mut content = vec![UserContent::text(prompt)];
    for bytes in images {
        content.push(UserContent::image_base64(
            base64::engine::general_purpose::STANDARD.encode(bytes),
            Some(ImageMediaType::JPEG),
            Some(ImageDetail::Low),
        ));
    }
    let schema = serde_json::from_value(schema).map_err(|e| e.to_string())?;
    let response = client
        .completion_model(options.model.trim())
        .completion_request(content)
        .preamble(system.to_string())
        .temperature(0.0)
        .max_tokens(4096)
        .output_schema(schema)
        .additional_params(json!({"think":false,"keep_alive":"10m","num_ctx":32768}))
        .send()
        .await
        .map_err(|e| {
            format!("本地视觉模型请求失败：{e}。请确认所选 Ollama 模型支持图片；不会回退云端")
        })?;
    visible_json(response)
}

pub async fn structured<T: DeserializeOwned>(
    options: &DirectorOptions,
    system: &str,
    user: &str,
    schema: Value,
) -> Result<T, String> {
    if options.provider != "ollama" {
        return Err("当前版本只允许本机 Ollama，已禁止把字幕、导演稿或画面发送到云模型".into());
    }
    if options.model.trim().is_empty() {
        return Err("请先设置导演模型".into());
    }
    let http = director::director_client(
        &options.provider,
        director::director_request_timeout(&options.provider),
    )
    .map_err(|e| e.to_string())?;
    let schema = serde_json::from_value(schema).map_err(|e| format!("无效的输出 schema：{e}"))?;
    let response = if options.provider == "ollama" {
        let base = options.base_url.trim().trim_end_matches('/');
        if !matches!(base, "http://127.0.0.1:11434" | "http://localhost:11434") {
            return Err("本地模型地址必须为本机 Ollama 地址".into());
        }
        let client = ollama::Client::builder()
            .api_key(rig_core::client::Nothing)
            .base_url(base)
            .http_client(http)
            .build()
            .map_err(|e| e.to_string())?;
        client
            .completion_model(options.model.trim())
            .completion_request(user)
            .preamble(system.to_string())
            .temperature(0.2)
            .max_tokens(8192)
            .output_schema(schema)
            .additional_params(json!({"think":false,"keep_alive":"10m","num_ctx":65536}))
            .send()
            .await
    } else {
        unreachable!("provider checked above")
    };
    let response = response.map_err(|error| {
        // Provider errors may contain request diagnostics; redact configured secrets.
        let mut detail = error.to_string();
        if !options.api_key.trim().is_empty() {
            detail = detail.replace(options.api_key.trim(), "[已隐藏]");
        }
        format!(
            "本机 Ollama 模型请求失败：{}",
            detail.chars().take(1000).collect::<String>()
        )
    })?;
    visible_json(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rig_provider_wire_contracts() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for local in [true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0u8; 4096];
                    let n = socket.read(&mut buffer).await.unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(boundary) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..boundary]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap();
                        if bytes.len() >= boundary + 4 + length {
                            let first = headers.lines().next().unwrap().to_string();
                            let body: Value =
                                serde_json::from_slice(&bytes[boundary + 4..boundary + 4 + length])
                                    .unwrap();
                            let response=json!({"model":"test","created_at":"2026-09-10T00:00:00Z","message":{"role":"assistant","content":"{\"ok\":true}"},"done":true,"done_reason":"stop"}).to_string();
                            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).as_bytes()).await.unwrap();
                            return (first, body);
                        }
                    }
                }
            });
            let http = reqwest::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap();
            let schema=serde_json::from_value(json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false})).unwrap();
            let response = {
                ollama::Client::builder()
                    .api_key(rig_core::client::Nothing)
                    .base_url(&base)
                    .http_client(http)
                    .build()
                    .unwrap()
                    .completion_model("test")
                    .completion_request("test")
                    .max_tokens(8192)
                    .output_schema(schema)
                    .additional_params(json!({"think":false,"keep_alive":"10m","num_ctx":65536}))
                    .send()
                    .await
                    .unwrap()
            };
            assert_eq!(visible_json::<Value>(response).unwrap()["ok"], true);
            let (path, body) = server.await.unwrap();
            assert!(local);
            assert!(path.contains("/api/chat "));
            assert_eq!(body["think"], false);
            assert_eq!(body["format"]["type"], "object");
            assert_eq!(body["options"]["num_predict"], 8192);
            assert_eq!(body["options"]["num_ctx"], 65536);
        }
    }
    #[test]
    fn rejects_truncated_even_if_json_is_parseable() {
        let response = CompletionResponse::new(
            vec![AssistantContent::text("{}")],
            Default::default(),
            "test",
        )
        .with_finish_reason(FinishReason::Length);
        assert!(visible_json::<Value>(response)
            .unwrap_err()
            .contains("截断"));
    }
    #[test]
    fn parses_only_visible_text() {
        let response = CompletionResponse::new(
            vec![AssistantContent::text("{\"passed\":true}")],
            Default::default(),
            "test",
        );
        assert_eq!(visible_json::<Value>(response).unwrap()["passed"], true);
        let response = CompletionResponse::new(
            vec![AssistantContent::reasoning("{\"passed\":true}")],
            Default::default(),
            "test",
        );
        assert!(visible_json::<Value>(response).is_err());
    }
}
