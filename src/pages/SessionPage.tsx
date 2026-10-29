import { convertFileSrc } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { save } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useParams, useNavigate } from "react-router-dom";
import { ProcessingPanel } from "@/components/ai/ProcessingPanel";
import { MarkdownEditor } from "@/components/documentation/MarkdownEditor";
import type { PreviewImageInfo } from "@/components/documentation/MarkdownPreview";
import { ImageAnnotationModal } from "@/components/sessions/ImageAnnotationModal";
import { ScreenshotPickerModal } from "@/components/sessions/ScreenshotPickerModal";
import { DuplicateScreenshotsModal } from "@/components/sessions/DuplicateScreenshotsModal";
import {
  clampSimilarity,
  DEFAULT_DUPLICATE_SIMILARITY,
  DUPLICATE_SIMILARITY_SETTING,
  SimilarityThreshold,
} from "@/components/sessions/SimilarityThreshold";
import { AudioTab, type AudioTranscriptionStatus } from "@/components/sessions/AudioTab";
import { TimelineTab } from "@/components/sessions/TimelineTab";
import { ExportPanel } from "@/components/export/ExportPanel";
import { ReplayPanel } from "@/components/replay/ReplayPanel";
import { AppButton } from "@/components/ui/AppButton";
import { ConfirmModal } from "@/components/ui/ConfirmModal";
import { Icon } from "@/components/ui/Icon";
import { Toast } from "@/components/ui/Toast";
import { useDebouncedEffect } from "@/hooks/useDebouncedEffect";
import { useSessionTitle } from "@/hooks/useSessionTitle";
import { useAiJob } from "@/context/AiJobContext";
import { describeCostEstimate, describeUsage } from "@/lib/cost";
import { useSessionsContext } from "@/context/SessionsContext";
import { useRecordingContext } from "@/context/RecordingContext";
import {
  api,
  type DuplicateScreenshotGroup,
  type ExportOptionsPayload,
  type ExportRecord,
  type RedactionSummary,
  type Session,
  type SessionEvent,
  type Screenshot,
  type WorkflowStep,
} from "@/lib/api";
import { replaceImageOnLine } from "@/lib/markdownSteps";
import { formatDuration } from "@/lib/utils";

/** How long after a recording ends the page keeps polling for its encoded videos. */
const VIDEO_POLL_WINDOW_MS = 5 * 60 * 1000;

const TABS = [
  "Timeline",
  "Screenshots",
  "Documentation",
  "Audio",
  "Replay",
  "Recording",
  "Exports",
] as const;

type TabName = (typeof TABS)[number];

