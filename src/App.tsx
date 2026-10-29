import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Routes, Route, useNavigate } from "react-router-dom";
import { Sidebar } from "@/components/layout/Sidebar";
import { RecordingHUD } from "@/components/recording/RecordingHUD";
import { RecordingProvider, useRecordingContext } from "@/context/RecordingContext";
import { SessionsProvider } from "@/context/SessionsContext";
import { AiJobProvider, useAiJob } from "@/context/AiJobContext";
import { HomePage } from "@/pages/HomePage";
import { SessionPage } from "@/pages/SessionPage";
import { SettingsPage } from "@/pages/SettingsPage";
import { ConfirmModal } from "@/components/ui/ConfirmModal";
import { MirrorStartupCheck } from "@/components/sessions/MirrorStartupCheck";
import { BrowserExtensionFirstRunNotice } from "@/components/recording/BrowserExtensionStatus";
import {
  MirrorProgress,
  MirrorSyncIndicator,
  NETWORK_HINT,
  mirrorBusy,
  useMirrorStatus,
} from "@/components/sessions/MirrorStatusBanner";
import { api } from "@/lib/api";
import { formatDuration } from "@/lib/utils";
import { stageLabel } from "@/components/ai/ProcessingPanel";
import { recordedSeconds } from "@/hooks/useRecording";

