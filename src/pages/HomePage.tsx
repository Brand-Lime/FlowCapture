import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { open } from "@tauri-apps/plugin-dialog";
import { AppButton } from "@/components/ui/AppButton";
import { ConfirmModal } from "@/components/ui/ConfirmModal";
import { Icon } from "@/components/ui/Icon";
import { Toast } from "@/components/ui/Toast";
import { PermissionsBanner } from "@/components/recording/PermissionsBanner";
import { MicrophoneTest } from "@/components/recording/MicrophoneTest";
import {
  BrowserExtensionWarning,
  describeMissingExtension,
  extensionBlocksRecording,
  useBrowserExtensionStatus,
} from "@/components/recording/BrowserExtensionStatus";
import { SessionThumbnail } from "@/components/sessions/SessionThumbnail";
import { MirrorStatusBanner } from "@/components/sessions/MirrorStatusBanner";
import { useRecordingContext } from "@/context/RecordingContext";
import { useSessionsContext } from "@/context/SessionsContext";
import { api, type Session } from "@/lib/api";
import { MicrophoneError } from "@/lib/microphone";
import { statusBadgeClass } from "@/lib/icons";
import { formatDuration, formatTimestamp } from "@/lib/utils";

export function HomePage() {
  const navigate = useNavigate();
  const {
    loading,
    error,
    setError,
    start,
    audioDevices,
    selectedMicId,
    setSelectedMicId,
    recordAudio,
    setRecordAudio,
    transcribeAudio,
    setTranscribeAudio,
    highlightClicks,
    setHighlightClicks,
    refreshAudioDevices,
    monitors,
    selectedMonitorId,
    setSelectedMonitorId,
    refreshMonitors,
  } = useRecordingContext();
  const { sessions, refreshSessions } = useSessionsContext();
  const [platform, setPlatform] = useState("");
  const [canRecord, setCanRecord] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [captureAllEvents, setCaptureAllEvents] = useState(false);
  const [transcriptionLanguage, setTranscriptionLanguage] = useState("it");
  const [importing, setImporting] = useState(false);
  const [toastMessage, setToastMessage] = useState<string | null>(null);
  const [sessionToDelete, setSessionToDelete] = useState<Session | null>(null);
  const [deletingSession, setDeletingSession] = useState(false);
  // Why the microphone cannot be used: the user can fix it or record without audio.
  const [micIssue, setMicIssue] = useState<string | null>(null);
  const [aiConfigured, setAiConfigured] = useState(true);
  const [isAdmin, setIsAdmin] = useState(false);

  useEffect(() => {
    api.isAdminMode().then(setIsAdmin).catch(() => undefined);
  }, []);

  useEffect(() => {
    api
      .listProviders()
      .then((providers) => setAiConfigured(providers.some((provider) => provider.enabled === 1)))
      .catch(() => undefined);
  }, []);

  // The saved microphone is not plugged in: recording uses the default one (see useRecording).
  const savedMicMissing =
    Boolean(selectedMicId) &&
    audioDevices.length > 0 &&
    audioDevices.every((device) => device.deviceId) &&
    !audioDevices.some((device) => device.deviceId === selectedMicId);

  const extension = useBrowserExtensionStatus();
  // Recording refused because the browser extension is missing (text of the dialog).
  const [extensionIssue, setExtensionIssue] = useState<string | null>(null);

  async function handleStart(withoutAudio = false) {
    // Checked again right now: the extension may have been installed or removed meanwhile.
    const extensionStatus = await extension.refresh();
    if (extensionStatus && extensionBlocksRecording(extensionStatus)) {
      setExtensionIssue(describeMissingExtension(extensionStatus));
      return;
    }
    if (recordAudio && !withoutAudio) {
      const devices = (await navigator.mediaDevices?.enumerateDevices?.().catch(() => [])) ?? [];
      if (!devices.some((device) => device.kind === "audioinput")) {
        setMicIssue("Non è collegato nessun microfono al computer.");
        return;
      }
    }
    try {
      await start(undefined, { withoutAudio });
    } catch (err) {
      if (err instanceof MicrophoneError) setMicIssue(err.message);
    }
  }

  /** Admin: imports the new and changed sessions from the sessions folder set in Settings. */
  async function handleImportFromMirror() {
    setImporting(true);
    setError(null);
    try {
      const summary = await api.importSessionsFromMirror();
      await refreshSessions();
      const parts = [
        `${summary.imported} nuove`,
        `${summary.updated} aggiornate`,
        `${summary.unchanged} invariate`,
      ];
      if (summary.skipped > 0) parts.push(`${summary.skipped} in copia (riprova tra poco)`);
      setToastMessage(`Importazione da ${summary.root}: ${parts.join(", ")}.`);
      if (summary.errors.length > 0) {
        setError(`Alcune sessioni non sono state importate:\n${summary.errors.join("\n")}`);
      }
    } catch (err) {
      setError(`Errore durante l'importazione dalla cartella sessioni: ${err}`);
    } finally {
      setImporting(false);
    }
  }

  async function handleImportSession() {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        title: "Seleziona sessione FlowCapture da importare",
        filters: [
          {
            name: "FlowCapture Session (*.flowcapture, *.zip)",
            extensions: ["flowcapture", "zip"],
          },
        ],
      });

      if (!selected) return;
      const filePath = typeof selected === "string" ? selected : selected[0];
      if (!filePath) return;

      setImporting(true);
      setError(null);

      const imported = await api.importSessionBundle(filePath);
      await refreshSessions();
      setToastMessage(`Sessione "${imported.title}" importata con successo!`);
      setTimeout(() => setToastMessage(null), 4000);
      navigate(`/sessions/${imported.id}`);
    } catch (err) {
      setError(`Errore durante l'importazione della sessione: ${err}`);
    } finally {
      setImporting(false);
    }
  }

  useEffect(() => {
    api.getSetting("capture_all_events").then((val) => {
      setCaptureAllEvents(val === "true");
    }).catch(() => undefined);
    api.getSetting("transcription_language").then((val) => {
      if (val) setTranscriptionLanguage(val);
    }).catch(() => undefined);
  }, []);

  function handlePermissionsChange(
    permissions: Awaited<ReturnType<typeof api.getRecordingPermissions>>,
  ) {
    setCanRecord(permissions.can_record);
  }

  async function refresh() {
    setRefreshing(true);
    try {
      const nextPlatform = await api.getPlatformName();
      await refreshSessions();
      setPlatform(nextPlatform);
      await refreshAudioDevices();
    } catch (err) {
      setError(String(err));
    } finally {
      setRefreshing(false);
    }
  }

  useEffect(() => {
    refresh().catch((err) => setError(String(err)));
  }, [setError]);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (
        (event.metaKey || event.ctrlKey) &&
        event.shiftKey &&
        event.key.toLowerCase() === "r" &&
        canRecord &&
        !loading
      ) {
        event.preventDefault();
        void handleStart();
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  return (
    <div className="page">
      <div className="home-hero home-top">
        <div>
          <div className="kick">FLOWCAPTURE</div>
          <h1>Turn workflows into documentation</h1>
          <p className="lead">
            Record screen activity, capture input events, and generate Markdown, HTML,
            PDF and video exports with your own AI provider.
          </p>
        </div>
        <span className="plat">
          <Icon name="monitor" size={15} /> Platform: {platform || "detecting…"}
        </span>
      </div>

      <PermissionsBanner onPermissionsChange={handlePermissionsChange} />

      <div className="card rec-card">
        <h3>Recording</h3>
        <p>
          Start a new session to record your screen, mouse activity, keyboard input, and
          window changes.
        </p>

        <div
          style={{
            margin: "18px 0",
            padding: "14px 16px",
            background: "rgba(255,255,255,0.03)",
            border: "1px solid var(--hair)",
            borderRadius: "8px",
            display: "flex",
            flexDirection: "column",
            gap: "12px",
          }}
        >
          {/* Schermo da registrare */}
          <div style={{ borderBottom: "1px solid var(--hair)", paddingBottom: "12px" }}>
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", flexWrap: "wrap", gap: "10px" }}>
              <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
                <span style={{ fontSize: "16px" }}>🖥️</span>
                <div>
                  <div style={{ fontWeight: 500, fontSize: "13.5px" }}>
                    Schermo da registrare {monitors.length > 1 ? `(${monitors.length} schermi rilevati)` : ""}
                  </div>
                  <div style={{ fontSize: "12px", color: "var(--dim)" }}>
                    Scegli quale monitor catturare durante la sessione di lavoro
                  </div>
                </div>
              </div>
              <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
                <select
                  id="home-monitor-select"
                  value={selectedMonitorId}
                  onChange={(e) => setSelectedMonitorId(e.target.value)}
                  style={{
                    background: "var(--surface)",
                    color: "var(--text)",
                    border: "1px solid var(--hair)",
                    borderRadius: "6px",
                    padding: "5px 10px",
                    fontSize: "12.5px",
                    maxWidth: "280px",
                  }}
                >
                  {monitors.map((mon, idx) => (
                    <option key={mon.id || idx} value={mon.id}>
                      {mon.name} {mon.is_primary ? "(Principale)" : ""} · {mon.width}x{mon.height}
                    </option>
                  ))}
                  {monitors.length === 0 && (
                    <option value="">Schermo Principale (Predefinito)</option>
                  )}
                </select>
                <AppButton
                  size="sm"
                  kind="ghost"
                  onClick={() => refreshMonitors()}
                  title="Rileva schermi"
                >
                  <Icon name="refresh" size={13} />
                </AppButton>
              </div>
            </div>
          </div>

          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", flexWrap: "wrap", gap: "10px" }}>
            <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
              <span style={{ fontSize: "16px" }}>🎯</span>
              <div>
                <div style={{ fontWeight: 500, fontSize: "13.5px" }}>Evidenzia click negli screenshot</div>
                <div style={{ fontSize: "12px", color: "var(--dim)" }}>Disegna un indicatore visivo ad alto contrasto dove viene premuto il mouse</div>
              </div>
            </div>
            <div className="seg">
              <button
                type="button"
                className={highlightClicks ? "on" : ""}
                onClick={() => setHighlightClicks(true)}
              >
                On
              </button>
              <button
                type="button"
                className={!highlightClicks ? "on" : ""}
                onClick={() => setHighlightClicks(false)}
              >
                Off
              </button>
            </div>
          </div>

          <div style={{ borderTop: "1px solid var(--hair)", paddingTop: "12px" }}>
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", flexWrap: "wrap", gap: "10px" }}>
              <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
                <span style={{ fontSize: "16px" }}>🎙️</span>
                <div>
                  <div style={{ fontWeight: 500, fontSize: "13.5px" }}>Registra audio microfono</div>
                  <div style={{ fontSize: "12px", color: "var(--dim)" }}>Cattura la tua voce e le spiegazioni durante la registrazione</div>
                </div>
              </div>
              <div className="seg">
                <button
                  type="button"
                  className={recordAudio ? "on" : ""}
                  onClick={() => setRecordAudio(true)}
                >
                  On
                </button>
                <button
                  type="button"
                  className={!recordAudio ? "on" : ""}
                  onClick={() => setRecordAudio(false)}
                >
                  Off
                </button>
              </div>
            </div>

            {recordAudio ? (
              <div style={{ marginTop: "12px", paddingLeft: "26px", display: "flex", flexDirection: "column", gap: "10px" }}>
                <div style={{ display: "flex", alignItems: "center", gap: "10px", flexWrap: "wrap" }}>
                  <label htmlFor="mic-select" style={{ fontSize: "12.5px", color: "var(--text-2)", minWidth: "120px" }}>
                    Scelta Microfono:
                  </label>
                  <select
                    id="mic-select"
                    value={selectedMicId}
                    onChange={(e) => setSelectedMicId(e.target.value)}
                    style={{
                      background: "var(--surface)",
                      color: "var(--text)",
                      border: "1px solid var(--hair)",
                      padding: "6px 12px",
                      borderRadius: "6px",
                      fontSize: "13px",
                      minWidth: "220px",
                    }}
                  >
                    <option value="">Microfono Predefinito</option>
                    {audioDevices.map((device, idx) => (
                      <option key={device.deviceId || idx} value={device.deviceId}>
                        {device.label || `Microfono ${idx + 1}`}
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    onClick={() => {
                      navigator.mediaDevices
                        ?.getUserMedia({ audio: true })
                        .then((stream) => {
                          stream.getTracks().forEach((t) => t.stop());
                        })
                        .catch(() => undefined)
                        .finally(() => refreshAudioDevices());
                    }}
                    style={{
                      background: "transparent",
                      border: "none",
                      color: "var(--mint)",
                      fontSize: "12px",
                      cursor: "pointer",
                      textDecoration: "underline",
                    }}
                  >
                    Aggiorna lista
                  </button>
                </div>

                {audioDevices.length === 0 ? (
                  <div className="warn-card danger" style={{ marginTop: 0 }}>
                    <span>🎙️</span>
                    <div className="warn-card-body">
                      <strong>Nessun microfono rilevato.</strong> Collega un microfono o una cuffia con microfono e
                      premi "Aggiorna lista". Se non ti serve l'audio, metti "Registra audio microfono" su Off.
                    </div>
                  </div>
                ) : savedMicMissing ? (
                  <div className="warn-card" style={{ marginTop: 0 }}>
                    <span>🎙️</span>
                    <div className="warn-card-body">
                      <strong>Il microfono scelto l'ultima volta non è collegato:</strong> verrà usato il microfono
                      predefinito di Windows. Ricollegalo o scegline un altro dalla lista.
                    </div>
                  </div>
                ) : null}

                <MicrophoneTest
                  deviceId={savedMicMissing ? "" : selectedMicId}
                  onDevicesChanged={() => void refreshAudioDevices()}
                />

                <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", flexWrap: "wrap", gap: "10px" }}>
                  <div style={{ fontSize: "12.5px", color: "var(--text-2)" }}>
                    Trascrizione audio automatica (AI):
                  </div>
                  <div className="seg">
                    <button
                      type="button"
                      className={transcribeAudio ? "on" : ""}
                      onClick={() => setTranscribeAudio(true)}
                    >
                      Attiva
                    </button>
                    <button
                      type="button"
                      className={!transcribeAudio ? "on" : ""}
                      onClick={() => setTranscribeAudio(false)}
                    >
                      Disattiva
                    </button>
                  </div>
                </div>

                {transcribeAudio && (
                  <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", flexWrap: "wrap", gap: "10px" }}>
                    <label htmlFor="home-trans-lang" style={{ fontSize: "12.5px", color: "var(--text-2)" }}>
                      Lingua Trascrizione:
                    </label>
                    <select
                      id="home-trans-lang"
                      value={transcriptionLanguage}
                      onChange={async (e) => {
                        const val = e.target.value;
                        setTranscriptionLanguage(val);
                        await api.setSetting("transcription_language", val);
                      }}
                      style={{
                        background: "var(--surface)",
                        color: "var(--text)",
                        border: "1px solid var(--hair)",
                        padding: "4px 8px",
                        borderRadius: "6px",
                        fontSize: "12px",
                      }}
                    >
                      <option value="it">Italiano (it)</option>
                      <option value="en">Inglese (en)</option>
                      <option value="es">Spagnolo (es)</option>
                      <option value="fr">Francese (fr)</option>
                      <option value="de">Tedesco (de)</option>
                      <option value="auto">Rilevamento automatico (auto)</option>
                    </select>
                  </div>
                )}
              </div>
            ) : null}
          </div>

          <div style={{ borderTop: "1px solid var(--hair)", paddingTop: "12px", display: "flex", alignItems: "center", justifyContent: "space-between", flexWrap: "wrap", gap: "10px" }}>
            <div style={{ display: "flex", alignItems: "center", gap: "8px" }}>
              <span style={{ fontSize: "16px" }}>📸</span>
              <div>
                <div style={{ fontWeight: 500, fontSize: "13.5px" }}>Screenshot su ogni evento (Modalità densa)</div>
                <div style={{ fontSize: "12px", color: "var(--dim)" }}>Cattura uno screenshot non solo ai click ma anche per tastiera e scroll</div>
              </div>
            </div>
            <div className="seg">
              <button
                type="button"
                className={captureAllEvents ? "on" : ""}
                onClick={async () => {
                  setCaptureAllEvents(true);
                  await api.setSetting("capture_all_events", "true");
                }}
              >
                On
              </button>
              <button
                type="button"
                className={!captureAllEvents ? "on" : ""}
                onClick={async () => {
                  setCaptureAllEvents(false);
                  await api.setSetting("capture_all_events", "false");
                }}
              >
                Off
              </button>
            </div>
          </div>
        </div>

        <div className="rec-actions" style={{ display: "flex", alignItems: "center", gap: "10px", flexWrap: "wrap" }}>
          <AppButton
            kind="primary"
            icon="play"
            disabled={loading || !canRecord}
            onClick={() => void handleStart()}
          >
            Start Recording
          </AppButton>
          <AppButton
            icon="folder"
            disabled={importing}
            onClick={() => void handleImportSession()}
          >
            {importing ? "Importazione…" : "Importa Sessione (.flowcapture)"}
          </AppButton>
          {isAdmin ? (
            <AppButton
              kind="ghost"
              icon="refresh"
              disabled={importing}
              onClick={() => void handleImportFromMirror()}
              title="Importa dalla cartella sessioni delle Impostazioni le sessioni registrate sugli altri computer (solo quelle nuove o modificate)"
            >
              {importing ? "Importazione…" : "Importa da cartella (admin)"}
            </AppButton>
          ) : null}
          <span style={{ fontFamily: "var(--mono)", fontSize: 12.5, color: "var(--dim)" }}>
            ⌘⇧R
          </span>
        </div>
      </div>

      <BrowserExtensionWarning
        status={extension.status}
        checking={extension.checking}
        onRecheck={() => void extension.refresh()}
      />

      <MirrorStatusBanner />

      {!aiConfigured ? (
        <div className="warn-card info">
          <span>✨</span>
          <div className="warn-card-body">
            <strong>Nessun servizio AI configurato.</strong> Puoi registrare normalmente, ma per trascrivere l'audio e
            generare la documentazione serve un provider AI (chiave API) nelle Impostazioni.
          </div>
          <div className="warn-card-actions">
            <AppButton size="sm" onClick={() => navigate("/settings")}>
              Apri Impostazioni
            </AppButton>
          </div>
        </div>
      ) : null}

      {error ? (
        <div
          className="card set-card"
          style={{
            marginTop: 18,
            borderColor: "rgba(255,138,138,.45)",
            background: "rgba(255,100,100,0.06)",
            color: "var(--rose)",
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            gap: "12px",
          }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: "10px" }}>
            <Icon name="alert" size={18} />
            <span style={{ fontSize: "13px", lineHeight: "1.4", whiteSpace: "pre-line" }}>{error}</span>
          </div>
          <button
            type="button"
            onClick={() => setError(null)}
            style={{
              background: "rgba(255, 255, 255, 0.08)",
              border: "none",
              borderRadius: "6px",
              color: "var(--rose)",
              cursor: "pointer",
              padding: "4px 8px",
              fontSize: "13px",
              fontWeight: "bold",
              flexShrink: 0,
            }}
            title="Chiudi messaggio di errore"
          >
            ✕
          </button>
        </div>
      ) : null}

      <div className="sec-row">
        <h2>Recent Sessions</h2>
        <AppButton
          size="sm"
          icon="refresh"
          className={refreshing ? "is-spinning" : undefined}
          disabled={refreshing}
          onClick={() => refresh()}
        >
          {refreshing ? "Refreshing…" : "Refresh"}
        </AppButton>
      </div>
      <div className="sess-list">
        {sessions.length === 0 ? (
          <div className="card panel">
            <div className="pd">
              No sessions yet. Start your first recording to build documentation.
            </div>
          </div>
        ) : (
          sessions.map((session) => (
            <div
              key={session.id}
              className="sess"
              role="button"
              tabIndex={0}
              onClick={() => navigate(`/sessions/${session.id}`)}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  navigate(`/sessions/${session.id}`);
                }
              }}
              style={{ position: "relative" }}
            >
              <SessionThumbnail path={session.preview_screenshot_path} />
              <div className="sinfo">
                <div className="stt">{session.title}</div>
                <div className="stm">
                  {formatTimestamp(session.started_at)}
                  <span className={`badge ${statusBadgeClass(session.status)}`}>
                    {session.status}
                  </span>
                  <span>{formatDuration(session.duration)}</span>
                </div>
              </div>
              <div className="sdur">{formatDuration(session.duration)}</div>
              <button
                type="button"
                title="Elimina sessione"
                onClick={(e) => {
                  e.stopPropagation();
                  setSessionToDelete(session);
                }}
                style={{
                  background: "transparent",
                  border: "none",
                  color: "#ef4444",
                  padding: "6px",
                  cursor: "pointer",
                  borderRadius: "4px",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  opacity: 0.7,
                  transition: "opacity 0.2s",
                }}
                onMouseEnter={(e) => ((e.currentTarget as HTMLElement).style.opacity = "1")}
                onMouseLeave={(e) => ((e.currentTarget as HTMLElement).style.opacity = "0.7")}
              >
                <Icon name="trash" size={16} />
              </button>
            </div>
          ))
        )}
      </div>

      <ConfirmModal
        isOpen={Boolean(sessionToDelete)}
        title="Elimina Sessione"
        message={`Sei sicuro di voler eliminare definitivamente la sessione "${sessionToDelete?.title}"?\n\nTutti gli eventi, gli screenshot e i file associati verranno rimossi in modo permanente.`}
        confirmLabel="Elimina Sessione"
        cancelLabel="Annulla"
        kind="danger"
        isLoading={deletingSession}
        onConfirm={async () => {
          if (!sessionToDelete) return;
          setDeletingSession(true);
          try {
            await api.deleteSession(sessionToDelete.id);
            await refreshSessions();
            setToastMessage(`Sessione "${sessionToDelete.title}" eliminata.`);
            setSessionToDelete(null);
          } catch (delErr) {
            setError(String(delErr));
          } finally {
            setDeletingSession(false);
          }
        }}
        onCancel={() => setSessionToDelete(null)}
      />

      <ConfirmModal
        isOpen={Boolean(extensionIssue)}
        title="Registrazione non disponibile: manca l'estensione del browser"
        message={extensionIssue ?? ""}
        confirmLabel="Ricontrolla"
        cancelLabel="Chiudi"
        kind="warning"
        isLoading={extension.checking}
        onConfirm={async () => {
          const next = await extension.refresh();
          if (next && extensionBlocksRecording(next)) {
            setExtensionIssue(describeMissingExtension(next));
          } else {
            setExtensionIssue(null);
            void handleStart();
          }
        }}
        onCancel={() => setExtensionIssue(null)}
      />

      <ConfirmModal
        isOpen={Boolean(micIssue)}
        title="Problema con il microfono"
        message={`${micIssue ?? ""}\n\nPuoi risolvere il problema e premere di nuovo "Start Recording" (usa "Prova microfono" per controllare), oppure registrare solo lo schermo, senza la tua voce.`}
        confirmLabel="Registra senza audio"
        cancelLabel="Annulla"
        kind="warning"
        onConfirm={() => {
          setMicIssue(null);
          void handleStart(true);
        }}
        onCancel={() => setMicIssue(null)}
      />

      <Toast message={toastMessage} onDismiss={() => setToastMessage(null)} />
    </div>
  );
}
