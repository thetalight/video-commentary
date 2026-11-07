//! DeepSeek JSON adapter. One bounded schema-format retry; business repairs stay separate.
use super::ai::{self, AiOptions};
use rig_core::{
    client::CompletionClient,
    completion::{AssistantContent, CompletionModel, CompletionResponse, FinishReason},
    providers::deepseek,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

const SPEAKER_RULE: &str = "请结合字幕上下文理解人物身份与关系，自主选择自然、清楚且前后一致的人物称呼，让观众容易听懂；对无法确认的身份保持审慎。";

fn visible_json<T: DeserializeOwned>(response: CompletionResponse) -> Result<T, String> {
    match response.finish_reason() {
        Some(FinishReason::Length) => {
            return Err("模型输出被长度上限截断；未发布残缺稿，请缩小本次工作范围后重试".into())
        }
        Some(FinishReason::Stop) => {}
        _ => return Err("模型未正常完成（内容过滤、工具调用或未知停止原因）；未发布此版本".into()),
    }
    let mut text = String::new();
    for part in response.choice {
        match part {
            AssistantContent::Text(part) => text.push_str(&part.text),
            AssistantContent::ToolCall(_) => {
                return Err("模型意外返回工具调用；未发布此版本".into())
            }
            _ => {} // Reasoning is never treated as returned prose or JSON.
        }
    }
    if text.trim().is_empty() {
        return Err("模型响应正文为空（思考内容不作为结果）；请检查模型配置".into());
    }
    serde_json::from_str(&text)
        .map_err(|_| "结构化输出校验失败（响应正文已隐藏）；未发布此版本".into())
}

fn system_prompt(system: &str, schema: &Value) -> String {
    // DeepSeek supports json_object, not the Ollama/Rig json_schema wire format.
    format!("{system}\n{SPEAKER_RULE}\n只返回 JSON 对象，不输出 Markdown 或分析过程。输出必须严格符合此 JSON Schema：\n{schema}")
}

// Closed subset used by this application's schemas; do not pretend json_object
// enforces JSON Schema remotely. Fail closed if a future caller adds unsupported rules.
#[cfg(test)]
fn matches_schema(value: &Value, schema: &Value) -> bool {
    schema_issues(value, schema).is_empty()
}

/// Diagnostics contain schema-owned paths/types only, never model prose, unknown
/// field names, actual enum values or provider responses that could include secrets.
pub(crate) fn schema_issues(value: &Value, schema: &Value) -> Vec<String> {
    let mut issues = Vec::new();
    inspect_schema(value, schema, "$", &mut issues);
    issues.truncate(4);
    issues
}
fn inspect_schema(value: &Value, schema: &Value, path: &str, issues: &mut Vec<String>) {
    if issues.len() >= 4 {
        return;
    }
    let Some(rules) = schema.as_object() else {
        issues.push(format!("{path}: 应用schema不是对象"));
        return;
    };
    if rules.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type"
                | "properties"
                | "required"
                | "additionalProperties"
                | "items"
                | "maxItems"
                | "minimum"
                | "maximum"
                | "enum"
                | "maxLength"
                | "minLength"
        )
    }) {
        issues.push(format!("{path}: 应用schema含不支持的规则"));
        return;
    }
    if let Some(allowed) = schema.get("enum") {
        if !allowed
            .as_array()
            .is_some_and(|items| items.contains(value))
        {
            issues.push(format!("{path}: 枚举不合法，须使用schema列出的原始枚举"));
        }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let Some(object) = value.as_object() else {
                issues.push(format!("{path}: 必须为object"));
                return;
            };
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for key in required {
                    if let Some(key) = key.as_str() {
                        if !object.contains_key(key) {
                            issues.push(format!("{path}.{key}: 缺少必填字段"));
                        }
                    } else {
                        issues.push(format!("{path}: 应用required规则无效"));
                    }
                    if issues.len() >= 4 {
                        return;
                    }
                }
            }
            let properties = schema.get("properties").and_then(Value::as_object);
            if schema.get("additionalProperties") == Some(&Value::Bool(false))
                && object
                    .keys()
                    .any(|key| properties.is_none_or(|p| !p.contains_key(key)))
            {
                issues.push(format!("{path}: 包含schema之外字段（字段名已隐藏）"));
            }
            if let Some(properties) = properties {
                for (key, rule) in properties {
                    if let Some(child) = object.get(key) {
                        inspect_schema(child, rule, &format!("{path}.{key}"), issues);
                    }
                    if issues.len() >= 4 {
                        return;
                    }
                }
            }
        }
        Some("array") => {
            let Some(items) = value.as_array() else {
                issues.push(format!("{path}: 必须为array"));
                return;
            };
            if schema
                .get("maxItems")
                .and_then(Value::as_u64)
                .is_some_and(|max| items.len() as u64 > max)
            {
                issues.push(format!("{path}: 数组数量超过maxItems"));
            }
            for (i, item) in items.iter().enumerate() {
                if let Some(rule) = schema.get("items") {
                    inspect_schema(item, rule, &format!("{path}[{i}]"), issues);
                } else {
                    issues.push(format!("{path}: 缺少应用items规则"));
                }
                if issues.len() >= 4 {
                    return;
                }
            }
        }
        Some("integer") | Some("number") => {
            if schema["type"] == "integer" && !(value.is_i64() || value.is_u64()) {
                issues.push(format!("{path}: 必须为整数，不能是字符串或小数"));
                return;
            }
            let Some(n) = value.as_f64() else {
                issues.push(format!("{path}: 必须为number"));
                return;
            };
            if schema
                .get("minimum")
                .and_then(Value::as_f64)
                .is_some_and(|min| n < min)
                || schema
                    .get("maximum")
                    .and_then(Value::as_f64)
                    .is_some_and(|max| n > max)
            {
                issues.push(format!("{path}: 数值超出schema范围"));
            }
        }
        Some("string") => {
            if let Some(text)=value.as_str() {
                let count=text.chars().count() as u64;
                if schema.get("maxLength").and_then(Value::as_u64).is_some_and(|max|count>max) {
                    issues.push(format!("{path}: 字符数{count}超过maxLength={}（含标点，须精简且保留有据事实）",schema["maxLength"]));
                }
                if schema.get("minLength").and_then(Value::as_u64).is_some_and(|min|count<min) {
                    issues.push(format!("{path}: 字符数少于minLength"));
                }
            } else {
                issues.push(format!("{path}: 必须为string，不接受null/对象/数组"));
            }
        }
        Some("boolean") => {
            if !value.is_boolean() {
                issues.push(format!("{path}: 必须为boolean"));
            }
        }
        _ => issues.push(format!("{path}: 应用schema类型不支持")),
    }
}