function FloatingAiProgress() {
  const { activeJob, cancelJob, clearJob } = useAiJob();
  const navigate = useNavigate();

  if (!activeJob) return null;

  return (
    <div
      style={{
        position: "fixed",
        bottom: "20px",
        right: "20px",
        zIndex: 9999,
        background: "rgba(23, 23, 23, 0.95)",
        border: activeJob.error ? "1px solid #ef4444" : activeJob.isDone ? "1px solid #22c55e" : "1px solid rgba(255, 255, 255, 0.15)",
        boxShadow: "0 8px 32px rgba(0, 0, 0, 0.5)",
        backdropFilter: "blur(12px)",
        borderRadius: "10px",
        padding: "12px 16px",
        width: "320px",
        display: "flex",
        flexDirection: "column",
        gap: "8px",
        color: "#fff",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <div style={{ display: "flex", alignItems: "center", gap: "8px", fontWeight: 600, fontSize: "13px" }}>
          {activeJob.isDone ? (
            activeJob.error ? (
              <span style={{ color: "#ef4444" }}>❌ Generazione non riuscita</span>
            ) : (
              <span style={{ color: "#22c55e" }}>✨ Documentazione pronta</span>
            )
          ) : (
            <>
              <span className="pulse" style={{ width: "8px", height: "8px" }} />
              <span>Generazione documentazione in corso…</span>
            </>
          )}
        </div>
        {activeJob.isDone && (
          <button
            type="button"
            onClick={clearJob}
            style={{
              background: "transparent",
              border: "none",
              color: "#aaa",
              cursor: "pointer",
              fontSize: "14px",
            }}
          >
            ✕
          </button>
        )}
      </div>

      <div style={{ fontSize: "12px", color: "#ccc", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
        {activeJob.sessionTitle}
      </div>

      <div style={{ fontSize: "11px", color: "#888" }}>
        Fase: <span style={{ color: "#eee" }}>{stageLabel(activeJob.stage)}</span>
      </div>

      <div style={{ display: "flex", gap: "8px", marginTop: "4px" }}>
        <button
          type="button"
          onClick={() => navigate(`/sessions/${activeJob.sessionId}`)}
          style={{
            flex: 1,
            padding: "5px 10px",
            fontSize: "12px",
            borderRadius: "6px",
            background: "rgba(255, 255, 255, 0.1)",
            border: "none",
            color: "#fff",
            cursor: "pointer",
          }}
        >
          Vai alla sessione
        </button>
        {!activeJob.isDone && (
          <button
            type="button"
            onClick={() => cancelJob(activeJob.sessionId)}
            style={{
              padding: "5px 10px",
              fontSize: "12px",
              borderRadius: "6px",
              background: "rgba(239, 68, 68, 0.2)",
              border: "1px solid rgba(239, 68, 68, 0.4)",
              color: "#f87171",
              cursor: "pointer",
            }}
          >
            Annulla
          </button>
        )}
      </div>
    </div>
  );
}

/**
 * Closing the window during a recording loses it, and during a generation stops it: ask first.
 * While sessions are being saved to the shared sessions folder (or a video is still being encoded)
 * the app cannot be closed: the progress is shown and the window closes by itself when everything
 * is saved. If the folder cannot be reached the user may still close (the sessions stay on this
 * computer and are copied at the next start). With nothing running the window closes as usual.
 */
function CloseGuard() {
  const { recording, stop } = useRecordingContext();
  const { activeJob } = useAiJob();
  const [asking, setAsking] = useState<"recording" | "generation" | "upload" | "failed" | null>(null);
  const [closing, setClosing] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const generating = Boolean(activeJob && !activeJob.isDone);
  const watchingUpload = asking === "upload" || asking === "failed";
  const { status: mirror, refresh: refreshMirror } = useMirrorStatus(watchingUpload ? 1_000 : 60_000);

  useEffect(() => {
    const appWindow = getCurrentWindow();
    const unlisten = appWindow.onCloseRequested(async (event) => {
      if (recording) {
        event.preventDefault();
        setAsking("recording");
        return;
      }
      if (generating) {
        event.preventDefault();
        setAsking("generation");
        return;
      }
      // Through the hook, so the dialog starts from this same fresh status (null if unknown:
      // nothing to hold the window for).
      const status = await refreshMirror();
      if (mirrorBusy(status)) {
        event.preventDefault();
        setAsking("upload");
      } else if (status && status.pending_sessions > 0) {
        event.preventDefault();
        setAsking("failed");
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [recording, generating, refreshMirror]);

  // Upload finished: close if everything reached the folder, otherwise explain what is left.
  useEffect(() => {
    if (asking !== "upload" || !mirror || mirrorBusy(mirror)) return;
    if (mirror.pending_sessions > 0) {
      setAsking("failed");
    } else {
      void getCurrentWindow().destroy();
    }
  }, [asking, mirror]);

  if (asking === "recording") {
    return (
      <ConfirmModal
        isOpen
        title="Registrazione in corso"
        message={
          "Stai ancora registrando. Se chiudi FlowCapture adesso la registrazione va persa.\n\n" +
          "Premi \"Ferma e salva\" per terminarla e salvarla: potrai chiudere l'applicazione subito dopo."
        }
        confirmLabel="Ferma e salva"
        cancelLabel="Continua a registrare"
        kind="warning"
        isLoading={closing}
        onConfirm={async () => {
          setClosing(true);
          try {
            await stop();
          } catch {
            // The error is shown on the page.
          } finally {
            setClosing(false);
            setAsking(null);
          }
        }}
        onCancel={() => setAsking(null)}
      />
    );
  }

  if (asking === "generation") {
    return (
      <ConfirmModal
        isOpen
        title="Generazione in corso"
        message={
          "La documentazione è ancora in fase di generazione. Se chiudi FlowCapture adesso la generazione " +
          "viene interrotta e dovrai rifarla (con i relativi costi).\n\nVuoi chiudere comunque?"
        }
        confirmLabel="Chiudi comunque"
        cancelLabel="Aspetta"
        kind="warning"
        onConfirm={() => {
          setAsking(null);
          void getCurrentWindow().destroy();
        }}
        onCancel={() => setAsking(null)}
      />
    );
  }

  if (asking === "upload") {
    return (
      <ConfirmModal
        isOpen
        title="Salvataggio della sessione in corso"
        message={
          "C'è una sessione che sta venendo caricata nella cartella condivisa: non è possibile chiudere " +
          "FlowCapture finché il caricamento non è terminato.\n\n" +
          "FlowCapture si chiuderà da solo appena tutto sarà salvato."
        }
        confirmLabel="Non chiudere, continua a lavorare"
        kind="primary"
        hideCancel
        onConfirm={() => setAsking(null)}
        onCancel={() => undefined}
      >
        {mirror ? <MirrorProgress status={mirror} /> : null}
      </ConfirmModal>
    );
  }

  if (asking === "failed") {
    const count = mirror?.pending_sessions ?? 0;
    return (
      <ConfirmModal
        isOpen
        title="Cartella condivisa non raggiungibile"
        message={
          `${count === 1 ? "1 sessione non è stata salvata" : `${count} sessioni non sono state salvate`} ` +
          "nella cartella condivisa perché la cartella non è raggiungibile.\n\n" +
          `${NETWORK_HINT} Poi premi "Riprova".\n\n` +
          "Se chiudi comunque, le sessioni restano salvate su questo computer e verranno caricate al prossimo avvio."
        }
        confirmLabel={retrying ? "Nuovo tentativo…" : "Riprova"}
        cancelLabel="Chiudi comunque"
        kind="warning"
        isLoading={retrying}
        onConfirm={async () => {
          setRetrying(true);
          try {
            await api.syncSessionMirror();
            // Fresh status first: the copy just requested must count as running.
            await refreshMirror();
            setAsking("upload");
          } catch {
            // Still unreachable: the dialog stays.
          } finally {
            setRetrying(false);
          }
        }}
        onCancel={() => {
          setAsking(null);
          void getCurrentWindow().destroy();
        }}
      >
        {mirror?.last_error ? (
          <div className="mirror-progress-note" style={{ marginTop: 8 }}>
            {mirror.last_error}
          </div>
        ) : null}
      </ConfirmModal>
    );
  }

  return null;
}

function AppShell() {
  const { t } = useLanguage();
  const {
    recording,
    loading,
    stop,
    markStep,
    recordAudio,
    selectedMicId,
    audioDevices,
    highlightClicks,
    selectedMonitorName,
    monitors,
    selectedMonitorId,
    switchMonitor,
    isPaused,
    pauseClock,
    isMuted,
    pause,
    resume,
    toggleMute,
    switchMicrophone,
    micProblem,
  } = useRecordingContext();
  const [elapsed, setElapsed] = useState(0);

  useEffect(() => {
    if (!recording) {
      setElapsed(0);
      return;
    }

    const startedAt = recording.started_at;
    function tick() {
      setElapsed(recordedSeconds(startedAt, pauseClock));
    }

    tick();
    const interval = window.setInterval(tick, 1000);
    return () => window.clearInterval(interval);
  }, [recording, pauseClock]);

  useEffect(() => {
    const appWindow = getCurrentWindow();
    if (recording) {
      void appWindow.setTitle(`● Registrazione ${formatDuration(elapsed)} — FlowCapture`).catch(() => {});
    } else {
      void appWindow.setTitle("FlowCapture").catch(() => {});
    }
  }, [recording, elapsed]);

  const [sidebarOpen, setSidebarOpen] = useState(() => {
    return localStorage.getItem("flowcapture_sidebar_open") !== "false";
  });

  function toggleSidebar(open: boolean) {
    setSidebarOpen(open);
    localStorage.setItem("flowcapture_sidebar_open", open ? "true" : "false");
  }

  return (
    <div className="win">
      <div className={`app-body ${sidebarOpen ? "sidebar-open" : "sidebar-collapsed"}`}>
        {sidebarOpen && <Sidebar onClose={() => toggleSidebar(false)} />}
        <main className="main" style={{ position: "relative" }}>
          {!sidebarOpen && (
            <button
              type="button"
              className="btn-toggle-sidebar"
              onClick={() => toggleSidebar(true)}
              title={t("sidebar.show", "Show sidebar")}
            >
              ☰ {t("sidebar.menu", "Menu")}
            </button>
          )}
          <Routes>
            <Route path="/" element={<HomePage />} />
            <Route path="/sessions/:sessionId" element={<SessionPage />} />
            <Route path="/settings" element={<SettingsPage />} />
          </Routes>
        </main>
      </div>
      {recording ? (
        <RecordingHUD
          session={recording}
          loading={loading}
          onStop={() => stop()}
          onMarkStep={() => markStep()}
          recordAudio={recordAudio}
          selectedMicId={selectedMicId}
          audioDevices={audioDevices}
          onSwitchMicrophone={switchMicrophone}
          isMuted={isMuted}
          onToggleMute={toggleMute}
          isPaused={isPaused}
          pauseClock={pauseClock}
          onPause={pause}
          onResume={resume}
          monitors={monitors}
          selectedMonitorId={selectedMonitorId}
          onSwitchMonitor={switchMonitor}
          highlightClicks={highlightClicks}
          selectedMonitorName={selectedMonitorName}
          micProblem={micProblem}
        />
      ) : null}
      <FloatingAiProgress />
      <CloseGuard />
      <MirrorSyncIndicator />
      <MirrorStartupCheck />
      <BrowserExtensionFirstRunNotice />
    </div>
  );
}

import { LanguageProvider, useLanguage } from "@/i18n";

export default function App() {
  return (
    <LanguageProvider>
      <RecordingProvider>
        <SessionsProvider>
          <AiJobProvider>
            <AppShell />
          </AiJobProvider>
        </SessionsProvider>
      </RecordingProvider>
    </LanguageProvider>
  );
}

