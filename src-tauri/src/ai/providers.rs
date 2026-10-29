use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;

use crate::storage::models::ProviderConfig;

/// Default models used when a provider has no model configured.
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-5-mini";
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-5";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";
pub const DEFAULT_OLLAMA_MODEL: &str = "llama3.2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiOperation {
    DocumentGeneration,
    Translation,
}

#[derive(Debug, Clone)]
pub struct GenerateOptions {
    pub allow_thinking: bool,
    pub custom_params: Option<serde_json::Value>,
}

impl Default for GenerateOptions {
    fn default() -> Self {
        Self {
            allow_thinking: true,
            custom_params: None,
        }
    }
}

/// Token counts reported by the provider for one request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl std::ops::AddAssign for TokenUsage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub text: String,
    /// `None` when the server did not report usage (some local servers).
    pub usage: Option<TokenUsage>,
}

pub fn get_ai_generate_options(
    db: &crate::storage::Database,
    operation: AiOperation,
) -> GenerateOptions {
    let thinking_mode = db
        .get_setting("ai_thinking_mode")
        .unwrap_or(None)
        .unwrap_or_else(|| "auto".to_string());

    let custom_params = db
        .get_setting("ai_custom_parameters")
        .unwrap_or(None)
        .and_then(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                serde_json::from_str::<serde_json::Value>(trimmed).ok()
            }
        });

    let allow_thinking = match operation {
        // Regola esplicita: "la traduzione non deve pensare, e la traduzione è l' unica con no think"
        AiOperation::Translation => false,
        AiOperation::DocumentGeneration => thinking_mode != "no_think",
    };

    GenerateOptions {
        allow_thinking,
        custom_params,
    }
}

pub fn clean_ai_output(raw: &str) -> String {
    let Ok(re_think) = regex::Regex::new(r"(?is)<think>.*?</think>") else {
        return raw.trim().to_string();
    };
    let Ok(re_thought) = regex::Regex::new(r"(?is)<thought>.*?</thought>") else {
        return raw.trim().to_string();
    };
    let Ok(re_stray) = regex::Regex::new(r"(?is)</?(think|thought)>") else {
        return raw.trim().to_string();
    };

    let step1 = re_think.replace_all(raw, "");
    let step2 = re_thought.replace_all(&step1, "");
    re_stray.replace_all(&step2, "").trim().to_string()
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn generate(&self, system: &str, prompt: &str, options: &GenerateOptions) -> Result<LlmResponse>;
}

pub fn build_provider(config: &ProviderConfig) -> Result<Box<dyn LlmProvider>> {
    match config.provider_type.as_str() {
        "openai" => Ok(Box::new(OpenAiProvider::new(config.clone()))),
        "claude" => Ok(Box::new(ClaudeProvider::new(config.clone()))),
        "ollama" => Ok(Box::new(OllamaProvider::new(config.clone()))),
        "gemini" => Ok(Box::new(GeminiProvider::new(config.clone()))),
        other => anyhow::bail!("tipo di provider non supportato: {other}"),
    }
}

/// No overall request timeout: long documents must be allowed to finish. The pipeline applies
/// its own (user-configurable) deadline; here we only give up when the server goes silent.
fn create_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .read_timeout(std::time::Duration::from_secs(300)) // 5 minuti senza dati (LLM locali lenti)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

const NO_THINK_SUFFIX: &str = "\n\nIMPORTANT: Do NOT output thinking steps, reasoning chains, or <think>...</think> tags. Respond immediately and strictly with the final result.";

fn effective_system(system: &str, options: &GenerateOptions) -> String {
    if options.allow_thinking {
        system.to_string()
    } else {
        format!("{system}{NO_THINK_SUFFIX}")
    }
}