/// Text AI stages understand the original-language evidence via the configured DeepSeek API.
pub async fn json<T: DeserializeOwned>(
    options: &AiOptions,
    system: &str,
    user: &str,
    schema: Value,
) -> Result<T, String> {
    json_with_limit(options, system, user, schema, 8192).await
}

/// Explicit output budget for long-form stages; truncated responses still fail closed.
pub async fn json_with_limit<T: DeserializeOwned>(
    options: &AiOptions,
    system: &str,
    user: &str,
    schema: Value,
    max_tokens: u64,
) -> Result<T, String> {
    let base = ai::validate_base_url(&options.base_url)?;
    ai::validate_model_endpoint(&base, &options.model)?;
    if options.model.trim().is_empty() || options.api_key.trim().is_empty() {
        return Err("DeepSeek 模型或 API Key 未配置".into());
    }
    let http =
        ai::client(ai::GENERATION_TIMEOUT).map_err(|_| "无法创建 AI HTTP 客户端".to_string())?;
    let client = deepseek::Client::builder()
        .api_key(&options.api_key)
        .base_url(&base)
        .http_client(http)
        .build()
        .map_err(|_| "无法创建 DeepSeek 客户端，请检查后端配置".to_string())?;
    let value = schema_response(&schema, |feedback| {
        let preamble = format!("{}\n{feedback}", system_prompt(system, &schema));
        let client = &client;
        async move {
            let response = client
                .completion_model(options.model.trim())
                .completion_request(user)
                .preamble(preamble)
                .temperature(0.2)
                .max_tokens(max_tokens)
                .additional_params(json!({"response_format":{"type":"json_object"},"thinking":{"type":"disabled"}}))
                .send().await.map_err(|error| {
                    if let Some(status) = error.provider_response_status() {
                        ai::response_error(status, error.provider_response_body())
                    } else { "DeepSeek 请求失败：请检查网络、模型和接口配置（响应详情已隐藏）".into() }
                })?;
            // Truncation, blank/invalid JSON, tool calls and HTTP failures are NOT
            // schema repairs and must not trigger an expensive blind retry.
            visible_json::<Value>(response)
        }
    }).await?;
    serde_json::from_value(value)
        .map_err(|_| "结构化输出类型校验失败（正文已隐藏）；未发布此版本".into())
}

