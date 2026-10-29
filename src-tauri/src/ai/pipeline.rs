use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::ai::cost::{estimate_tokens, provider_price, CostEstimate};
use crate::ai::documentation::{
    build_workflow_steps, events_for_prompt, infer_workflow_summary, match_screenshots_to_steps,
    normalize_screenshot_references, render_documentation_markdown, sanitize_markdown,
};
use crate::ai::prompts::{self, DocLanguage, PROMPT_VERSION};
use crate::ai::providers::{build_provider, GenerateOptions, LlmProvider, LlmResponse, TokenUsage};
use crate::ai::transcription::{AudioSegment, transcribe_audio_with_segments};
use crate::compression::compress_events;
use crate::security::redact_events;
use crate::storage::Database;
use crate::storage::models::{
    AiJobStage, AiJobStatus, ProviderConfig, RedactionSummary, SessionEvent, SessionStatus, Screenshot,
    WorkflowStep,
};

/// Steps returned by the model are rejected above this count (the model is told the same limit).
const MAX_REFINED_STEPS: usize = 262144;

/// Result of a full documentation run.
pub struct PipelineOutput {
    pub markdown: String,
    pub redaction_summary: RedactionSummary,
    pub usage: TokenUsage,
    /// Requests whose token usage was not reported by the server (usage is then incomplete).
    pub unreported_calls: u32,
    pub cost_usd: Option<f64>,
}

/// Accumulates token usage across the LLM calls of one run.
#[derive(Default)]
struct UsageTracker {
    total: TokenUsage,
    unreported_calls: u32,
}

impl UsageTracker {
    fn record(&mut self, response: &LlmResponse) {
        match response.usage {
            Some(usage) => self.total += usage,
            None => self.unreported_calls += 1,
        }
    }
}

/// An event paired with nearby spoken audio segments (within a ±5s window).
#[derive(serde::Serialize)]
struct AlignedEvent<'a> {
    event: &'a SessionEvent,
    /// Text of audio segments that temporally overlap this event (3s before → 2s after).
    nearby_audio: Vec<&'a str>,
}

pub struct AiPipeline {
    db: Arc<Database>,
}

#[derive(Clone, serde::Serialize)]
pub struct AiProgressEvent {
    pub session_id: String,
    pub stage: String,
    pub status: String,
}

#[derive(Clone, serde::Serialize)]
pub struct AiLogEvent {
    pub session_id: String,
    pub message: String,
    pub timestamp_ms: i64,
}