/// Merges the user's custom JSON parameters into `target`, skipping the protected keys.
fn merge_custom_params(target: &mut serde_json::Value, options: &GenerateOptions, protected: &[&str]) {
    let (Some(serde_json::Value::Object(custom_map)), Some(obj)) =
        (&options.custom_params, target.as_object_mut())
    else {
        return;
    };
    for (k, v) in custom_map {
        if !protected.contains(&k.as_str()) {
            obj.insert(k.clone(), v.clone());
        }
    }
}

/// Removes thinking-related overrides (e.g. coming from custom params) when thinking is off.
fn strip_thinking_params(target: &mut serde_json::Value, options: &GenerateOptions) {
    if options.allow_thinking {
        return;
    }
    if let Some(obj) = target.as_object_mut() {
        obj.remove("thinking");
        obj.remove("chat_template_kwargs");
        obj.remove("reasoning_effort");
    }
}

/// Reasoning models reject sampling parameters (`temperature`, `top_p`, `top_k`) with a 400:
/// OpenAI GPT-5 / o-series and Claude Opus 4.7+, Sonnet 5, Fable. Names are also matched in
/// the dotted form used by proxies such as GitHub Copilot ("claude-opus-4.7").
fn supports_sampling_params(model: &str) -> bool {
    let lower = model.to_ascii_lowercase().replace('.', "-");
    let m = lower.rsplit('/').next().unwrap_or(&lower);
    let reasoning_only = m.starts_with("gpt-5")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
        || m.starts_with("claude-opus-4-7")
        || m.starts_with("claude-opus-4-8")
        || m.starts_with("claude-opus-5")
        || m.starts_with("claude-sonnet-5")
        || m.starts_with("claude-fable")
        || m.starts_with("claude-mythos");
    !reasoning_only
}

/// Drops sampling parameters (e.g. a saved `{"temperature": 0.2}` custom parameter) that the
/// model would reject.
fn strip_unsupported_sampling(target: &mut serde_json::Value, model: &str) {
    if supports_sampling_params(model) {
        return;
    }
    if let Some(obj) = target.as_object_mut() {
        for key in ["temperature", "top_p", "top_k"] {
            if obj.remove(key).is_some() {
                crate::logger::info("AI", &format!("Parametro '{key}' ignorato: non supportato da {model}"));
            }
        }
    }
}

