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
import { statusBadgeClass, statusLabel } from "@/lib/icons";
import { SettingSwitch } from "@/components/ui/SettingSwitch";
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
            name: "Sessione FlowCapture (*.flowcapture, *.zip)",
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
          <h1>Trasforma il tuo lavoro in una guida</h1>
          <p className="lead">
            Registra lo schermo, i click e la tastiera e ottieni una guida passo-passo in Markdown, HTML,
            PDF o video, scritta dal servizio AI configurato.
          </p>
        </div>
        <span className="plat">
          <Icon name="monitor" size={15} /> Sistema: {platform || "rilevamento…"}
        </span>
      </div>

      <PermissionsBanner onPermissionsChange={handlePermissionsChange} />

      <div className="card rec-card">
        <h3>Registrazione</h3>
        <p>
          Avvia una nuova sessione per registrare lo schermo, il mouse, la tastiera e i cambi di finestra.
        </p>

        <div className="rec-options">
          <div className="field setting-row rec-option">
            <div className="setting-label">
              <span className="setting-emoji">🖥️</span>
              <div>
                <label htmlFor="home-monitor-select">
                  Schermo da registrare {monitors.length > 1 ? `(${monitors.length} schermi rilevati)` : ""}
                </label>
                <div className="setting-desc">Scegli quale monitor registrare durante la sessione</div>
              </div>
            </div>
            <div className="settings-device-row">
              <select
                id="home-monitor-select"
                className="settings-select rec-select"
                value={selectedMonitorId}
                onChange={(e) => setSelectedMonitorId(e.target.value)}
              >
                {monitors.map((mon, idx) => (
                  <option key={mon.id || idx} value={mon.id}>
                    {mon.name} {mon.is_primary ? "(Principale)" : ""} · {mon.width}x{mon.height}
                  </option>
                ))}
                {monitors.length === 0 && <option value="">Schermo principale (predefinito)</option>}
              </select>
              <AppButton
                size="sm"
                kind="ghost"
                icon="refresh"
                onClick={() => refreshMonitors()}
                title="Rileva schermi"
                aria-label="Rileva schermi"
              />
            </div>
          </div>

          <SettingSwitch
            className="rec-option"
            emoji="🎯"
            label="Evidenzia click negli screenshot"
            description="Disegna un indicatore ad alto contrasto nel punto in cui premi il mouse"
            value={highlightClicks}
            onChange={setHighlightClicks}
          />

          <div className="rec-option-group">
            <SettingSwitch
              className="rec-option"
              emoji="🎙️"
              label="Registra audio microfono"
              description="Registra la tua voce e le tue spiegazioni durante il lavoro"
              value={recordAudio}
              onChange={setRecordAudio}
            />

            {recordAudio ? (
              <div className="rec-suboptions">
                <div className="settings-inline-field">
                  <label htmlFor="mic-select">Microfono:</label>
                  <select
                    id="mic-select"
                    className="settings-select rec-select"
                    value={selectedMicId}
                    onChange={(e) => setSelectedMicId(e.target.value)}
                  >
                    <option value="">Microfono predefinito</option>
                    {audioDevices.map((device, idx) => (
                      <option key={device.deviceId || idx} value={device.deviceId}>
                        {device.label || `Microfono ${idx + 1}`}
                      </option>
                    ))}
                  </select>
                  <AppButton
                    size="sm"
                    kind="ghost"
                    icon="refresh"
                    onClick={() => {
                      navigator.mediaDevices
                        ?.getUserMedia({ audio: true })
                        .then((stream) => {
                          stream.getTracks().forEach((t) => t.stop());
                        })
                        .catch(() => undefined)
                        .finally(() => refreshAudioDevices());
                    }}
                  >
                    Aggiorna lista
                  </AppButton>
                </div>

                {audioDevices.length === 0 ? (
                  <div className="warn-card danger" style={{ marginTop: 0 }}>
                    <span>🎙️</span>
                    <div className="warn-card-body">
                      <strong>Nessun microfono rilevato.</strong> Collega un microfono o una cuffia con microfono e
                      premi "Aggiorna lista". Se non ti serve l'audio, metti "Registra audio microfono" su Disattivo.
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

                <SettingSwitch
                  className="rec-option rec-option-sub"
                  label="Trascrizione audio automatica (AI)"
                  value={transcribeAudio}
                  onChange={setTranscribeAudio}
                />

                {transcribeAudio && (
                  <div className="settings-inline-field">
                    <label htmlFor="home-trans-lang">Lingua trascrizione:</label>
                    <select
                      id="home-trans-lang"
                      className="settings-select rec-select"
                      value={transcriptionLanguage}
                      onChange={async (e) => {
                        const val = e.target.value;
                        setTranscriptionLanguage(val);
                        await api.setSetting("transcription_language", val);
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

          <SettingSwitch
            className="rec-option"
            emoji="📸"
            label="Screenshot su ogni evento (modalità densa)"
            description="Cattura uno screenshot non solo ai click ma anche per tastiera e scorrimento"
            value={captureAllEvents}
            onChange={async (next) => {
              setCaptureAllEvents(next);
              await api.setSetting("capture_all_events", String(next));
            }}
          />
        </div>

        <div className="rec-actions">
          <AppButton
            kind="primary"
            icon="play"
            disabled={loading || !canRecord}
            onClick={() => void handleStart()}
          >
            Avvia registrazione
          </AppButton>
          <AppButton
            icon="folder"
            disabled={importing}
            onClick={() => void handleImportSession()}
          >
            {importing ? "Importazione…" : "Importa sessione (.flowcapture)"}
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
          <span className="kbd-hint" title="Scorciatoia da tastiera per avviare la registrazione">
            Ctrl+Shift+R
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
        <div className="card set-card error-card">
          <div className="error-card-body">
            <Icon name="alert" size={18} />
            <span style={{ whiteSpace: "pre-line" }}>{error}</span>
          </div>
          <button type="button" className="error-card-close" onClick={() => setError(null)} title="Chiudi messaggio di errore">
            ✕
          </button>
        </div>
      ) : null}

      <div className="sec-row">
        <h2>Sessioni recenti</h2>
        <AppButton
          size="sm"
          icon="refresh"
          className={refreshing ? "is-spinning" : undefined}
          disabled={refreshing}
          onClick={() => refresh()}
        >
          {refreshing ? "Aggiornamento…" : "Aggiorna"}
        </AppButton>
      </div>
      <div className="sess-list">
        {sessions.length === 0 ? (
          <div className="card panel">
            <div className="pd">
              Nessuna sessione. Avvia la prima registrazione per creare una guida.
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
                    {statusLabel(session.status)}
                  </span>
                  <span>{formatDuration(session.duration)}</span>
                </div>
              </div>
              <div className="sdur">{formatDuration(session.duration)}</div>
              <button
                type="button"
                className="sess-delete"
                title="Elimina sessione"
                aria-label="Elimina sessione"
                onClick={(e) => {
                  e.stopPropagation();
                  setSessionToDelete(session);
                }}
              >
                <Icon name="trash" size={16} />
              </button>
            </div>
          ))
        )}
      </div>

      <ConfirmModal
        isOpen={Boolean(sessionToDelete)}
        title="Elimina sessione"
        message={`Sei sicuro di voler eliminare definitivamente la sessione "${sessionToDelete?.title}"?\n\nTutti gli eventi, gli screenshot e i file associati verranno rimossi in modo permanente.`}
        confirmLabel="Elimina sessione"
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
        message={`${micIssue ?? ""}\n\nPuoi risolvere il problema e premere di nuovo "Avvia registrazione" (usa "Prova microfono" per controllare), oppure registrare solo lo schermo, senza la tua voce.`}
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