impl AiPipeline {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub async fn run(
        &self,
        session_id: &str,
        app: &AppHandle,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<PipelineOutput> {
        self.check_cancelled(app, session_id, &cancel_flag)?;

        emit_log(app, session_id, "Inizializzazione generazione documentazione AI...");
        self.db.update_session_status(session_id, SessionStatus::Processing)?;
        let lang = DocLanguage::from_db(&self.db);
        let mut usage = UsageTracker::default();

        let provider = self
            .db
            .get_enabled_provider()?
            .context("nessun provider AI attivo: configuralo nelle Impostazioni")?;

        let session = self
            .db
            .get_session(session_id)?
            .context("sessione non trovata")?;

        let raw_events = load_session_events(&self.db, session_id)?;
        let compressed = compress_events(&raw_events);
        let (redacted_events, redaction_summary) = redact_events(&compressed);
        let screenshots = self.db.list_screenshots(session_id)?;

        emit_log(
            app,
            session_id,
            &format!(
                "Caricati {} eventi e {} screenshot; compressi in {} azioni chiave ({} elementi oscurati)",
                raw_events.len(),
                screenshots.len(),
                compressed.len(),
                redaction_summary.count
            ),
        );

        self.check_cancelled(app, session_id, &cancel_flag)?;

        let audio_transcript_result = {
            let stored_segments = session
                .audio_segments_json
                .as_deref()
                .and_then(|j| serde_json::from_str::<Vec<AudioSegment>>(j).ok())
                .unwrap_or_default();

            let has_valid_stored_segments = !stored_segments.is_empty();
            let has_existing_text = session
                .audio_transcript
                .as_ref()
                .map(|t| !t.trim().is_empty())
                .unwrap_or(false);

            if has_valid_stored_segments && has_existing_text {
                let existing_text = session.audio_transcript.as_ref().unwrap();
                emit_log(
                    app,
                    session_id,
                    &format!(
                        "Utilizzo trascrizione vocale e {} segmenti temporizzati esistenti a database",
                        stored_segments.len()
                    ),
                );
                Some(crate::ai::transcription::TranscriptionResult {
                    text: existing_text.clone(),
                    segments: stored_segments,
                })
            } else if let Some(audio_path_str) = &session.audio_path {
                let audio_path = std::path::Path::new(audio_path_str);
                if audio_path.is_file() {
                    if has_existing_text {
                        emit_log(
                            app,
                            session_id,
                            "Trascrizione presente ma priva di segmentazione temporizzata a DB. Avvio analisi audio con Whisper per recuperare i segmenti...",
                        );
                    } else {
                        emit_log(
                            app,
                            session_id,
                            "Avvio trascrizione audio del microfono con AI...",
                        );
                    }

                    let transcription_prov_id = self.db.get_setting("transcription_provider_id").unwrap_or(None);
                    let transcription_model_override = self.db.get_setting("transcription_model").unwrap_or(None);
                    let transcription_url_override = self.db.get_setting("transcription_base_url").unwrap_or(None);

                    let mut trans_provider = if let Some(prov_id) = transcription_prov_id.filter(|s| !s.trim().is_empty()) {
                        self.db
                            .list_providers()
                            .unwrap_or_default()
                            .into_iter()
                            .find(|p| p.id == prov_id)
                            .unwrap_or_else(|| provider.clone())
                    } else {
                        provider.clone()
                    };

                    if let Some(model_override) = transcription_model_override.filter(|s| !s.trim().is_empty()) {
                        trans_provider.model = Some(model_override);
                    }
                    if let Some(url_override) = transcription_url_override.filter(|s| !s.trim().is_empty()) {
                        trans_provider.base_url = Some(url_override);
                    }

                    let transcription_lang = self.db.get_setting("transcription_language").unwrap_or(None);

                    match transcribe_audio_with_segments(&trans_provider, audio_path, transcription_lang.as_deref()).await {
                        Ok(result) => {
                            let _ = self.db.update_session_audio_transcript(session_id, &result.text);
                            emit_log(app, session_id, &format!(
                                "Trascrizione vocale completata con {}: {} caratteri, {} segmenti temporizzati",
                                trans_provider.name, result.text.len(), result.segments.len()
                            ));
                            // Persist timed segments to DB for replay/re-alignment
                            if !result.segments.is_empty() {
                                if let Ok(segs_json) = serde_json::to_string(&result.segments) {
                                    let _ = self.db.update_session_audio_segments(session_id, &segs_json);
                                }
                            }
                            Some(result)
                        }
                        Err(err) => {
                            emit_log(app, session_id, &format!("Trascrizione non riuscita: {err}"));
                            if let Some(existing_text) = &session.audio_transcript {
                                emit_log(app, session_id, "Utilizzo comunque il testo della trascrizione precedentemente salvato");
                                Some(crate::ai::transcription::TranscriptionResult {
                                    text: existing_text.clone(),
                                    segments: vec![],
                                })
                            } else {
                                None
                            }
                        }
                    }
                } else if let Some(existing_text) = &session.audio_transcript {
                    emit_log(app, session_id, "Utilizzo trascrizione vocale esistente (file audio non presente per ri-segmentare)");
                    Some(crate::ai::transcription::TranscriptionResult {
                        text: existing_text.clone(),
                        segments: vec![],
                    })
                } else {
                    None
                }
            } else if let Some(existing_text) = &session.audio_transcript {
                emit_log(app, session_id, "Utilizzo trascrizione vocale esistente");
                Some(crate::ai::transcription::TranscriptionResult {
                    text: existing_text.clone(),
                    segments: vec![],
                })
            } else {
                None
            }
        };

        self.check_cancelled(app, session_id, &cancel_flag)?;

        let timeline_job = self
            .db
            .create_ai_job(session_id, AiJobStage::TimelineBuilder)?;
        emit_progress(app, session_id, AiJobStage::TimelineBuilder, "running");
        emit_log(app, session_id, "Costruzione sequenza temporale e raggruppamento azioni...");
        let mut steps = build_workflow_steps(&redacted_events);

        let session_start_ms = chrono::DateTime::parse_from_rfc3339(&session.started_at)
            .map(|dt| dt.timestamp_millis())
            .ok();

        // Build pre-aligned event+audio pairs in Rust (deterministic, no AI guesswork needed)
        let aligned_events = if let Some(ref result) = audio_transcript_result {
            if !result.segments.is_empty() {
                let pauses = self.db.get_session_pauses(session_id).unwrap_or_default();
                let aligned = align_audio_to_events(&redacted_events, &result.segments, session_start_ms, &pauses);
                emit_log(app, session_id, &format!(
                    "Allineamento deterministico audio-eventi: {}/{} eventi con audio associato",
                    aligned.iter().filter(|a| !a.nearby_audio.is_empty()).count(),
                    aligned.len()
                ));
                Some(aligned)
            } else {
                None
            }
        } else {
            None
        };

        let audio_text = audio_transcript_result.as_ref().map(|r| r.text.as_str()).filter(|t| !t.trim().is_empty());

        let timeout_secs = self
            .db
            .get_setting("ai_generation_timeout_seconds")
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(600);

        let refine_timeout = Duration::from_secs((timeout_secs / 2).max(60));
        let enhance_timeout = Duration::from_secs(timeout_secs.max(120));

        emit_log(app, session_id, "Ottimizzazione passaggi con modello LLM...");
        let audio_section = build_audio_context_section(audio_text, aligned_events.as_deref());
        let refine_fut = self.refine_timeline(
            &provider,
            lang,
            &session.title,
            &redacted_events,
            &steps,
            &audio_section,
            &mut usage,
        );

        match tokio::time::timeout(refine_timeout, refine_fut).await {
            Ok(Ok(ai_steps)) => {
                if !ai_steps.is_empty() && ai_steps.len() <= MAX_REFINED_STEPS {
                    steps = ai_steps;
                    emit_log(app, session_id, "Passaggi consolidati con successo dal modello AI");
                } else {
                    emit_log(app, session_id, "Passaggi deterministici mantenuti");
                }
            }
            Ok(Err(err)) => {
                emit_log(app, session_id, &format!("Ottimizzazione passaggi LLM saltata ({err}), utilizzo raggruppamento deterministico"));
            }
            Err(_) => {
                emit_log(app, session_id, "Timeout risposta modello AI per i passaggi, proseguo con i passaggi deterministici");
            }
        }

        self.check_cancelled(app, session_id, &cancel_flag)?;

        emit_progress(app, session_id, AiJobStage::TimelineBuilder, "completed");
        emit_log(app, session_id, &format!("Timeline definita: {} passaggi identificati", steps.len()));
        self.db.finish_ai_job(
            &timeline_job.id,
            AiJobStatus::Completed,
            Some(json!({ "steps": steps, "prompt_version": PROMPT_VERSION }).to_string()),
            None,
        )?;

        self.check_cancelled(app, session_id, &cancel_flag)?;

        let selector_job = self
            .db
            .create_ai_job(session_id, AiJobStage::ScreenshotSelector)?;
        emit_progress(app, session_id, AiJobStage::ScreenshotSelector, "running");
        emit_log(app, session_id, "Abbinamento e selezione screenshot migliori per ogni passaggio...");
        match_screenshots_to_steps(&mut steps, &screenshots);
        let selected_ids: Vec<String> = steps
            .iter()
            .flat_map(|step| step.screenshot_ids.clone())
            .collect();
        let _ = self.db.mark_screenshots_selected(&selected_ids);

        let highlight_enabled = crate::export::highlight_clicks_enabled(&self.db);
        if highlight_enabled {
            emit_log(app, session_id, "Evidenziazione punti di click sugli screenshot del documento...");
        }
        crate::export::prepare_screenshots_for_export(&screenshots, highlight_enabled);

        emit_progress(app, session_id, AiJobStage::ScreenshotSelector, "completed");
        emit_log(app, session_id, &format!("Selezionati {} screenshot per la documentazione", selected_ids.len()));
        self.db.finish_ai_job(
            &selector_job.id,
            AiJobStatus::Completed,
            Some(json!({ "count": screenshots.len() }).to_string()),
            None,
        )?;

        self.check_cancelled(app, session_id, &cancel_flag)?;

        let (doc_title, overview) =
            infer_workflow_summary(&session.title, &redacted_events, &steps, audio_text);
        let base_markdown = render_documentation_markdown(lang, &doc_title, &overview, &steps, &screenshots);

        let writer_job = self
            .db
            .create_ai_job(session_id, AiJobStage::TechnicalWriter)?;
        emit_progress(app, session_id, AiJobStage::TechnicalWriter, "running");
        emit_log(app, session_id, "Generazione guida e arricchimento tecnico con modello LLM...");

        let enhance_fut = self.enhance_documentation(
            &provider,
            lang,
            &doc_title,
            &overview,
            &base_markdown,
            &audio_section,
            &mut usage,
        );

        let markdown = match tokio::time::timeout(enhance_timeout, enhance_fut).await {
            Ok(Ok(enhanced)) => {
                emit_progress(app, session_id, AiJobStage::TechnicalWriter, "completed");
                emit_log(app, session_id, "Guida generata con successo dal modello AI");
                self.db.finish_ai_job(
                    &writer_job.id,
                    AiJobStatus::Completed,
                    Some(json!({ "length": enhanced.len(), "prompt_version": PROMPT_VERSION }).to_string()),
                    None,
                )?;
                enhanced
            }
            Ok(Err(err)) => {
                emit_progress(app, session_id, AiJobStage::TechnicalWriter, "failed");
                emit_log(app, session_id, &format!("Arricchimento AI non riuscito ({err}), uso template base"));
                self.db.finish_ai_job(
                    &writer_job.id,
                    AiJobStatus::Failed,
                    None,
                    Some(err.to_string()),
                )?;
                base_markdown.clone()
            }
            Err(_) => {
                emit_progress(app, session_id, AiJobStage::TechnicalWriter, "failed");
                emit_log(
                    app,
                    session_id,
                    &format!(
                        "Timeout ({}s) della risposta AI per la guida, uso template base. \
Per documentazioni lunghe aumenta il timeout di generazione nelle Impostazioni.",
                        enhance_timeout.as_secs()
                    ),
                );
                self.db.finish_ai_job(
                    &writer_job.id,
                    AiJobStatus::Failed,
                    None,
                    Some("Timeout response from LLM provider".to_string()),
                )?;
                base_markdown.clone()
            }
        };

        self.check_cancelled(app, session_id, &cancel_flag)?;

        let reviewer_job = self
            .db
            .create_ai_job(session_id, AiJobStage::QualityReviewer)?;
        emit_progress(app, session_id, AiJobStage::QualityReviewer, "running");
        emit_log(app, session_id, "Revisione qualità, sanitizzazione e controllo link screenshot...");
        let reviewed_markdown = finalize_markdown(&markdown, &base_markdown, &screenshots);
        emit_progress(app, session_id, AiJobStage::QualityReviewer, "completed");

        self.db.save_documentation(
            session_id,
            &reviewed_markdown,
            &serde_json::to_string(&steps)?,
            &serde_json::to_string(&redacted_events)?,
        )?;

        let cost_usd = provider_price(&self.db, &provider).map(|price| price.cost_usd(usage.total));
        let usage_line = format!(
            "Token utilizzati: {} in ingresso, {} in uscita{}{}",
            usage.total.input_tokens,
            usage.total.output_tokens,
            cost_usd.map(|c| format!(" (costo stimato ${c:.4})")).unwrap_or_default(),
            if usage.unreported_calls > 0 {
                format!(" [{} richieste senza conteggio dal server]", usage.unreported_calls)
            } else {
                String::new()
            }
        );
        emit_log(app, session_id, &usage_line);
        self.db.finish_ai_job(
            &reviewer_job.id,
            AiJobStatus::Completed,
            Some(
                json!({
                    "note": "Sanitized placeholders and normalized screenshot paths.",
                    "usage": usage.total,
                    "unreported_calls": usage.unreported_calls,
                    "cost_usd": cost_usd,
                    "prompt_version": PROMPT_VERSION,
                })
                .to_string(),
            ),
            None,
        )?;

        self.db.update_session_status(session_id, SessionStatus::Ready)?;
        emit_log(app, session_id, "Documentazione completata e salvata con successo!");

        Ok(PipelineOutput {
            markdown: reviewed_markdown,
            redaction_summary,
            usage: usage.total,
            unreported_calls: usage.unreported_calls,
            cost_usd,
        })
    }

    fn check_cancelled(&self, app: &AppHandle, session_id: &str, cancel_flag: &AtomicBool) -> Result<()> {
        if cancel_flag.load(Ordering::Relaxed) {
            emit_log(app, session_id, "Generazione annullata dall'utente.");
            let _ = self.db.update_session_status(session_id, SessionStatus::Ready);
            anyhow::bail!("generazione annullata dall'utente");
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn refine_timeline(
        &self,
        provider: &ProviderConfig,
        lang: DocLanguage,
        session_title: &str,
        events: &[SessionEvent],
        steps: &[WorkflowStep],
        audio_section: &str,
        usage: &mut UsageTracker,
    ) -> Result<Vec<WorkflowStep>> {
        if steps.len() <= 3 {
            return Ok(steps.to_vec());
        }

        let options = crate::ai::providers::get_ai_generate_options(
            &self.db,
            crate::ai::providers::AiOperation::DocumentGeneration,
        );
        let llm = build_provider(provider)?;
        let prompt = refine_prompt(lang, session_title, events, steps, audio_section)?;

        let response = llm.generate(&prompt.system, &prompt.user, &options).await?;
        usage.record(&response);
        parse_steps_json_with_retry(&*llm, &prompt.system, &response.text, &options, usage).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn enhance_documentation(
        &self,
        provider: &ProviderConfig,
        lang: DocLanguage,
        title: &str,
        overview: &str,
        base_markdown: &str,
        audio_section: &str,
        usage: &mut UsageTracker,
    ) -> Result<String> {
        let options = crate::ai::providers::get_ai_generate_options(
            &self.db,
            crate::ai::providers::AiOperation::DocumentGeneration,
        );
        let llm = build_provider(provider)?;
        let prompt = prompts::write_guide(lang, title, overview, base_markdown, audio_section);
        let response = llm.generate(&prompt.system, &prompt.user, &options).await?;
        usage.record(&response);
        Ok(response.text)
    }
}

fn refine_prompt(
    lang: DocLanguage,
    session_title: &str,
    events: &[SessionEvent],
    steps: &[WorkflowStep],
    audio_section: &str,
) -> Result<prompts::Prompt> {
    // Compact JSON: pretty-printing roughly doubles the token count of the event list.
    Ok(prompts::refine_timeline(
        lang,
        session_title,
        &serde_json::to_string(steps)?,
        &serde_json::to_string(&events_for_prompt(events))?,
        audio_section,
    ))
}

/// Estimates tokens and cost of `run` for a session without calling the model.
/// Uses the deterministic steps and any transcript already stored; the model's actual output
/// length (and hidden reasoning tokens) can differ, so this is an order of magnitude.
pub fn estimate_generation_cost(db: &Database, session_id: &str) -> Result<CostEstimate> {
    let provider = db
        .get_enabled_provider()?
        .context("nessun provider AI attivo: configuralo nelle Impostazioni")?;
    let session = db.get_session(session_id)?.context("sessione non trovata")?;
    let lang = DocLanguage::from_db(db);

    let raw_events = load_session_events(db, session_id)?;
    let (events, _) = redact_events(&compress_events(&raw_events));
    let screenshots = db.list_screenshots(session_id)?;
    let mut steps = build_workflow_steps(&events);

    let stored_segments: Vec<AudioSegment> = session
        .audio_segments_json
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    let transcript = session.audio_transcript.as_deref().filter(|t| !t.trim().is_empty());
    let needs_transcription = transcript.is_none()
        && session
            .audio_path
            .as_deref()
            .is_some_and(|p| std::path::Path::new(p).is_file());
    let session_start_ms = chrono::DateTime::parse_from_rfc3339(&session.started_at)
        .map(|dt| dt.timestamp_millis())
        .ok();
    let pauses = db.get_session_pauses(session_id).unwrap_or_default();
    let aligned = (!stored_segments.is_empty())
        .then(|| align_audio_to_events(&events, &stored_segments, session_start_ms, &pauses));
    let audio_section = build_audio_context_section(transcript, aligned.as_deref());

    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    let mut llm_calls = 0u32;

    if steps.len() > 3 {
        let prompt = refine_prompt(lang, &session.title, &events, &steps, &audio_section)?;
        input_tokens += estimate_tokens(&prompt.system) + estimate_tokens(&prompt.user);
        // The answer is a consolidated version of the steps JSON.
        output_tokens += estimate_tokens(&serde_json::to_string(&steps)?);
        llm_calls += 1;
    }

    match_screenshots_to_steps(&mut steps, &screenshots);
    let (title, overview) = infer_workflow_summary(&session.title, &events, &steps, transcript);
    let draft = render_documentation_markdown(lang, &title, &overview, &steps, &screenshots);
    let prompt = prompts::write_guide(lang, &title, &overview, &draft, &audio_section);
    input_tokens += estimate_tokens(&prompt.system) + estimate_tokens(&prompt.user);
    // The final guide expands every step with where/how/why: about twice the draft.
    output_tokens += estimate_tokens(&draft) * 2;
    llm_calls += 1;

    let price = provider_price(db, &provider);
    let estimated_cost_usd = price.map(|p| {
        p.cost_usd(TokenUsage {
            input_tokens,
            output_tokens,
        })
    });

    Ok(CostEstimate {
        provider_name: provider.name,
        model: provider.model,
        llm_calls,
        input_tokens,
        output_tokens,
        price,
        estimated_cost_usd,
        needs_transcription,
    })
}

fn load_session_events(db: &Database, session_id: &str) -> Result<Vec<SessionEvent>> {
    Ok(db
        .list_events(session_id)?
        .into_iter()
        .map(|event| SessionEvent {
            event_type: event.event_type,
            app_name: event.app_name,
            payload: serde_json::from_str(&event.payload).unwrap_or(json!({})),
            timestamp_ms: event.timestamp_ms,
        })
        .collect())
}

/// Placeholders are removed by `sanitize_markdown`; the base template is used only when the
/// model returned (almost) nothing. (Before, a word like "MISSING" anywhere in a guide that
/// contained an image link made the whole AI guide be thrown away.)
fn finalize_markdown(markdown: &str, fallback: &str, screenshots: &[Screenshot]) -> String {
    let cleaned = sanitize_markdown(markdown);
    let normalized = normalize_screenshot_references(&cleaned, screenshots);
    if normalized.trim().len() < 80 {
        normalize_screenshot_references(fallback, screenshots)
    } else {
        normalized
    }
}

fn parse_steps_json(response: &str) -> Result<Vec<WorkflowStep>> {
    serde_json::from_str(&extract_json_array(response)).context("JSON dei passi non valido")
}

/// Attempts to parse steps JSON from the AI response. If parsing fails, sends a correction
/// prompt asking the model to fix its own output before giving up.
async fn parse_steps_json_with_retry(
    llm: &dyn LlmProvider,
    system: &str,
    response: &str,
    options: &GenerateOptions,
    usage: &mut UsageTracker,
) -> Result<Vec<WorkflowStep>> {
    // First attempt
    match parse_steps_json(response) {
        Ok(steps) if !steps.is_empty() => return Ok(steps),
        _ => {}
    }

    // Second attempt: ask the model to correct its own output
    let correction_prompt = prompts::refine_timeline_correction(response);
    let corrected = llm.generate(system, &correction_prompt, options).await?;
    usage.record(&corrected);
    parse_steps_json(&corrected.text)
}

fn extract_json_array(response: &str) -> String {
    if let Some(start) = response.find('[') {
        if let Some(end) = response.rfind(']') {
            return response[start..=end].to_string();
        }
    }
    "[]".to_string()
}

/// Start of the audio timeline in the events' clock, or `None` when event timestamps are
/// already relative (legacy sessions). The microphone starts with the recording, so the session
/// start is used when it is close to the first event.
pub(crate) fn audio_timeline_origin(first_event_ts: i64, session_start_ms: Option<i64>) -> Option<i64> {
    if first_event_ts <= 1_000_000_000_000 {
        return None;
    }
    Some(
        session_start_ms
            .filter(|&start| start > 0 && (start - first_event_ts).abs() < 120_000)
            .map_or(first_event_ts, |start| start.min(first_event_ts)),
    )
}

/// Position of an event in the audio file. The microphone recorder is paused together with the
/// recording, so every pause that happened before the event is cut out of the audio timeline.
pub(crate) fn event_audio_offset_ms(timestamp_ms: i64, origin: Option<i64>, pauses: &[(i64, i64)]) -> i64 {
    let Some(t0) = origin else {
        return timestamp_ms;
    };
    let paused_before: i64 = pauses
        .iter()
        .map(|&(start, end)| (end.min(timestamp_ms) - start.max(t0)).max(0))
        .sum();
    (timestamp_ms - t0 - paused_before).max(0)
}

/// Deterministically aligns audio segments to events by timestamp proximity.
/// Converts absolute epoch event timestamps to relative audio file offsets using T0 and the
/// recorded pauses. For each event, finds all segments whose `[start_ms - PRE_MS, end_ms + POST_MS]`
/// window overlaps the event's timestamp. The model then receives already-paired data.
fn align_audio_to_events<'a>(
    events: &'a [SessionEvent],
    segments: &'a [AudioSegment],
    session_start_ms: Option<i64>,
    pauses: &[(i64, i64)],
) -> Vec<AlignedEvent<'a>> {
    // Window: 3s before event (user often explains before clicking) to 2s after
    const PRE_MS: i64 = 3000;
    const POST_MS: i64 = 2000;

    let first_event_ts = events.first().map(|e| e.timestamp_ms).unwrap_or(0);
    let origin = audio_timeline_origin(first_event_ts, session_start_ms);

    events
        .iter()
        .map(|event| {
            let event_offset_ms = event_audio_offset_ms(event.timestamp_ms, origin, pauses);

            let window_start = event_offset_ms.saturating_sub(PRE_MS);
            let window_end = event_offset_ms + POST_MS;

            let nearby_audio = segments
                .iter()
                .filter(|seg| {
                    // Segment overlaps [window_start, window_end]
                    seg.start_ms <= window_end && seg.end_ms >= window_start
                })
                .map(|seg| seg.text.as_str())
                .collect::<Vec<_>>();
            AlignedEvent { event, nearby_audio }
        })
        .collect()
}

/// Builds the audio context block for an AI prompt.
/// If pre-aligned events are available, formats them as structured JSON pairs (deterministic).
/// Otherwise falls back to a plain transcript blob.
fn build_audio_context_section(
    audio_transcript: Option<&str>,
    aligned_events: Option<&[AlignedEvent<'_>]>,
) -> String {
    if let Some(aligned) = aligned_events {
        // Only include events that have nearby audio
        let with_audio: Vec<_> = aligned
            .iter()
            .filter(|a| !a.nearby_audio.is_empty())
            .collect();

        if !with_audio.is_empty() {
            let pairs_json = with_audio
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "event_type": a.event.event_type,
                        "app": a.event.app_name,
                        "timestamp_ms": a.event.timestamp_ms,
                        "nearby_audio": a.nearby_audio,
                    })
                })
                .collect::<Vec<_>>();
            return format!(
                "\nPre-aligned Audio-Event pairs (audio already matched to the nearest event by timestamp):\n{}\n\n",
                serde_json::to_string_pretty(&pairs_json).unwrap_or_default()
            );
        }
    }

    // Fallback: plain blob
    if let Some(transcript) = audio_transcript.filter(|t| !t.trim().is_empty()) {
        format!(
            "\nUser's Spoken Microphone Explanation (Audio Transcript):\n\"\"\"\n{}\n\"\"\"\n",
            transcript.trim()
        )
    } else {
        String::new()
    }
}