fn usage_from_openai(json_val: &serde_json::Value) -> Option<TokenUsage> {
    let usage = json_val.get("usage")?;
    Some(TokenUsage {
        input_tokens: usage.get("prompt_tokens")?.as_u64()?,
        output_tokens: usage.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

pub struct OpenAiProvider {
    client: reqwest::Client,
    config: ProviderConfig,
}

impl OpenAiProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            client: create_http_client(),
            config,
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn generate(&self, system: &str, prompt: &str, options: &GenerateOptions) -> Result<LlmResponse> {
        let api_key = self
            .config
            .api_key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .unwrap_or_else(|| "not-needed".to_string());

        let raw_base = self
            .config
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let clean_base = raw_base.trim().trim_end_matches('/');
        let url = if clean_base.ends_with("/v1") || clean_base.contains("/v1/") || clean_base.starts_with("https://api.openai.com") {
            format!("{clean_base}/chat/completions")
        } else {
            format!("{clean_base}/v1/chat/completions")
        };

        let model = self
            .config
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());

        // No max_tokens: the model may write the whole document, however long.
        let mut request_payload = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": effective_system(system, options)},
                {"role": "user", "content": prompt}
            ]
        });
        if supports_sampling_params(&model) {
            request_payload["temperature"] = json!(0.2);
        }

        merge_custom_params(&mut request_payload, options, &["messages"]);
        strip_thinking_params(&mut request_payload, options);
        strip_unsupported_sampling(&mut request_payload, &model);

        let service = format!("OpenAI/Local ({model})");
        let payload_str = serde_json::to_string_pretty(&request_payload).unwrap_or_default();

        crate::logger::log_api_request(
            &format!("{service} [think={}]", options.allow_thinking),
            "POST",
            &url,
            Some(&[("Authorization", &format!("Bearer {api_key}")), ("Content-Type", "application/json")]),
            &payload_str,
        );

        let start_time = std::time::Instant::now();
        let send_res = self
            .client
            .post(&url)
            .bearer_auth(api_key)
            .json(&request_payload)
            .send()
            .await;
        let duration_ms = start_time.elapsed().as_millis();

        let response = match send_res {
            Ok(resp) => resp,
            Err(e) => {
                let err_msg = if e.is_timeout() {
                    format!(
                        "Timeout di connessione superato (oltre 5 minuti senza risposta) verso il server AI ({url}). \
                        Il modello locale potrebbe essere troppo lento o sovraccarico."
                    )
                } else if e.is_connect() {
                    format!(
                        "Impossibile stabilire la connessione con il server AI ({url}). \
                        Verifica che LM Studio o il server locale sia avviato e in ascolto sull'indirizzo specificato."
                    )
                } else {
                    format!("Errore di comunicazione verso il server AI ({url}): {e}")
                };
                crate::logger::log_api_error(&service, &url, duration_ms, &err_msg);
                anyhow::bail!("{err_msg}");
            }
        };

        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            crate::logger::log_api_error(&service, &url, duration_ms, &format!("HTTP {status}: {body_text}"));
            anyhow::bail!("Il server AI su {url} ha restituito un errore HTTP {status}: {body_text}");
        }

        crate::logger::log_api_response(&service, &url, status.as_u16(), duration_ms, &body_text);

        let json_val: serde_json::Value = serde_json::from_str(&body_text)
            .context("Risposta non valida dal server AI (formato JSON non riconosciuto)")?;

        let choice = &json_val["choices"][0];
        let raw_content = choice["message"]["content"]
            .as_str()
            .context("Contenuto mancante nella risposta del modello AI")?;
        if choice["finish_reason"].as_str() == Some("length") {
            crate::logger::warn(&service, "La risposta è stata troncata dal limite di token del modello/server.");
        }

        Ok(LlmResponse {
            text: clean_ai_output(raw_content),
            usage: usage_from_openai(&json_val),
        })
    }
}

/// Largest `max_tokens` each Claude generation accepts. We always ask for the maximum so long
/// documents are never cut off (you only pay for the tokens actually generated).
/// Can still be overridden with `max_tokens` in the custom parameters.
fn claude_max_output_tokens(model: &str) -> u64 {
    let m = model.to_ascii_lowercase();
    if m.contains("claude-3-haiku") || m.contains("claude-3-opus") || m.contains("claude-3-sonnet") {
        4_096
    } else if m.contains("claude-3-5") {
        8_192
    } else if m.contains("opus-4-0") || m.contains("opus-4-1") || m.contains("claude-opus-4-2025") {
        32_000
    } else if m.contains("claude-3-7")
        || m.contains("sonnet-4-0")
        || m.contains("claude-sonnet-4-2025")
        || m.contains("sonnet-4-5")
        || m.contains("opus-4-5")
        || m.contains("haiku-4")
    {
        64_000
    } else {
        // Claude 4.6+ / 5 family (Opus, Sonnet, Fable) and newer.
        128_000
    }
}

/// Result of reading a Claude streaming (SSE) response.
#[derive(Debug, Default, PartialEq)]
struct ClaudeStreamResult {
    text: String,
    stop_reason: Option<String>,
    usage: TokenUsage,
}

