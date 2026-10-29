use std::path::Path;
use anyhow::{Context, Result};
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::storage::models::ProviderConfig;

/// A single timed speech segment from Whisper, with start/end in milliseconds and a confidence score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// Whisper avg_logprob for the segment (0.0 = unavailable). More negative = less confident.
    pub avg_logprob: f32,
}

/// Full result of a timed transcription.
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    /// Concatenated full transcript (same as returned by plain /v1/audio/transcriptions).
    pub text: String,
    /// Per-segment timed data. Empty when the provider doesn't support segments (e.g. cloud APIs).
    pub segments: Vec<AudioSegment>,
}

#[allow(dead_code)]
pub async fn transcribe_audio(
    provider: &ProviderConfig,
    audio_path: &Path,
    language: Option<&str>,
) -> Result<String> {
    transcribe_audio_with_segments(provider, audio_path, language)
        .await
        .map(|r| r.text)
}

/// Like `transcribe_audio` but also returns per-segment timing data when available.
/// For local faster-whisper it calls the `/segments` endpoint; for cloud providers it
/// falls back to the standard endpoint and returns an empty `segments` vec.
pub async fn transcribe_audio_with_segments(
    provider: &ProviderConfig,
    audio_path: &Path,
    language: Option<&str>,
) -> Result<TranscriptionResult> {
    let bytes = std::fs::read(audio_path)
        .with_context(|| format!("impossibile leggere il file audio {}", audio_path.display()))?;

    if bytes.is_empty() {
        anyhow::bail!("il file audio è vuoto");
    }

    match provider.provider_type.as_str() {
        "openai" => transcribe_openai_audio(provider, audio_path, bytes, language).await,
        "gemini" => {
            let text = transcribe_gemini(provider, audio_path, bytes, language).await?;
            Ok(TranscriptionResult { text, segments: vec![] })
        }
        "ollama" => {
            // Try OpenAI-compatible whisper endpoint first
            match transcribe_openai_audio(provider, audio_path, bytes.clone(), language).await {
                Ok(result) => Ok(result),
                Err(_) => {
                    let text = transcribe_ollama(provider, audio_path, bytes, language).await?;
                    Ok(TranscriptionResult { text, segments: vec![] })
                }
            }
        }
        other => {
            anyhow::bail!("La trascrizione audio funziona con OpenAI (Whisper), Gemini e server locali (Whisper/Ollama). Provider configurato: {other}")
        }
    }
}

fn create_transcription_client() -> Client {
    Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| Client::new())
}