export function SessionPage() {
  const { sessionId = "" } = useParams();
  const navigate = useNavigate();
  const { refreshSessions } = useSessionsContext();
  const { activeJob, startJob, cancelJob } = useAiJob();
  const { transcribingSessionIds } = useRecordingContext();
  // Transcription started automatically when the recording ended.
  const autoTranscribing = transcribingSessionIds.includes(sessionId);

  const [session, setSession] = useState<Session | null>(null);
  const [events, setEvents] = useState<SessionEvent[]>([]);
  const [screenshots, setScreenshots] = useState<Screenshot[]>([]);
  const [exports, setExports] = useState<ExportRecord[]>([]);
  const [steps, setSteps] = useState<WorkflowStep[]>([]);
  const [busy, setBusy] = useState(false);
  const [transcribing, setTranscribing] = useState(false);
  const [translating, setTranslating] = useState(false);
  const [translationLogs, setTranslationLogs] = useState<string[]>([]);
  const [showTranslationLog, setShowTranslationLog] = useState(false);
  const [translationStatus, setTranslationStatus] = useState<"idle" | "translating" | "success" | "error">("idle");

  const [audioLogs, setAudioLogs] = useState<string[]>([]);
  const [showAudioLog, setShowAudioLog] = useState(false);
  const [audioStatus, setAudioStatus] = useState<AudioTranscriptionStatus>("idle");
  const [annotatingScreenshot, setAnnotatingScreenshot] = useState<Screenshot | null>(null);
  const [exportingBundle, setExportingBundle] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [redactionSummary, setRedactionSummary] = useState<RedactionSummary | null>(null);
  const [tab, setTab] = useState<TabName>("Timeline");

  const [scanningDuplicates, setScanningDuplicates] = useState(false);
  const [duplicateGroups, setDuplicateGroups] = useState<DuplicateScreenshotGroup[] | null>(null);
  const [duplicateThreshold, setDuplicateThreshold] = useState(DEFAULT_DUPLICATE_SIMILARITY);
  // Bumped on every search so the dialog restarts from the new groups.
  const [duplicateScanId, setDuplicateScanId] = useState(0);

  useEffect(() => {
    api
      .getSetting(DUPLICATE_SIMILARITY_SETTING)
      .then((value) => {
        if (value) setDuplicateThreshold(clampSimilarity(Number(value)));
      })
      .catch(() => undefined);
  }, []);

  function changeDuplicateThreshold(value: number) {
    setDuplicateThreshold(value);
    api.setSetting(DUPLICATE_SIMILARITY_SETTING, String(value)).catch(() => undefined);
  }

  const [docImagePick, setDocImagePick] = useState<PreviewImageInfo | null>(null);
  const [docImageVersion, setDocImageVersion] = useState(0);
  const [confirmModal, setConfirmModal] = useState<{
    isOpen: boolean;
    title: string;
    message: string;
    confirmLabel?: string;
    cancelLabel?: string;
    kind?: "danger" | "warning" | "primary";
    action: () => Promise<void> | void;
  } | null>(null);

  const isCurrentSessionGenerating =
    activeJob?.sessionId === sessionId && !activeJob.isDone;

  const handleTitleError = useCallback((message: string) => {
    setError(message);
  }, []);

  const handleTitleSaved = useCallback((nextTitle: string) => {
    setSession((current) => (current ? { ...current, title: nextTitle } : current));
    // Exported files are renamed after the session: reload their new paths.
    api.listExports(sessionId).then(setExports).catch(() => undefined);
  }, [sessionId]);

  const { title, setTitle, saving } = useSessionTitle({
    sessionId,
    initialTitle: session?.id === sessionId ? session.title : "",
    onError: handleTitleError,
    onSaved: handleTitleSaved,
  });

  const [markdown, setMarkdown] = useState("");
  const [savingMarkdown, setSavingMarkdown] = useState(false);
  const lastSavedMarkdown = useRef<string>("");
  const isEditingMarkdown = useRef<boolean>(false);

  const refresh = useCallback(async (options?: { syncMarkdown?: boolean }) => {
    const [nextSession, nextEvents, nextScreenshots, nextExports, nextSteps] =
      await Promise.all([
        api.getSession(sessionId),
        api.getCompressedTimeline(sessionId),
        api.listScreenshots(sessionId),
        api.listExports(sessionId),
        api.getReplaySteps(sessionId),
      ]);
    setSession(nextSession);
    setEvents(nextEvents);
    setScreenshots(nextScreenshots);
    setExports(nextExports);
    setSteps(nextSteps);

    const dbMd = nextSession?.documentation_md ?? "";
    if (options?.syncMarkdown || (!isEditingMarkdown.current && lastSavedMarkdown.current === "")) {
      lastSavedMarkdown.current = dbMd;
      setMarkdown(dbMd);
    }
  }, [sessionId]);

  // Latest text in the editor, to tell whether more edits arrived while a save was running.
  const latestMarkdown = useRef("");
  latestMarkdown.current = markdown;

  const handleSaveDocumentation = useCallback(async (textToSave?: string) => {
    const md = textToSave !== undefined ? textToSave : markdown;
    if (!sessionId) return;
    setSavingMarkdown(true);
    try {
      await api.updateDocumentation(sessionId, md);
      lastSavedMarkdown.current = md;
      if (latestMarkdown.current === md) {
        isEditingMarkdown.current = false;
      }
      // Saving also re-derives the steps from the guide: reload them (and the session) so the
      // Replay and Export tabs work on what was just saved, with the new step numbers.
      const [nextSession, nextSteps] = await Promise.all([
        api.getSession(sessionId),
        api.getReplaySteps(sessionId),
      ]);
      if (nextSession) setSession(nextSession);
      setSteps(nextSteps);
      setToast("Documentazione salvata con successo");
    } catch (err) {
      setError(String(err));
    } finally {
      setSavingMarkdown(false);
    }
  }, [sessionId, markdown]);

  const handleMarkdownChange = useCallback((newMd: string) => {
    isEditingMarkdown.current = true;
    setMarkdown(newMd);
  }, []);

  useDebouncedEffect(() => {
    // An emptied guide is a change too: save it.
    if (isEditingMarkdown.current && markdown !== lastSavedMarkdown.current) {
      handleSaveDocumentation(markdown);
    }
  }, [markdown, handleSaveDocumentation], 800);

  useEffect(() => {
    refresh({ syncMarkdown: true }).catch((err) => setError(String(err)));
  }, [sessionId, refresh]);

  // When activeJob completes for this session, refresh data
  useEffect(() => {
    if (activeJob?.sessionId === sessionId && activeJob.isDone && activeJob.result) {
      setMarkdown(activeJob.result.markdown);
      setTab("Documentation");
      setRedactionSummary(activeJob.result.redaction_summary);
      setToast(`Documentazione generata. ${describeUsage(activeJob.result)}`);
      refresh().catch(() => undefined);
    }
  }, [activeJob?.sessionId, activeJob?.isDone, activeJob?.result, sessionId]);

  // Reload the transcript once the automatic transcription is over.
  const wasAutoTranscribing = useRef(autoTranscribing);
  useEffect(() => {
    if (wasAutoTranscribing.current && !autoTranscribing) {
      refresh({ syncMarkdown: false }).catch(() => undefined);
    }
    wasAutoTranscribing.current = autoTranscribing;
  }, [autoTranscribing, refresh]);

  const [selectedVideoMode, setSelectedVideoMode] = useState<"full" | "timelapse">("full");

  const fullVideoSrc = useMemo(() => {
    if (!session?.full_video_path?.endsWith(".mp4")) {
      return null;
    }
    return convertFileSrc(session.full_video_path);
  }, [session?.full_video_path]);

  const timelapseVideoSrc = useMemo(() => {
    if (!session?.video_path?.endsWith(".mp4")) {
      return null;
    }
    return convertFileSrc(session.video_path);
  }, [session?.video_path]);

  const activeVideoSrc = selectedVideoMode === "full"
    ? (fullVideoSrc ?? timelapseVideoSrc)
    : (timelapseVideoSrc ?? fullVideoSrc);

  const videoEncoding = Boolean(
    session &&
      session.duration > 0 &&
      !session.full_video_path?.endsWith(".mp4") &&
      !session.video_path?.endsWith(".mp4"),
  );

  useEffect(() => {
    if (!sessionId) return;
    // One video is enough: with the HD recording on, the low-fps slideshow is not produced.
    if (session?.video_path?.endsWith(".mp4") || session?.full_video_path?.endsWith(".mp4")) {
      return;
    }

    let disposed = false;
    const unlisteners: (() => void)[] = [];
    const keep = (unlisten: () => void) => {
      if (disposed) unlisten();
      else unlisteners.push(unlisten);
    };

    void api
      .onVideoReady(sessionId, () => {
        refresh({ syncMarkdown: false }).catch((err) => setError(String(err)));
      })
      .then(keep);

    void api
      .onFullVideoReady(sessionId, () => {
        refresh({ syncMarkdown: false }).catch((err) => setError(String(err)));
      })
      .then(keep);

    // Polling only backs up the "ready" events while the videos are being encoded right after a
    // recording. A video that never comes (full video disabled, no ffmpeg) must not keep the
    // page polling forever.
    const endedAt = session?.ended_at ? Date.parse(session.ended_at) : Number.NaN;
    const pollUntil = Number.isFinite(endedAt)
      ? endedAt + VIDEO_POLL_WINDOW_MS
      : Date.now() + VIDEO_POLL_WINDOW_MS;

    const interval = window.setInterval(async () => {
      if (Date.now() > pollUntil) {
        window.clearInterval(interval);
        return;
      }
      if (session && session.duration > 0) {
        try {
          const updated = await api.getSession(sessionId);
          if (updated && (updated.video_path !== session.video_path || updated.full_video_path !== session.full_video_path)) {
            setSession((prev) =>
              prev
                ? { ...prev, video_path: updated.video_path, full_video_path: updated.full_video_path }
                : updated,
            );
          }
        } catch {
          // ignore
        }
      }
    }, 3000);

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
      window.clearInterval(interval);
    };
  }, [sessionId, session?.video_path, session?.full_video_path, session?.ended_at]);

  /**
   * Checks that generating makes sense before asking for confirmation: the generation is never
   * started automatically and costs money, so a non expert user is stopped (or warned) when the
   * result would be empty or incomplete.
   */
  async function handleGenerate() {
    if (!session) return;
    setError(null);

    if (activeJob && !activeJob.isDone && activeJob.sessionId !== sessionId) {
      setConfirmModal({
        isOpen: true,
        title: "C'è già una generazione in corso",
        message: `Si può generare una documentazione alla volta. È in corso quella di "${activeJob.sessionTitle}": attendi che finisca.`,
        confirmLabel: "Vai alla sessione",
        cancelLabel: "Chiudi",
        kind: "primary",
        action: () => navigate(`/sessions/${activeJob.sessionId}`),
      });
      return;
    }
    if (session.status === "recording") {
      setError("La sessione è ancora in registrazione: fermala prima di generare la documentazione.");
      return;
    }
    if (screenshots.length === 0 && events.length === 0) {
      setError(
        "Questa sessione non contiene nessun passaggio registrato (né click né screenshot): non c'è niente da documentare. Registra di nuovo il flusso di lavoro.",
      );
      return;
    }

    const providers = await api.listProviders().catch(() => null);
    if (providers && !providers.some((provider) => provider.enabled === 1)) {
      setConfirmModal({
        isOpen: true,
        title: "Nessun servizio AI configurato",
        message:
          "Per generare la documentazione serve un provider AI (es. OpenAI, Claude, Gemini) con la sua chiave API.\n\nConfiguralo nelle Impostazioni e poi torna qui.",
        confirmLabel: "Apri Impostazioni",
        cancelLabel: "Annulla",
        kind: "primary",
        action: () => navigate("/settings"),
      });
      return;
    }

    if (transcribing || autoTranscribing) {
      setConfirmModal({
        isOpen: true,
        title: "Trascrizione audio in corso",
        message:
          "L'audio di questa sessione è ancora in fase di trascrizione. Se generi adesso, la documentazione non userà le spiegazioni che hai dato a voce.\n\nConviene attendere la fine della trascrizione (pochi minuti).",
        confirmLabel: "Genera comunque",
        cancelLabel: "Aspetta",
        kind: "warning",
        action: () => confirmGenerate(),
      });
      return;
    }

    const audioNotTranscribed = Boolean(session.audio_path) && !session.audio_transcript?.trim();
    await confirmGenerate(
      audioNotTranscribed
        ? "⚠️ L'audio registrato non è ancora stato trascritto: la documentazione non userà le tue spiegazioni a voce. Per usarle annulla, apri la scheda \"Audio\" e avvia la trascrizione."
        : undefined,
    );
  }

  async function confirmGenerate(note?: string) {
    let estimateText: string;
    try {
      estimateText = describeCostEstimate(await api.estimateGenerationCost(sessionId));
    } catch (err) {
      // No provider configured or session not readable: the generation itself reports the real error.
      estimateText = `Stima dei costi non disponibile (${String(err)}).`;
    }

    if (note) {
      estimateText = `${note}\n\n${estimateText}`;
    }

    const hasDocumentation = Boolean(session?.documentation_md && session.documentation_md.trim().length > 0);
    setConfirmModal({
      isOpen: true,
      title: hasDocumentation ? "Sovrascrivere la Documentazione Esistente?" : "Generare la documentazione?",
      message: hasDocumentation
        ? `Per questa sessione esiste già una documentazione generata: la documentazione attuale e i passaggi salvati verranno sovrascritti.\n\n${estimateText}`
        : estimateText,
      confirmLabel: hasDocumentation ? "Rigenera e Sovrascrivi" : "Genera",
      cancelLabel: "Annulla",
      kind: hasDocumentation ? "warning" : "primary",
      action: () => executeGenerate(),
    });
  }

  async function executeGenerate() {
    setBusy(true);
    setError(null);
    setToast(null);
    try {
      await startJob(sessionId, title || session?.title || "Session");
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  function handleDeleteSession() {
    if (!session) return;
    setConfirmModal({
      isOpen: true,
      title: "Elimina Sessione",
      message: `Sei sicuro di voler eliminare definitivamente la sessione "${session.title}"?\n\nTutti gli eventi registrati, gli screenshot e i file associati verranno cancellati in modo permanente. Questa operazione non può essere annullata.`,
      confirmLabel: "Elimina Sessione",
      cancelLabel: "Annulla",
      kind: "danger",
      action: async () => {
        setBusy(true);
        try {
          await api.deleteSession(sessionId);
          await refreshSessions();
          navigate("/");
        } catch (err) {
          setError(String(err));
          setBusy(false);
        }
      },
    });
  }

  async function handleCopyMarkdown() {
    setError(null);
    try {
      await navigator.clipboard.writeText(markdown);
      setToast("Markdown copied");
    } catch (err) {
      setError(String(err));
    }
  }

  function handleTranscribeAudio() {
    if (session?.audio_transcript && session.audio_transcript.trim().length > 0) {
      setConfirmModal({
        isOpen: true,
        title: "Sovrascrivere la Trascrizione Audio?",
        message: "Questa sessione possiede già una trascrizione audio salvata.\n\nAvviando una nuova trascrizione con Whisper AI, il testo e i segmenti temporali attuali verranno sostituiti.\n\nDesideri procedere?",
        confirmLabel: "Ritrascrivi Audio",
        cancelLabel: "Annulla",
        kind: "warning",
        action: () => executeTranscribeAudio(),
      });
    } else {
      executeTranscribeAudio();
    }
  }

  async function executeTranscribeAudio() {
    setTranscribing(true);
    setError(null);
    setToast(null);
    setShowAudioLog(true);
    setAudioStatus("transcribing");
    setAudioLogs([`[${new Date().toLocaleTimeString()}] Avvio richiesta di trascrizione vocale...`]);

    let unlisten: (() => void) | undefined;
    try {
      unlisten = await api.onAiLog(sessionId, (evt) => {
        setAudioLogs((prev) => [...prev, `[${new Date().toLocaleTimeString()}] ${evt.message}`]);
      });

      await api.transcribeSessionAudio(sessionId);
      setAudioStatus("success");
      setToast("Audio trascritto con successo!");
      await refresh({ syncMarkdown: false });
    } catch (err) {
      const msg = String(err);
      setError(msg);
      setAudioStatus("error");
      setAudioLogs((prev) => [...prev, `[${new Date().toLocaleTimeString()}] ❌ ERRORE: ${msg}`]);
    } finally {
      if (unlisten) unlisten();
      setTranscribing(false);
    }
  }

  function handleTranslateDocumentation(targetLanguage = "Italian") {
    if (markdown && markdown.trim().length > 0) {
      setConfirmModal({
        isOpen: true,
        title: "Tradurre la Documentazione in Italiano?",
        message: "Il testo attuale della guida verrà inviato al modello AI per essere tradotto in lingua italiana.\n\nI tag delle immagini e gli screenshot associati rimarranno intatti.\n\nDesideri avviare la traduzione?",
        confirmLabel: "Avvia Traduzione",
        cancelLabel: "Annulla",
        kind: "primary",
        action: () => executeTranslateDocumentation(targetLanguage),
      });
    } else {
      executeTranslateDocumentation(targetLanguage);
    }
  }

  async function executeTranslateDocumentation(targetLanguage = "Italian") {
    setTranslating(true);
    setError(null);
    setToast(null);
    setShowTranslationLog(true);
    setTranslationStatus("translating");
    setTranslationLogs([`[${new Date().toLocaleTimeString()}] Avvio richiesta di traduzione in ${targetLanguage}...`]);

    let unlisten: (() => void) | undefined;
    try {
      unlisten = await api.onAiLog(sessionId, (evt) => {
        setTranslationLogs((prev) => [...prev, `[${new Date().toLocaleTimeString()}] ${evt.message}`]);
      });

      const translated = await api.translateDocumentation(sessionId, targetLanguage);
      setMarkdown(translated);
      lastSavedMarkdown.current = translated;
      setTranslationStatus("success");
      setToast("Documentazione tradotta in italiano con successo!");
      await refresh({ syncMarkdown: false });
    } catch (err) {
      const msg = String(err);
      setError(msg);
      setTranslationStatus("error");
      setTranslationLogs((prev) => [...prev, `[${new Date().toLocaleTimeString()}] ❌ ERRORE: ${msg}`]);
    } finally {
      if (unlisten) unlisten();
      setTranslating(false);
    }
  }

  async function handleExport(format: string, options?: ExportOptionsPayload) {
    setBusy(true);
    setError(null);
    try {
      const record = await api.exportSession(sessionId, format, options);
      setToast(`Exported ${record.format.toUpperCase()}`);
      await refresh();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  async function handleRevealExport(path: string) {
    setError(null);
    try {
      await revealItemInDir(path);
      setToast("Revealed in folder");
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleExportBundle() {
    if (!session) return;
    try {
      const sanitizedTitle = (session.title || "session").replace(/[/\\?%*:|"<>]/g, "_");
      const defaultName = `${sanitizedTitle}.flowcapture`;
      const targetPath = await save({
        defaultPath: defaultName,
        title: "Salva archivio completo sessione FlowCapture",
        filters: [
          {
            name: "FlowCapture Session (*.flowcapture)",
            extensions: ["flowcapture", "zip"],
          },
        ],
      });

      if (!targetPath) return;

      setExportingBundle(true);
      setError(null);
      await api.exportSessionBundle(session.id, targetPath);
      setToast(`Sessione esportata con successo in: ${targetPath}`);
    } catch (err) {
      setError(`Errore durante l'esportazione: ${err}`);
    } finally {
      setExportingBundle(false);
    }
  }

  if (!session) {
    return (
      <div className="page">
        <p style={{ color: "var(--muted)" }}>Loading session…</p>
      </div>
    );
  }

  if (isCurrentSessionGenerating && activeJob) {
    return (
      <div className="page">
        <ProcessingPanel
          activeStage={activeJob.stage}
          completedStages={activeJob.completedStages}
          failedStage={activeJob.failedStage}
          logs={activeJob.logs}
          onCancel={() => cancelJob(sessionId)}
        />
      </div>
    );
  }


  /** Leaving the guide with unsaved edits saves them right away (no 800 ms wait), so the
   * other tabs show the current guide and its renumbered steps. */
  function switchTab(next: TabName) {
    if (
      tab === "Documentation" &&
      next !== tab &&
      isEditingMarkdown.current &&
      markdown !== lastSavedMarkdown.current
    ) {
      void handleSaveDocumentation(markdown);
    }
    setTab(next);
  }

  async function handleFindDuplicates(threshold = duplicateThreshold) {
    setScanningDuplicates(true);
    try {
      const groups = await api.findDuplicateScreenshots(sessionId, threshold);
      if (groups.length === 0) {
        setDuplicateGroups(null);
        setToast(`Nessuno screenshot duplicato trovato (somiglianza ≥ ${threshold}%)`);
      } else {
        setDuplicateScanId((id) => id + 1);
        setDuplicateGroups(groups);
      }
    } catch (err) {
      setError(String(err));
    } finally {
      setScanningDuplicates(false);
    }
  }

  async function handleMergeDuplicates(keepId: string, removeIds: string[]) {
    await api.mergeDuplicateScreenshots(sessionId, keepId, removeIds);
    setToast(`Uniti ed eliminati ${removeIds.length} screenshot duplicati`);
    await refresh({ syncMarkdown: !isEditingMarkdown.current });
  }

  async function handleDocImageSelected(screenshotId: string) {
    const target = docImagePick;
    const shot = screenshots.find((item) => item.id === screenshotId);
    if (!target?.line || !shot) return;
    const filename = shot.path.split(/[/\\]/).pop() ?? shot.path;
    const next = replaceImageOnLine(markdown, target.line, `screenshots/${filename}`);
    if (next === markdown) return;
    setMarkdown(next);
    await handleSaveDocumentation(next);
    await refresh({ syncMarkdown: false });
    setToast("Immagine sostituita nella documentazione");
  }

  return (
    <div className="page">
      <div className="sd-titlebar">
        <input
          value={title}
          onChange={(event) => setTitle(event.target.value)}
          aria-label="Session title"
        />
        {saving ? <span className="sd-title-status">Saving…</span> : null}
      </div>
      <div className="sd-meta">
        {session.status} · {formatDuration(session.duration)} · {events.length} timeline
        events
      </div>

      <div className="sd-actions" style={{ display: "flex", gap: "10px", alignItems: "center" }}>
        <AppButton kind="primary" icon="sparkles" disabled={busy} onClick={handleGenerate}>
          Generate Documentation
        </AppButton>
        <AppButton icon="download" disabled={busy} onClick={() => switchTab("Exports")}>
          Export
        </AppButton>
        <AppButton
          icon="folder"
          disabled={busy || exportingBundle}
          onClick={handleExportBundle}
          title="Esporta l'intera sessione (eventi, screenshot, audio, video e documenti) in un file compresso trasferibile"
        >
          {exportingBundle ? "Esportazione…" : "Esporta Sessione (.flowcapture)"}
        </AppButton>
        <AppButton
          kind="ghost"
          icon="trash"
          disabled={busy}
          onClick={handleDeleteSession}
          style={{ marginLeft: "auto", color: "#ef4444", borderColor: "rgba(239, 68, 68, 0.3)" }}
        >
          Elimina Sessione
        </AppButton>
      </div>

      {error ? (
        <div className="card set-card error-card">
          <div className="error-card-body">
            <Icon name="alert" size={18} />
            <span>{error}</span>
          </div>
          <button type="button" className="error-card-close" onClick={() => setError(null)} title="Chiudi messaggio di errore">
            ✕
          </button>
        </div>
      ) : null}

      {autoTranscribing ? (
        <div className="warn-card info">
          <span className="pulse" style={{ width: 8, height: 8, marginTop: 5 }} />
          <div className="warn-card-body">
            <strong>Trascrizione dell'audio in corso…</strong> Attendi che finisca prima di generare la
            documentazione, così verranno usate anche le tue spiegazioni a voce.
          </div>
        </div>
      ) : null}

      {redactionSummary && redactionSummary.count > 0 ? (
        <div className="banner" style={{ marginTop: 18 }}>
          <span className="bt">
            Applied {redactionSummary.count} redaction pattern(s) before AI processing:{" "}
            {redactionSummary.patterns.join(", ")}
          </span>
        </div>
      ) : null}

      <div className="tabs">
        {TABS.map((name) => (
          <button
            key={name}
            type="button"
            className={`tab${tab === name ? " active" : ""}`}
            onClick={() => switchTab(name)}
          >
            {name}
          </button>
        ))}
      </div>

      {tab === "Timeline" ? (
        <TimelineTab
          events={events}
          screenshots={screenshots}
          onOpenScreenshot={setAnnotatingScreenshot}
        />
      ) : null}

      {tab === "Screenshots" ? (
        <div className="card panel">
          <div
            style={{
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
              marginBottom: "14px",
              flexWrap: "wrap",
              gap: "10px",
            }}
          >
            <div>
              <h3 style={{ margin: 0 }}>Captured Screenshots ({screenshots.length})</h3>
              <div className="pd">Catture basate sugli eventi registrati durante il workflow.</div>
            </div>
            <div className="duplicate-tools">
              <SimilarityThreshold
                value={duplicateThreshold}
                onChange={changeDuplicateThreshold}
                disabled={scanningDuplicates}
              />
              <AppButton
                size="sm"
                disabled={scanningDuplicates || screenshots.length < 2 || busy}
                onClick={() => handleFindDuplicates()}
                title={`Trova immagini con somiglianza ≥ ${duplicateThreshold}% e ti permette di scegliere quali unire o eliminare`}
              >
                {scanningDuplicates ? "Scansione duplicati…" : `🔍 Elimina duplicati (≥ ${duplicateThreshold}%)`}
              </AppButton>
            </div>
          </div>
          <div className="shot-grid">
            {screenshots.length === 0 ? (
              <div className="pd">No screenshots captured yet.</div>
            ) : (
              screenshots.map((shot) => (
                <div className="shot" key={shot.id}>
                  <div className="simg">
                    <img
                      src={convertFileSrc(shot.path)}
                      alt={shot.trigger ?? "Screenshot"}
                      style={{
                        position: "absolute",
                        inset: 0,
                        width: "100%",
                        height: "100%",
                        objectFit: "contain",
                        objectPosition: "center",
                        backgroundColor: "rgba(0,0,0,0.4)",
                      }}
                      onError={(event) => {
                        (event.target as HTMLImageElement).style.display = "none";
                      }}
                    />
                  </div>
                  <div className="scap" style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                    <span className="sn">{shot.trigger ?? "capture"}</span>
                    <div style={{ display: "flex", alignItems: "center", gap: "6px" }}>
                      <button
                        type="button"
                        className="btn btn-ghost btn-sm"
                        style={{ padding: "2px 6px", fontSize: "11px", display: "flex", alignItems: "center", gap: "4px" }}
                        onClick={() => setAnnotatingScreenshot(shot)}
                        title="Modifica screenshot, sposta click o aggiungi annotazioni"
                      >
                        🎨 Modifica
                      </button>
                      <button
                        type="button"
                        className="del"
                        onClick={() =>
                          api
                            .deleteScreenshot(shot.id)
                            // The backend also drops the image from the guide: reload it unless
                            // the user is in the middle of editing it.
                            .then(() => refresh({ syncMarkdown: !isEditingMarkdown.current }))
                            .catch((err) => setError(String(err)))
                        }
                      >
                        <Icon name="trash" size={16} />
                      </button>
                    </div>
                  </div>
                </div>
              ))
            )}
          </div>
        </div>
      ) : null}

      {tab === "Documentation" ? (
        <div className="card panel">
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "8px", flexWrap: "wrap", gap: "10px" }}>
            <div>
              <h3>Documentazione Generata</h3>
              <div className="pd">
                Modifica il Markdown con la barra degli strumenti avanzata, inserisci screenshot o passaggi e visualizza l'anteprima live.
              </div>
            </div>
            <div style={{ display: "flex", gap: "8px", alignItems: "center", flexWrap: "wrap" }}>
              <AppButton
                size="sm"
                kind="ghost"
                icon="sparkles"
                disabled={!markdown || translating || busy}
                onClick={() => handleTranslateDocumentation("Italian")}
                title="Traduci il solo testo della guida in Italiano (le immagini vengono isolate e ripristinate intatte)"
              >
                {translating ? "Traduzione in corso…" : "🌐 Traduci in Italiano (AI)"}
              </AppButton>
              <AppButton
                size="sm"
                icon="copy"
                disabled={!markdown}
                onClick={handleCopyMarkdown}
              >
                Copia Markdown
              </AppButton>
              <AppButton
                size="sm"
                kind="primary"
                icon="save"
                disabled={busy || translating || savingMarkdown}
                onClick={() => handleSaveDocumentation(markdown)}
              >
                {savingMarkdown ? "Salvataggio…" : "Salva Modifiche"}
              </AppButton>
            </div>
          </div>

          {showTranslationLog && (
            <div
              style={{
                marginTop: "12px",
                marginBottom: "16px",
                background: "#0d1117",
                border: `1px solid ${
                  translationStatus === "error"
                    ? "rgba(239, 68, 68, 0.45)"
                    : translationStatus === "success"
                    ? "rgba(52, 211, 153, 0.45)"
                    : "rgba(108, 198, 255, 0.4)"
                }`,
                borderRadius: "10px",
                padding: "14px 16px",
                boxShadow: "0 6px 20px rgba(0, 0, 0, 0.35)",
              }}
            >
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                  marginBottom: "8px",
                }}
              >
                <div
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: "8px",
                    fontWeight: 600,
                    fontSize: "13px",
                    color:
                      translationStatus === "error"
                        ? "#f87171"
                        : translationStatus === "success"
                        ? "#34d399"
                        : "var(--ice)",
                  }}
                >
                  {translationStatus === "translating" && (
                    <span
                      style={{
                        display: "inline-block",
                        width: "12px",
                        height: "12px",
                        border: "2px solid var(--ice)",
                        borderTopColor: "transparent",
                        borderRadius: "50%",
                        animation: "spin 0.8s linear infinite",
                      }}
                    />
                  )}
                  {translationStatus === "success" && <Icon name="check" size={15} />}
                  {translationStatus === "error" && <Icon name="alert" size={15} />}
                  <span>
                    {translationStatus === "translating"
                      ? "Traduzione AI in corso (invio solo testo, immagini protette)..."
                      : translationStatus === "success"
                      ? "Traduzione completata con successo!"
                      : "Errore durante la traduzione con il server AI"}
                  </span>
                </div>
                <button
                  type="button"
                  onClick={() => setShowTranslationLog(false)}
                  style={{
                    background: "rgba(255, 255, 255, 0.08)",
                    border: "none",
                    borderRadius: "6px",
                    color: "#8b949e",
                    cursor: "pointer",
                    padding: "3px 8px",
                    fontSize: "11px",
                  }}
                >
                  Chiudi log ✕
                </button>
              </div>

              <div
                style={{
                  fontFamily: "var(--mono, monospace)",
                  fontSize: "12px",
                  lineHeight: "1.5",
                  color: "#c9d1d9",
                  maxHeight: "160px",
                  overflowY: "auto",
                  background: "rgba(0, 0, 0, 0.4)",
                  borderRadius: "6px",
                  padding: "10px 12px",
                  display: "flex",
                  flexDirection: "column",
                  gap: "4px",
                }}
              >
                {translationLogs.map((log, idx) => (
                  <div
                    key={idx}
                    style={{
                      color: log.includes("ERRORE")
                        ? "#f87171"
                        : log.includes("successo")
                        ? "#34d399"
                        : log.includes("Isolate")
                        ? "#fbbf24"
                        : "#e6edf3",
                    }}
                  >
                    {log}
                  </div>
                ))}
              </div>
            </div>
          )}

          <div style={{ marginTop: "14px" }}>
            <MarkdownEditor
              value={markdown}
              onChange={handleMarkdownChange}
              screenshots={screenshots}
              disabled={busy || translating}
              showSaveButton={true}
              saving={savingMarkdown}
              onSave={() => handleSaveDocumentation(markdown)}
              imageVersion={docImageVersion}
              imageActions={{
                onChange: (info) => setDocImagePick(info),
                onAnnotate: (info) => {
                  if (info.screenshot) setAnnotatingScreenshot(info.screenshot);
                },
              }}
            />
          </div>
        </div>
      ) : null}

      {tab === "Audio" ? (
        <AudioTab
          session={session}
          busy={busy}
          transcribing={transcribing}
          onTranscribe={handleTranscribeAudio}
          showLog={showAudioLog}
          onCloseLog={() => setShowAudioLog(false)}
          status={audioStatus}
          logs={audioLogs}
        />
      ) : null}

      {tab === "Replay" ? (
        <ReplayPanel
          sessionId={sessionId}
          hasDocumentation={Boolean(session.documentation_md?.trim())}
          steps={steps}
          setSteps={setSteps}
          screenshots={screenshots}
          refresh={refresh}
          onToast={setToast}
          onError={setError}
        />
      ) : null}

      {tab === "Recording" ? (
        <div className="card panel">
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", flexWrap: "wrap", gap: "10px" }}>
            <div>
              <h3>Screen Recording</h3>
              <div className="pd">
                {selectedVideoMode === "full"
                  ? "Video HD fluido a framerate continuo con audio microfono sincronizzato."
                  : "Timelapse leggero a 1 fotogramma al secondo generato dai frame della sessione."}
              </div>
            </div>

            <div className="seg">
              <button
                type="button"
                className={selectedVideoMode === "full" ? "on" : ""}
                onClick={() => setSelectedVideoMode("full")}
                title="Video HD continuo fluido con audio sincronizzato"
              >
                🎥 Video HD (30 FPS) {fullVideoSrc ? "✓" : ""}
              </button>
              <button
                type="button"
                className={selectedVideoMode === "timelapse" ? "on" : ""}
                onClick={() => setSelectedVideoMode("timelapse")}
                title="Video timelapse a 1 fotogramma al secondo"
              >
                ⏱️ Timelapse (1 FPS) {timelapseVideoSrc ? "✓" : ""}
              </button>
            </div>
          </div>

          <div className="video-tab">
            {activeVideoSrc ? (
              <video
                key={activeVideoSrc}
                src={activeVideoSrc}
                controls
                playsInline
                className="video-player"
                style={{ width: "100%", display: "block" }}
              />
            ) : (
              <div className="video-player">
                <div className="pbtn">
                  <Icon name="play" size={26} />
                </div>
                <div className="vbar">
                  <span>0:00</span>
                  <div className="track">
                    <i />
                  </div>
                  <span>{formatDuration(session.duration)}</span>
                </div>
                <div
                  style={{
                    position: "absolute",
                    inset: 0,
                    display: "grid",
                    placeItems: "center",
                    padding: 24,
                    textAlign: "center",
                    color: "var(--muted)",
                    fontSize: 14,
                  }}
                >
                  {videoEncoding
                    ? "Finalizzazione ed elaborazione video in background in corso…"
                    : "Nessun video registrato per questa modalità."}
                </div>
              </div>
            )}
          </div>
        </div>
      ) : null}

      {tab === "Exports" ? (
        <ExportPanel
          session={session}
          steps={steps}
          screenshots={screenshots}
          exports={exports}
          busy={busy}
          onExport={handleExport}
          onRevealExport={handleRevealExport}
        />
      ) : null}

      {docImagePick && (
        <ScreenshotPickerModal
          title="Cambia immagine nella documentazione"
          currentScreenshotId={docImagePick.screenshot?.id}
          screenshots={screenshots}
          onSelect={handleDocImageSelected}
          onClose={() => setDocImagePick(null)}
        />
      )}

      {duplicateGroups && (
        <DuplicateScreenshotsModal
          key={duplicateScanId}
          groups={duplicateGroups}
          screenshots={screenshots}
          onMerge={handleMergeDuplicates}
          onClose={() => setDuplicateGroups(null)}
          minSimilarity={duplicateThreshold}
          rescanning={scanningDuplicates}
          onRescan={async (value) => {
            changeDuplicateThreshold(value);
            await handleFindDuplicates(value);
          }}
        />
      )}

      {annotatingScreenshot && (
        <ImageAnnotationModal
          sessionId={sessionId}
          screenshot={annotatingScreenshot}
          onClose={() => setAnnotatingScreenshot(null)}
          onSaved={async () => {
            setToast("Screenshot aggiornato con successo!");
            setDocImageVersion((value) => value + 1);
            await refresh();
          }}
        />
      )}

      <ConfirmModal
        isOpen={Boolean(confirmModal?.isOpen)}
        title={confirmModal?.title ?? ""}
        message={confirmModal?.message ?? ""}
        confirmLabel={confirmModal?.confirmLabel}
        cancelLabel={confirmModal?.cancelLabel}
        kind={confirmModal?.kind ?? "warning"}
        onConfirm={async () => {
          if (confirmModal?.action) {
            const act = confirmModal.action;
            setConfirmModal(null);
            await act();
          }
        }}
        onCancel={() => setConfirmModal(null)}
      />

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </div>
  );
}