/// Parses the server-sent events of a streamed Messages API response. Only `text` deltas are
/// kept: thinking blocks (on by default for newer models) are skipped.
fn parse_claude_sse(body: &str) -> Result<ClaudeStreamResult> {
    let mut result = ClaudeStreamResult::default();
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<serde_json::Value>(data.trim()) else {
            continue;
        };
        match event["type"].as_str() {
            Some("message_start") => {
                let usage = &event["message"]["usage"];
                result.usage.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0)
                    + usage["cache_creation_input_tokens"].as_u64().unwrap_or(0)
                    + usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
            }
            Some("content_block_delta") => {
                if event["delta"]["type"].as_str() == Some("text_delta") {
                    if let Some(text) = event["delta"]["text"].as_str() {
                        result.text.push_str(text);
                    }
                }
            }
            Some("message_delta") => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    result.stop_reason = Some(reason.to_string());
                }
                if let Some(out) = event["usage"]["output_tokens"].as_u64() {
                    result.usage.output_tokens = out;
                }
            }
            Some("error") => {
                let msg = event["error"]["message"].as_str().unwrap_or("errore sconosciuto");
                anyhow::bail!("Claude ha interrotto la risposta: {msg}");
            }
            _ => {}
        }
    }
    Ok(result)
}

pub struct ClaudeProvider {
    client: reqwest::Client,
    config: ProviderConfig,
}

impl ClaudeProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            client: create_http_client(),
            config,
        }
    }
}

#[async_trait]
impl LlmProvider for ClaudeProvider {
    async fn generate(&self, system: &str, prompt: &str, options: &GenerateOptions) -> Result<LlmResponse> {
        let api_key = self
            .config
            .api_key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .context("chiave API di Claude non configurata")?;
        let base_url = self
            .config
            .base_url
            .clone()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string());
        let model = self
            .config
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CLAUDE_MODEL.to_string());

        let url = format!("{}/messages", base_url.trim().trim_end_matches('/'));

        // Streaming: with a very large max_tokens a non-streamed request can exceed HTTP
        // timeouts; the stream keeps the connection alive until the document is complete.
        let mut request_payload = json!({
            "model": model,
            "max_tokens": claude_max_output_tokens(&model),
            "stream": true,
            "system": effective_system(system, options),
            "messages": [
                {"role": "user", "content": prompt}
            ]
        });

        merge_custom_params(&mut request_payload, options, &["messages", "system", "stream"]);
        strip_thinking_params(&mut request_payload, options);
        strip_unsupported_sampling(&mut request_payload, &model);

        let service = format!("Anthropic Claude ({model})");
        let payload_str = serde_json::to_string_pretty(&request_payload).unwrap_or_default();

        crate::logger::log_api_request(
            &format!("{service} [think={}]", options.allow_thinking),
            "POST",
            &url,
            Some(&[("x-api-key", &api_key), ("anthropic-version", "2023-06-01"), ("Content-Type", "application/json")]),
            &payload_str,
        );

        let start_time = std::time::Instant::now();
        let send_res = self
            .client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&request_payload)
            .send()
            .await;

        let response = match send_res {
            Ok(resp) => resp,
            Err(e) => {
                let err_msg = format!("Errore di rete verso Claude ({url}): {e}");
                crate::logger::log_api_error(&service, &url, start_time.elapsed().as_millis(), &err_msg);
                anyhow::bail!("{err_msg}");
            }
        };

        let status = response.status();
        let body_text = match response.text().await {
            Ok(body) => body,
            Err(e) => {
                let err_msg = format!("Connessione con Claude interrotta durante la risposta ({url}): {e}");
                crate::logger::log_api_error(&service, &url, start_time.elapsed().as_millis(), &err_msg);
                anyhow::bail!("{err_msg}");
            }
        };
        let duration_ms = start_time.elapsed().as_millis();

        if !status.is_success() {
            crate::logger::log_api_error(&service, &url, duration_ms, &format!("HTTP {status}: {body_text}"));
            anyhow::bail!("Claude API su {url} ha restituito un errore HTTP {status}: {body_text}");
        }

        // Proxies that ignore `stream` answer with a regular JSON message.
        let result = match serde_json::from_str::<serde_json::Value>(&body_text) {
            Ok(message) if message["type"].as_str() == Some("message") => ClaudeStreamResult {
                text: message["content"]
                    .as_array()
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter(|b| b["type"].as_str() == Some("text"))
                            .filter_map(|b| b["text"].as_str())
                            .collect::<String>()
                    })
                    .unwrap_or_default(),
                stop_reason: message["stop_reason"].as_str().map(str::to_string),
                usage: TokenUsage {
                    input_tokens: message["usage"]["input_tokens"].as_u64().unwrap_or(0),
                    output_tokens: message["usage"]["output_tokens"].as_u64().unwrap_or(0),
                },
            },
            _ => parse_claude_sse(&body_text)?,
        };

        crate::logger::log_api_response(
            &service,
            &url,
            status.as_u16(),
            duration_ms,
            &format!(
                "stop_reason={:?}, input_tokens={}, output_tokens={}\n{}",
                result.stop_reason, result.usage.input_tokens, result.usage.output_tokens, result.text
            ),
        );

        match result.stop_reason.as_deref() {
            Some("refusal") => anyhow::bail!("Claude ha rifiutato di completare la richiesta."),
            Some("max_tokens") => crate::logger::warn(
                &service,
                "La risposta ha raggiunto il limite massimo di token del modello ed è stata troncata.",
            ),
            _ => {}
        }

        if result.text.trim().is_empty() {
            anyhow::bail!("risposta di Claude senza contenuto");
        }

        Ok(LlmResponse {
            text: clean_ai_output(&result.text),
            usage: Some(result.usage),
        })
    }
}