pub fn parse_segments_from_json(json_val: &serde_json::Value) -> Vec<AudioSegment> {
    json_val["segments"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|seg| {
                    let start_ms = match seg["start_ms"].as_i64() {
                        Some(ms) => ms,
                        None => (seg["start"].as_f64()? * 1000.0).round() as i64,
                    };

                    let end_ms = match seg["end_ms"].as_i64() {
                        Some(ms) => ms,
                        None => (seg["end"].as_f64()? * 1000.0).round() as i64,
                    };

                    let seg_text = seg["text"].as_str()?.trim().to_string();
                    if seg_text.is_empty() {
                        return None;
                    }
                    let avg_logprob = seg["avg_logprob"].as_f64().unwrap_or(0.0) as f32;
                    Some(AudioSegment {
                        start_ms,
                        end_ms,
                        text: seg_text,
                        avg_logprob,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Calls OpenAI / faster-whisper audio transcription endpoint with `response_format=verbose_json`.
/// Automatically parses both text and timed segments. Supports fallback across candidate URLs.
pub async fn transcribe_openai_audio(
    provider: &ProviderConfig,
    audio_path: &Path,
    bytes: Vec<u8>,
    language: Option<&str>,
) -> Result<TranscriptionResult> {
    let api_key = provider
        .api_key
        .clone()
        .filter(|k| !k.trim().is_empty())
        .unwrap_or_else(|| "not-needed".to_string());
    let base_url = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

    let clean_base = base_url.trim().trim_end_matches('/');

    let candidate_urls = if clean_base.ends_with("/audio/transcriptions/segments") {
        vec![clean_base.to_string()]
    } else if clean_base.ends_with("/audio/transcriptions") {
        vec![
            clean_base.to_string(),
            format!("{clean_base}/segments"),
        ]
    } else if clean_base.ends_with("/v1") {
        vec![
            format!("{clean_base}/audio/transcriptions"),
            format!("{clean_base}/audio/transcriptions/segments"),
        ]
    } else {
        vec![
            format!("{clean_base}/v1/audio/transcriptions"),
            format!("{clean_base}/v1/audio/transcriptions/segments"),
            format!("{clean_base}/audio/transcriptions"),
            clean_base.to_string(),
        ]
    };

    let file_name = audio_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "recording.webm".to_string());

    let model_name = provider
        .model
        .clone()
        .unwrap_or_else(|| "whisper-1".to_string());

    let client = create_transcription_client();
    let bytes_len = bytes.len();
    let mut last_err = None;

    for url in &candidate_urls {
        let part = reqwest::multipart::Part::bytes(bytes.clone()).file_name(file_name.clone());
        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", model_name.clone())
            .text("response_format", "verbose_json");

        if let Some(lang) = language.filter(|l| !l.is_empty() && *l != "auto") {
            form = form.text("language", lang.to_string());
        }

        let payload_desc = format!(
            "File: {file_name} ({bytes_len} bytes), Modello: {model_name}, Lingua: {:?}, format: verbose_json",
            language
        );
        crate::logger::log_api_request("Whisper/OpenAI Audio", "POST", url, None, &payload_desc);

        let start_time = std::time::Instant::now();
        let send_res = client
            .post(url)
            .bearer_auth(&api_key)
            .multipart(form)
            .send()
            .await;
        let duration_ms = start_time.elapsed().as_millis();

        let response = match send_res {
            Ok(resp) => resp,
            Err(e) => {
                let err_msg = if e.is_timeout() {
                    format!("Timeout scaduto (oltre 5 minuti) in attesa di trascrizione da {url}.")
                } else if e.is_connect() {
                    format!("Impossibile connettersi al server Whisper su {url}: {e}")
                } else {
                    format!("Errore di rete verso {url}: {e}")
                };
                crate::logger::log_api_error("Whisper/OpenAI Audio", url, duration_ms, &err_msg);
                last_err = Some(anyhow::anyhow!("{err_msg}"));
                continue;
            }
        };

        let status = response.status();
        let body = response.text().await.unwrap_or_default();

        if status.as_u16() == 404 && candidate_urls.len() > 1 {
            crate::logger::log_api_error(
                "Whisper/OpenAI Audio (404 - tentando URL fallback)",
                url,
                duration_ms,
                &body,
            );
            last_err = Some(anyhow::anyhow!("HTTP 404 su {url}"));
            continue;
        }

        if !status.is_success() {
            crate::logger::log_api_error(
                "Whisper/OpenAI Audio",
                url,
                duration_ms,
                &format!("HTTP {status}: {body}"),
            );
            anyhow::bail!("Whisper API su {url} ha risposto con errore HTTP {status}: {body}");
        }

        crate::logger::log_api_response(
            "Whisper/OpenAI Audio",
            url,
            status.as_u16(),
            duration_ms,
            &body,
        );

        let json_val: serde_json::Value = serde_json::from_str(&body)
            .context("risposta JSON non valida da Whisper")?;

        let text = json_val["text"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_default();

        if text.trim().is_empty() {
            anyhow::bail!("Whisper API ha restituito una trascrizione vuota");
        }

        let segments = parse_segments_from_json(&json_val);

        return Ok(TranscriptionResult { text, segments });
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Nessun endpoint Whisper valido trovato")))
}

async fn transcribe_gemini(
    provider: &ProviderConfig,
    audio_path: &Path,
    bytes: Vec<u8>,
    language: Option<&str>,
) -> Result<String> {
    let api_key = provider
        .api_key
        .clone()
        .context("chiave API di Gemini non configurata")?;
    let base_url = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta".to_string());
    let model = provider
        .model
        .clone()
        .unwrap_or_else(|| "gemini-2.5-flash".to_string());

    let ext = audio_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("webm")
        .to_lowercase();
    let mime_type = match ext.as_str() {
        "wav" => "audio/wav",
        "mp3" => "audio/mp3",
        "ogg" => "audio/ogg",
        "m4a" => "audio/m4a",
        _ => "audio/webm",
    };

    let base64_audio = base64::engine::general_purpose::STANDARD.encode(&bytes);

    let prompt_text = match language {
        Some("it") => "Transcribe the spoken audio verbatim in Italian. Output only the transcript without conversational pleasantries or commentary.".to_string(),
        Some("en") => "Transcribe the spoken audio verbatim in English. Output only the transcript without conversational pleasantries or commentary.".to_string(),
        Some("es") => "Transcribe the spoken audio verbatim in Spanish. Output only the transcript without conversational pleasantries or commentary.".to_string(),
        Some("fr") => "Transcribe the spoken audio verbatim in French. Output only the transcript without conversational pleasantries or commentary.".to_string(),
        Some("de") => "Transcribe the spoken audio verbatim in German. Output only the transcript without conversational pleasantries or commentary.".to_string(),
        Some(other) if !other.is_empty() && other != "auto" => format!("Transcribe the spoken audio verbatim in {other}. Output only the transcript without conversational pleasantries or commentary."),
        _ => "Transcribe the spoken audio verbatim in its original spoken language (Italian by default if detected). Output only the transcript without conversational pleasantries or commentary.".to_string(),
    };

    let client = create_transcription_client();
    let response = client
        .post(format!("{base_url}/models/{model}:generateContent"))
        .header("x-goog-api-key", api_key)
        .json(&json!({
            "contents": [{
                "role": "user",
                "parts": [
                    {
                        "inline_data": {
                            "mime_type": mime_type,
                            "data": base64_audio
                        }
                    },
                    {
                        "text": prompt_text
                    }
                ]
            }]
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<serde_json::Value>()
        .await?;

    response["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .map(str::to_string)
        .context("risposta di trascrizione di Gemini senza contenuto")
}

async fn transcribe_ollama(
    provider: &ProviderConfig,
    audio_path: &Path,
    bytes: Vec<u8>,
    language: Option<&str>,
) -> Result<String> {
    let base_url = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "http://localhost:11434".to_string());
    let model = provider
        .model
        .clone()
        .unwrap_or_else(|| "whisper".to_string());

    let file_name = audio_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "recording.webm".to_string());

    let client = create_transcription_client();

    // 1. First attempt: OpenAI-compatible audio transcription endpoint (used by whisper.cpp server, faster-whisper server, vLLM, LocalAI)
    let part = reqwest::multipart::Part::bytes(bytes.clone()).file_name(file_name.clone());
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", model.clone());

    if let Some(lang) = language.filter(|l| !l.is_empty() && *l != "auto") {
        form = form.text("language", lang.to_string());
    }

    let mut req = client.post(format!("{base_url}/v1/audio/transcriptions")).multipart(form);
    if let Some(key) = &provider.api_key {
        if !key.trim().is_empty() {
            req = req.bearer_auth(key);
        }
    }

    if let Ok(res) = req.send().await {
        if res.status().is_success() {
            if let Ok(json_res) = res.json::<serde_json::Value>().await {
                if let Some(text) = json_res["text"].as_str() {
                    return Ok(text.to_string());
                }
            }
        }
    }

    // 2. Second attempt: Ollama chat/generate API with audio base64 if a multimodal model is used
    let chat_instruction = match language {
        Some("it") => "Transcribe the spoken audio in Italian. Output only the transcript verbatim.",
        Some(other) if !other.is_empty() && other != "auto" => &format!("Transcribe the spoken audio in {other}. Output only the transcript verbatim."),
        _ => "Transcribe the spoken audio in its original language. Output only the transcript verbatim.",
    };

    let base64_audio = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let res = client
        .post(format!("{base_url}/api/chat"))
        .json(&json!({
            "model": model,
            "messages": [
                {
                    "role": "user",
                    "content": chat_instruction,
                    "images": [base64_audio]
                }
            ],
            "stream": false
        }))
        .send()
        .await;

    if let Ok(response) = res {
        if response.status().is_success() {
            if let Ok(val) = response.json::<serde_json::Value>().await {
                if let Some(txt) = val["message"]["content"].as_str() {
                    return Ok(txt.to_string());
                }
            }
        }
    }

    anyhow::bail!(
        "Impossibile trascrivere l'audio con Ollama/Self-hosted. Assicurati che un server compatibile con Whisper (es. whisper.cpp, faster-whisper o LocalAI) o un modello audio sia in esecuzione su {base_url} con modello '{model}'."
    )
}