async fn schema_response<F, Fut>(schema: &Value, mut request: F) -> Result<Value, String>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Result<Value, String>>,
{
    let mut feedback = String::new();
    for attempt in 0..2 {
        let value = request(feedback).await?;
        let issues = schema_issues(&value, schema);
        if issues.is_empty() {
            return Ok(value);
        }
        let detail = issues.join("；");
        if attempt == 1 {
            return Err(format!(
                "模型JSON结构在一次格式重试后仍不合格：{detail}（正文已隐藏）；未发布此版本"
            ));
        }
        feedback=format!("这是唯一一次JSON格式重试。上次格式问题：{detail}。重新完成相同任务，严格使用schema字段；顶层不要包result/report/data，不加解释字段；整数不用引号，数组不能是null。不得伪造证据、补出通过项或修改任务要求来满足结构；没有问题的可选数组按schema返回[]，但必填逐段评审不得遗漏。此次不是新增业务修稿轮次。");
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn unicode_length_limit_retries_overlong_text_once() {
        let schema=json!({"type":"object","properties":{"text":{"type":"string","minLength":1,"maxLength":102}},"required":["text"],"additionalProperties":false});
        let mut calls=0;
        let value=schema_response(&schema,|feedback|{
            calls+=1;
            if calls==2{assert!(feedback.contains("maxLength=102"));}
            std::future::ready(Ok(json!({"text":"甲".repeat(if calls==1{113}else{102})})))
        }).await.unwrap();
        assert_eq!(calls,2);assert_eq!(value["text"].as_str().unwrap().chars().count(),102);
        let unicode=json!({"type":"string","maxLength":3,"minLength":1});
        assert!(schema_issues(&json!("中😀，"),&unicode).is_empty());
        assert!(!schema_issues(&json!("中😀，。"),&unicode).is_empty());
        assert!(!schema_issues(&json!(""),&unicode).is_empty());
    }
    #[tokio::test]
    async fn deepseek_wire_contract_uses_bearer_json_mode_and_no_ollama_parameters() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
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
                        assert!(headers
                            .to_lowercase()
                            .contains("authorization: bearer test-only"));
                        let body: Value =
                            serde_json::from_slice(&bytes[boundary + 4..boundary + 4 + length])
                                .unwrap();
                        let response = json!({"id":"test","object":"chat.completion","created":0,"model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"{\"ok\":true}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}).to_string();
                        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
                        return (first, body);
                    }
                }
            }
        });
        let options = AiOptions {
            base_url: base,
            model: "test-model".into(),
            style: String::new(),
            duration: 0.0,
            api_key: "test-only".into(),
        };
        let schema = json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false});
        assert_eq!(
            json::<Value>(&options, "synthetic test", "test", schema)
                .await
                .unwrap()["ok"],
            true
        );
        let (path, body) = server.await.unwrap();
        assert!(path.contains("/v1/chat/completions "));
        assert_eq!(body["response_format"]["type"], "json_object");
        assert_eq!(body["thinking"]["type"], "disabled");
        assert_eq!(body["max_tokens"], 8192);
        assert!(body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("JSON Schema"));
        for field in ["think", "keep_alive", "num_ctx", "format", "options"] {
            assert!(body.get(field).is_none());
        }
    }
    #[tokio::test]
    async fn provider_failures_hide_bodies_and_do_not_follow_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for status in [
            "401 Unauthorized",
            "429 Too Many Requests",
            "307 Temporary Redirect",
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
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
                            break;
                        }
                    }
                }
                let body = "sensitive-provider-body";
                socket.write_all(format!("HTTP/1.1 {status}\r\nLocation: http://127.0.0.1:1/stolen\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
            let options = AiOptions {
                base_url,
                model: "test-model".into(),
                style: String::new(),
                duration: 0.0,
                api_key: "test-only-secret".into(),
            };
            let error = json::<Value>(&options, "test", "test", json!({"type":"object"}))
                .await
                .unwrap_err();
            server.await.unwrap();
            assert!(!error.contains("sensitive-provider-body"));
            assert!(!error.contains("test-only-secret"));
            assert!(error.contains(&status[..3]), "{error}");
        }
    }

    #[test]
    fn rejects_truncated_blank_and_reasoning_only_responses() {
        for (parts, finish) in [
            (vec![AssistantContent::text("{}")], FinishReason::Length),
            (vec![AssistantContent::text("   ")], FinishReason::Stop),
            (vec![AssistantContent::reasoning("{}")], FinishReason::Stop),
        ] {
            let response = CompletionResponse::new(parts, Default::default(), "test")
                .with_finish_reason(finish);
            assert!(visible_json::<Value>(response).is_err());
        }
        let response = CompletionResponse::new(
            vec![AssistantContent::text("{\"ok\":true}")],
            Default::default(),
            "test",
        )
        .with_finish_reason(FinishReason::Stop);
        assert_eq!(visible_json::<Value>(response).unwrap()["ok"], true);
    }
    #[test]
    fn json_mode_results_still_obey_the_application_schema() {
        let schema = json!({"type":"object","properties":{"items":{"type":"array","maxItems":1,"items":{"type":"object","properties":{"score":{"type":"integer","minimum":1,"maximum":5},"verdict":{"type":"string","enum":["pass","revise"]}},"required":["score","verdict"],"additionalProperties":false}}},"required":["items"],"additionalProperties":false});
        assert!(matches_schema(
            &json!({"items":[{"score":3,"verdict":"pass"}]}),
            &schema
        ));
        for value in [
            json!({}),
            json!({"items":[],"extra":true}),
            json!({"items":[{"score":6,"verdict":"pass"}]}),
            json!({"items":[{"score":1.5,"verdict":"pass"}]}),
            json!({"items":[{"score":3,"verdict":"unknown"}]}),
            json!({"items":[{},{}]}),
        ] {
            assert!(!matches_schema(&value, &schema));
        }
        assert!(!matches_schema(
            &json!("test"),
            &json!({"type":"string","pattern":"test"})
        ));
    }

    #[test]
    fn schema_diagnostics_locate_fields_without_echoing_model_values_or_unknown_keys() {
        let schema = json!({"type":"object","properties":{"findings":{"type":"array","items":{"type":"object","properties":{"paragraph_index":{"type":"integer"},"verdict":{"type":"string","enum":["pass","revise"]}},"required":["paragraph_index","verdict"],"additionalProperties":false}},"coherence":{"type":"array","items":{"type":"integer"}}},"required":["findings","coherence"],"additionalProperties":false});
        let value = json!({"findings":[{"paragraph_index":"secret-number","verdict":"secret-prose","secret-key":"secret-value"}]});
        let issues = schema_issues(&value, &schema).join(";");
        assert!(issues.contains("$.coherence"));
        assert!(issues.contains("$.findings[0].paragraph_index"));
        assert!(!issues.contains("secret"));
    }

    #[tokio::test]
    async fn one_schema_retry_gets_safe_feedback_and_must_pass_original_rules() {
        let schema = json!({"type":"object","properties":{"id":{"type":"integer"}},"required":["id"],"additionalProperties":false});
        let mut calls = 0;
        let value = schema_response(&schema, |feedback| {
            calls += 1;
            if calls == 1 {
                assert!(feedback.is_empty());
            } else {
                assert!(feedback.contains("$.id"));
                assert!(!feedback.contains("secret-response"));
            }
            std::future::ready(Ok(if calls == 1 {
                json!({"id":"secret-response"})
            } else {
                json!({"id":2})
            }))
        })
        .await
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(value["id"], 2);
    }

    #[tokio::test]
    async fn schema_retry_is_bounded_and_transport_errors_do_not_retry() {
        let schema = json!({"type":"integer"});
        let mut calls = 0;
        let error = schema_response(&schema, |_| {
            calls += 1;
            std::future::ready(Ok(json!("secret-response")))
        })
        .await
        .unwrap_err();
        assert_eq!(calls, 2);
        assert!(!error.contains("secret-response"));
        assert!(error.contains("一次格式重试"));
        calls = 0;
        assert!(schema_response(&schema, |_| {
            calls += 1;
            std::future::ready(Err("HTTP failure".into()))
        })
        .await
        .is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn parsing_errors_do_not_echo_response_secrets() {
        let response = CompletionResponse::new(
            vec![AssistantContent::text("sensitive-response")],
            Default::default(),
            "test",
        )
        .with_finish_reason(FinishReason::Stop);
        assert!(!visible_json::<Value>(response)
            .unwrap_err()
            .contains("sensitive-response"));
    }
}