pub struct OllamaProvider {
    client: reqwest::Client,
    config: ProviderConfig,
}

impl OllamaProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            client: create_http_client(),
            config,
        }
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn generate(&self, system: &str, prompt: &str, options: &GenerateOptions) -> Result<LlmResponse> {
        let base_url = self
            .config
            .base_url
            .clone()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        let clean_base = base_url.trim().trim_end_matches('/').to_string();
        let model = self
            .config
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_string());

        let system_text = effective_system(system, options);

        // 1. Prova l'endpoint nativo di Ollama: /api/chat
        let chat_url = format!("{clean_base}/api/chat");
        let mut chat_payload = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system_text},
                {"role": "user", "content": prompt}
            ],
            "think": false,
            "stream": false
        });
        merge_custom_params(&mut chat_payload, options, &["messages"]);

        let chat_payload_str = serde_json::to_string_pretty(&chat_payload).unwrap_or_default();
        crate::logger::log_api_request(
            &format!("Ollama Native ({model}) [think={}]", options.allow_thinking),
            "POST",
            &chat_url,
            None,
            &chat_payload_str,
        );

        let start_time = std::time::Instant::now();
        let res = self.client.post(&chat_url).json(&chat_payload).send().await;
        let duration_ms = start_time.elapsed().as_millis();

        if let Ok(resp) = res {
            let status = resp.status();
            if let Ok(body_text) = resp.text().await {
                if status.is_success() {
                    crate::logger::log_api_response(
                        &format!("Ollama Native ({model})"),
                        &chat_url,
                        status.as_u16(),
                        duration_ms,
                        &body_text,
                    );
                    if let Ok(json_res) = serde_json::from_str::<serde_json::Value>(&body_text) {
                        if let Some(content) = json_res["message"]["content"].as_str() {
                            let usage = json_res["prompt_eval_count"].as_u64().map(|input| TokenUsage {
                                input_tokens: input,
                                output_tokens: json_res["eval_count"].as_u64().unwrap_or(0),
                            });
                            return Ok(LlmResponse {
                                text: clean_ai_output(content),
                                usage,
                            });
                        }
                    }
                } else {
                    crate::logger::log_api_error(
                        &format!("Ollama Native ({model})"),
                        &chat_url,
                        duration_ms,
                        &format!("HTTP {status}: {body_text}"),
                    );
                }
            }
        }

        // 2. Fallback all'endpoint OpenAI-compatibile: /v1/chat/completions (LM Studio, LocalAI, vLLM, Ollama)
        let v1_url = if clean_base.ends_with("/v1") {
            format!("{clean_base}/chat/completions")
        } else {
            format!("{clean_base}/v1/chat/completions")
        };

        let mut v1_payload = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system_text},
                {"role": "user", "content": prompt}
            ],
            "think": false,
            "stream": false
        });
        merge_custom_params(&mut v1_payload, options, &["messages"]);
        strip_thinking_params(&mut v1_payload, options);

        let v1_payload_str = serde_json::to_string_pretty(&v1_payload).unwrap_or_default();

        let auth_header = self.config.api_key.as_deref().filter(|k| !k.trim().is_empty());
        let headers_log = auth_header.map(|k| vec![("Authorization", k)]);

        let service = format!("Ollama/Local V1 ({model})");
        crate::logger::log_api_request(
            &format!("{service} [think={}]", options.allow_thinking),
            "POST",
            &v1_url,
            headers_log.as_deref(),
            &v1_payload_str,
        );

        let mut req = self.client.post(&v1_url).json(&v1_payload);
        if let Some(key) = auth_header {
            req = req.bearer_auth(key);
        }

        let start_v1 = std::time::Instant::now();
        let send_res = req.send().await;
        let duration_v1_ms = start_v1.elapsed().as_millis();

        let response = match send_res {
            Ok(resp) => resp,
            Err(e) => {
                let err_msg = if e.is_timeout() {
                    format!("Timeout scaduto (oltre 5 minuti senza risposta) verso il server AI ({v1_url}).")
                } else if e.is_connect() {
                    format!("Impossibile connettersi al server locale su {clean_base}. Verifica che sia attivo.")
                } else {
                    format!("Errore di rete verso il server AI ({v1_url}): {e}")
                };
                crate::logger::log_api_error(&service, &v1_url, duration_v1_ms, &err_msg);
                anyhow::bail!("{err_msg}");
            }
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            crate::logger::log_api_error(&service, &v1_url, duration_v1_ms, &format!("HTTP {status}: {body}"));
            anyhow::bail!("Il server AI su {v1_url} ha risposto con errore HTTP {status}: {body}");
        }

        crate::logger::log_api_response(&service, &v1_url, status.as_u16(), duration_v1_ms, &body);

        let json_val: serde_json::Value = serde_json::from_str(&body).context("risposta JSON non valida da Ollama/Local LLM")?;
        let raw_content = json_val["choices"][0]["message"]["content"]
            .as_str()
            .context("contenuto mancante nella risposta di Ollama/Local LLM")?;

        Ok(LlmResponse {
            text: clean_ai_output(raw_content),
            usage: usage_from_openai(&json_val),
        })
    }
}