fn emit_progress(app: &AppHandle, session_id: &str, stage: AiJobStage, status: &str) {
    let _ = app.emit(
        "ai-progress",
        AiProgressEvent {
            session_id: session_id.to_string(),
            stage: stage.as_str().to_string(),
            status: status.to_string(),
        },
    );
}

pub fn emit_log(app: &AppHandle, session_id: &str, message: &str) {
    crate::logger::info(&format!("AI:{session_id}"), message);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let _ = app.emit(
        "ai-log",
        AiLogEvent {
            session_id: session_id.to_string(),
            message: message.to_string(),
            timestamp_ms: now,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::{audio_timeline_origin, event_audio_offset_ms};

    #[test]
    fn pauses_are_cut_out_of_the_audio_timeline() {
        let t0 = 1_700_000_000_000;
        let origin = audio_timeline_origin(t0 + 500, Some(t0));
        assert_eq!(origin, Some(t0));
        // 10 s recorded, paused 20 s, event 5 s after resuming → 15 s into the audio.
        let pauses = [(t0 + 10_000, t0 + 30_000)];
        assert_eq!(event_audio_offset_ms(t0 + 35_000, origin, &pauses), 15_000);
        // Events before the pause are unaffected.
        assert_eq!(event_audio_offset_ms(t0 + 4_000, origin, &pauses), 4_000);
    }

    #[test]
    fn relative_timestamps_are_used_as_is() {
        assert_eq!(audio_timeline_origin(12_000, None), None);
        assert_eq!(event_audio_offset_ms(12_000, None, &[(1, 2)]), 12_000);
    }
}