pub struct GeminiProvider {
    client: reqwest::Client,
    config: ProviderConfig,
}

impl GeminiProvider {
    pub fn new(config: ProviderConfig) -> Self {
        Self {
            client: create_http_client(),
            config,
        }
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn generate(&self, system: &str, prompt: &str, options: &GenerateOptions) -> Result<LlmResponse> {
        let api_key = self
            .config
            .api_key
            .clone()
            .filter(|k| !k.trim().is_empty())
            .context("chiave API di Gemini non configurata")?;
        let base_url = self
            .config
            .base_url
            .clone()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".to_string());
        let model = self
            .config
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_GEMINI_MODEL.to_string());

        let url = format!("{}/models/{model}:generateContent", base_url.trim().trim_end_matches('/'));

        let mut payload = json!({
            "systemInstruction": {
                "parts": [{ "text": effective_system(system, options) }]
            },
            "contents": [{
                "role": "user",
                "parts": [{ "text": prompt }]
            }],
            "generationConfig": {
                "temperature": 0.2
            }
        });

        if let Some(gen_cfg) = payload.get_mut("generationConfig") {
            merge_custom_params(gen_cfg, options, &[]);
            if !options.allow_thinking {
                gen_cfg["thinkingConfig"] = json!({ "thinkingBudget": 0 });
            }
        }

        let service = format!("Google Gemini ({model})");
        let payload_str = serde_json::to_string_pretty(&payload).unwrap_or_default();

        crate::logger::log_api_request(
            &format!("{service} [think={}]", options.allow_thinking),
            "POST",
            &url,
            Some(&[("x-goog-api-key", &api_key), ("Content-Type", "application/json")]),
            &payload_str,
        );

        let start_time = std::time::Instant::now();
        let send_res = self
            .client
            .post(&url)
            .header("x-goog-api-key", api_key)
            .json(&payload)
            .send()
            .await;
        let duration_ms = start_time.elapsed().as_millis();

        let response = match send_res {
            Ok(r) => r,
            Err(e) => {
                let err_msg = format!("Errore di rete verso Gemini ({url}): {e}");
                crate::logger::log_api_error(&service, &url, duration_ms, &err_msg);
                anyhow::bail!("{err_msg}");
            }
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            crate::logger::log_api_error(&service, &url, duration_ms, &format!("HTTP {status}: {body}"));
            anyhow::bail!("Gemini API su {url} ha risposto con errore HTTP {status}: {body}");
        }

        crate::logger::log_api_response(&service, &url, status.as_u16(), duration_ms, &body);

        let json_val: serde_json::Value = serde_json::from_str(&body).context("JSON non valido da Gemini")?;
        // Join every non-thought text part (a response can be split across several parts).
        let raw_content: String = json_val["candidates"][0]["content"]["parts"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter(|p| p["thought"].as_bool() != Some(true))
                    .filter_map(|p| p["text"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        if raw_content.trim().is_empty() {
            anyhow::bail!("risposta di Gemini senza contenuto");
        }

        let usage_meta = &json_val["usageMetadata"];
        let usage = usage_meta["promptTokenCount"].as_u64().map(|input| TokenUsage {
            input_tokens: input,
            output_tokens: usage_meta["candidatesTokenCount"].as_u64().unwrap_or(0)
                + usage_meta["thoughtsTokenCount"].as_u64().unwrap_or(0),
        });

        Ok(LlmResponse {
            text: clean_ai_output(&raw_content),
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_max_tokens_per_generation() {
        assert_eq!(claude_max_output_tokens("claude-opus-5"), 128_000);
        assert_eq!(claude_max_output_tokens("claude-sonnet-5"), 128_000);
        assert_eq!(claude_max_output_tokens("claude-haiku-4-5"), 64_000);
        assert_eq!(claude_max_output_tokens("claude-sonnet-4-5-20250929"), 64_000);
        assert_eq!(claude_max_output_tokens("claude-3-5-haiku-latest"), 8_192);
    }

    #[test]
    fn parses_claude_stream_and_skips_thinking() {
        let body = r##"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":120,"output_tokens":1}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"# Guida"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":" completa"}}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}
"##;
        let parsed = parse_claude_sse(body).unwrap();
        assert_eq!(parsed.text, "# Guida completa");
        assert_eq!(parsed.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(parsed.usage, TokenUsage { input_tokens: 120, output_tokens: 42 });
    }

    #[test]
    fn claude_stream_error_is_reported() {
        let body = "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n";
        assert!(parse_claude_sse(body).is_err());
    }

    #[test]
    fn sampling_params_are_dropped_for_reasoning_models() {
        assert!(!supports_sampling_params("gpt-5-mini"));
        assert!(!supports_sampling_params("openai/o3"));
        assert!(!supports_sampling_params("claude-opus-5"));
        assert!(!supports_sampling_params("claude-opus-4.7"));
        assert!(supports_sampling_params("gpt-4.1"));
        assert!(supports_sampling_params("claude-sonnet-4.5"));

        let mut payload = json!({"model": "claude-opus-5", "temperature": 0.2, "top_p": 0.9});
        strip_unsupported_sampling(&mut payload, "claude-opus-5");
        assert!(payload.get("temperature").is_none() && payload.get("top_p").is_none());
    }
}
