import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { AppButton } from "@/components/ui/AppButton";
import { MirrorStatusBanner } from "@/components/sessions/MirrorStatusBanner";
import {
  BrowserExtensionList,
  extensionBlocksRecording,
  useBrowserExtensionStatus,
} from "@/components/recording/BrowserExtensionStatus";
import { api, type ProviderConfig, type MonitorInfo } from "@/lib/api";
import { Icon } from "@/components/ui/Icon";
import { providerGlyph } from "@/lib/icons";
import { useLanguage } from "@/i18n";

export function SettingsPage() {
  const { t, language, setLanguage } = useLanguage();
  const [providers, setProviders] = useState<ProviderConfig[]>([]);
  const [redactionEnabled, setRedactionEnabled] = useState(true);
  const [recordAudio, setRecordAudio] = useState(false);
  const [transcribeAudio, setTranscribeAudio] = useState(true);
  const [highlightClicks, setHighlightClicks] = useState(true);
  const [selectedMonitorId, setSelectedMonitorId] = useState("");
  const [monitors, setMonitors] = useState<MonitorInfo[]>([]);
  const [selectedMicId, setSelectedMicId] = useState("");
  const [audioDevices, setAudioDevices] = useState<MediaDeviceInfo[]>([]);
  const [captureAllEvents, setCaptureAllEvents] = useState(false);
  const [dedupeScreenshots, setDedupeScreenshots] = useState(true);
  const [transcriptionProviderId, setTranscriptionProviderId] = useState("");
  const [transcriptionModel, setTranscriptionModel] = useState("");
  const [transcriptionBaseUrl, setTranscriptionBaseUrl] = useState("");
  const [transcriptionLanguage, setTranscriptionLanguage] = useState("it");
  const [aiThinkingMode, setAiThinkingMode] = useState("auto");
  // Empty by default: a saved temperature would be sent with every request.
  const [aiCustomParams, setAiCustomParams] = useState("");
  const [customParamsError, setCustomParamsError] = useState<string | null>(null);
  const [aiGenerationTimeout, setAiGenerationTimeout] = useState("600");
  const [docLanguage, setDocLanguage] = useState("Italian");
  const [prices, setPrices] = useState<Record<string, { input: string; output: string }>>({});
  const [recordFullVideo, setRecordFullVideo] = useState(true);
  const [fullVideoFps, setFullVideoFps] = useState("30");
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [mirrorDir, setMirrorDir] = useState<string | null>("\\\\srvdati\\GenAuto13485");
  // Off: the folder stays set (e.g. for the admin import) but the sessions are not written into it.
  const [mirrorWrite, setMirrorWrite] = useState(true);
  const [isAdmin, setIsAdmin] = useState(false);
  // Admin only: folder with the complete, unzipped copy of every session.
  const [fullCopyDir, setFullCopyDir] = useState<string | null>(null);
  const extension = useBrowserExtensionStatus();

  useEffect(() => {
    api.isAdminMode().then(setIsAdmin).catch(() => {});
  }, []);
  const [bridgeInfo, setBridgeInfo] = useState<{ port: number; recording: boolean } | null>(null);

  async function refreshDevices() {
    try {
      if (!navigator?.mediaDevices?.enumerateDevices) return;
      const devices = await navigator.mediaDevices.enumerateDevices();
      setAudioDevices(devices.filter((d) => d.kind === "audioinput"));
    } catch {
      // ignore
    }
  }

  async function refreshMonitors() {
    try {
      const list = await api.listMonitors();
      setMonitors(list);
    } catch {
      // ignore
    }
  }

  async function refresh() {
    const [
      nextProviders,
      nextSetting,
      nextRecordAudio,
      nextTranscribe,
      nextHighlight,
      nextMicId,
      nextMonitorId,
      nextCaptureAll,
      nextTransProvId,
      nextTransModel,
      nextTransUrl,
      nextTransLang,
      nextThinkingMode,
      nextCustomParams,
      nextMonitors,
      nextRecordFullVideo,
      nextFullVideoFps,
      nextAiTimeout,
      nextDedupe,
      nextDocLanguage,
    ] = await Promise.all([
      api.listProviders(),
      api.getSetting("redaction_enabled"),
      api.getSetting("record_audio"),
      api.getSetting("transcribe_audio"),
      api.getSetting("highlight_clicks"),
      api.getSetting("selected_microphone_id"),
      api.getSetting("selected_monitor_id"),
      api.getSetting("capture_all_events"),
      api.getSetting("transcription_provider_id"),
      api.getSetting("transcription_model"),
      api.getSetting("transcription_base_url"),
      api.getSetting("transcription_language"),
      api.getSetting("ai_thinking_mode"),
      api.getSetting("ai_custom_parameters"),
      api.listMonitors().catch(() => [] as MonitorInfo[]),
      api.getSetting("record_full_video"),
      api.getSetting("full_video_fps"),
      api.getSetting("ai_generation_timeout_seconds"),
      api.getSetting("dedupe_screenshots"),
      api.getSetting("documentation_language"),
    ]);
    setProviders(nextProviders);
    setRedactionEnabled((nextSetting ?? "true") === "true");
    setRecordAudio(nextRecordAudio === "true");
    setTranscribeAudio((nextTranscribe ?? "true") === "true");
    setHighlightClicks((nextHighlight ?? "true") === "true");
    setCaptureAllEvents(nextCaptureAll === "true");
    setDedupeScreenshots((nextDedupe ?? "true") === "true");
    setTranscriptionProviderId(nextTransProvId ?? "");
    setTranscriptionModel(nextTransModel ?? "");
    setTranscriptionBaseUrl(nextTransUrl ?? "");
    setTranscriptionLanguage(nextTransLang ?? "it");
    setAiThinkingMode(nextThinkingMode ?? "auto");
    setAiGenerationTimeout(nextAiTimeout || "600");
    setDocLanguage(nextDocLanguage || "Italian");
    const priceEntries = await Promise.all(
      nextProviders.map(async (p) => {
        const [input, output] = await Promise.all([
          api.getSetting(`price_input_per_mtok:${p.id}`),
          api.getSetting(`price_output_per_mtok:${p.id}`),
        ]);
        return [p.id, { input: input ?? "", output: output ?? "" }] as const;
      }),
    );
    setPrices(Object.fromEntries(priceEntries));
    setRecordFullVideo((nextRecordFullVideo ?? "true") !== "false");
    setFullVideoFps(nextFullVideoFps || "30");
    if (nextCustomParams) {
      setAiCustomParams(nextCustomParams);
    }
    if (nextMicId) setSelectedMicId(nextMicId);
    if (nextMonitorId) setSelectedMonitorId(nextMonitorId);
    setMonitors(nextMonitors);
    api.getBrowserBridgeStatus().then(setBridgeInfo).catch(() => {});
    api.getSessionMirrorDir().then(setMirrorDir).catch(() => {});
    api.getSetting("session_mirror_write").then((value) => setMirrorWrite(value !== "false")).catch(() => {});
    api.getSessionFullCopyDir().then(setFullCopyDir).catch(() => {});
    await refreshDevices();
  }

  useEffect(() => {
    refresh().catch((err) => setError(String(err)));
  }, []);

  async function saveProvider(provider: ProviderConfig) {
    setMessage(null);
    setError(null);
    try {
      await api.updateProvider(provider);
      setMessage(`${provider.name} saved`);
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  }

  async function handleSaveCustomParams(jsonStr: string) {
    const trimmed = jsonStr.trim();
    if (!trimmed) {
      setCustomParamsError(null);
      setAiCustomParams("{}");
      await api.setSetting("ai_custom_parameters", "{}");
      setMessage(t("settings.custom_params.saved_empty", "Custom parameters saved ({})"));
      return;
    }
    try {
      const parsed = JSON.parse(trimmed);
      if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
        setCustomParamsError(t("settings.custom_params.err_obj", "Parameters must be a valid JSON object"));
        return;
      }
      setCustomParamsError(null);
      const formatted = JSON.stringify(parsed, null, 2);
      setAiCustomParams(formatted);
      await api.setSetting("ai_custom_parameters", formatted);
      setMessage(t("settings.custom_params.saved_success", "Custom parameters saved successfully"));
    } catch (e: any) {
      setCustomParamsError(`Errore sintassi JSON: ${e?.message ?? e}`);
    }
  }

  function applyPreset(presetObj: Record<string, any>) {
    const formatted = JSON.stringify(presetObj, null, 2);
    setAiCustomParams(formatted);
    setCustomParamsError(null);
    api.setSetting("ai_custom_parameters", formatted).then(() => {
      setMessage(t("settings.custom_params.preset_applied", "Custom parameters preset applied"));
    });
  }

  const activeProvider = providers.find((provider) => provider.enabled);

  async function chooseMirrorDir() {
    try {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: "Cartella in cui salvare una copia di ogni sessione",
        defaultPath: mirrorDir ?? undefined,
      });
      if (typeof picked !== "string" || !picked) return;
      const count = await api.setSessionMirrorDir(picked);
      console.log(`Mirror directory set to: ${picked}, ${count} sessions copied`);
      setMirrorDir(picked);
      setMessage(`Cartella sessioni impostata: copia di ${count} sessioni in corso`);
    } catch (err) {
      setError(String(err));
    }
  }

  async function disableMirrorDir() {
    try {
      await api.setSessionMirrorDir(null);
      setMirrorDir(null);
      setMessage("Copia automatica delle sessioni disattivata (i file già copiati restano dove sono)");
    } catch (err) {
      setError(String(err));
    }
  }

  async function changeMirrorWrite(next: boolean) {
    setMirrorWrite(next);
    await saveSetting("session_mirror_write", String(next));
    if (next) {
      // Copies what changed while writing was off.
      const count = await api.syncSessionMirror().catch(() => 0);
      setMessage(`Salvataggio nella cartella sessioni riattivato: aggiornamento di ${count} sessioni in corso`);
    } else {
      setMessage("Le sessioni non vengono più scritte nella cartella sessioni (i file già copiati restano dove sono)");
    }
  }

  async function chooseFullCopyDir() {
    try {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: "Cartella in cui salvare la copia completa di ogni sessione",
        defaultPath: fullCopyDir ?? undefined,
      });
      if (typeof picked !== "string" || !picked) return;
      const count = await api.setSessionFullCopyDir(picked);
      setFullCopyDir(picked);
      setMessage(`Cartella "Copia completa" impostata: copia di ${count} sessioni in corso`);
    } catch (err) {
      setError(String(err));
    }
  }

  async function removeFullCopyDir() {
    try {
      await api.setSessionFullCopyDir(null);
      setFullCopyDir(null);
      setMessage("Copia completa delle sessioni disattivata (i file già copiati restano dove sono)");
    } catch (err) {
      setError(String(err));
    }
  }

  async function syncMirrorNow() {
    try {
      const count = await api.syncSessionMirror();
      setMessage(`Aggiornamento di ${count} sessioni in corso`);
    } catch (err) {
      setError(String(err));
    }
  }

  async function saveSetting(key: string, value: string) {
    try {
      await api.setSetting(key, value);
    } catch (err) {
      setError(String(err));
    }
  }

  return (
    <div className="page settings-page">
      <div className="home-hero">
        <h1 className="settings-title">Settings</h1>
        <p className="lead">Configure BYOK AI providers and privacy controls.</p>
      </div>

      {message ? (
        <div className="banner settings-banner">
          <span className="bt">{message}</span>
          <button type="button" className="settings-banner-close" onClick={() => setMessage(null)}>
            ✕
          </button>
        </div>
      ) : null}
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

      <div className="card set-card">
        <h3>{t("settings.language.title")}</h3>
        <div className="sub">{t("settings.language.sub")}</div>
        <div className="field settings-narrow">
          <select
            className="settings-select"
            value={language}
            onChange={(e) => setLanguage(e.target.value as any)}
          >
            <option value="en">English (US)</option>
            <option value="it">Italiano (IT)</option>
          </select>
        </div>
      </div>

      <div className="card set-card">
        <h3>{t("settings.privacy.title")}</h3>
        <div className="sub">{t("settings.privacy.sub")}</div>
        <SettingSwitch
          narrow
          label={t("settings.redaction.label")}
          value={redactionEnabled}
          onLabel={t("settings.enabled")}
          offLabel={t("settings.disabled")}
          onChange={(next) => {
            setRedactionEnabled(next);
            return saveSetting("redaction_enabled", String(next));
          }}
        />
      </div>

      <div className="card set-card">
        <h3>Cartella sessioni</h3>
        {isAdmin ? (
          <div className="sub">
            Cartella condivisa in cui gli utenti salvano le loro sessioni. Come amministratore questa cartella viene
            solo letta da "Importa da cartella" (sessioni nuove o modificate di tutti gli utenti): le sessioni di
            questo computer non vengono mai scritte qui, ma nella "Copia completa" qui sotto.
          </div>
        ) : (
          <div className="sub">
            Salva automaticamente ogni sessione in questa cartella come file <code>.flowcapture</code> (un unico file
            con video, audio, screenshot, documentazione ed esportazioni), con il nome della sessione. Il file si
            aggiorna da solo a ogni modifica. Può essere una cartella di rete condivisa (es.{" "}
            <code>{"\\\\server\\FlowCapture\\nome.cognome"}</code>): se non è raggiungibile, le sessioni vengono
            salvate appena torna disponibile.
          </div>
        )}
        {mirrorWrite && !isAdmin ? <MirrorStatusBanner /> : null}
        <div className="mirror-path">
          <Icon name="folder" size={16} />
          <span className={mirrorDir ? "" : "mirror-path-empty"}>{mirrorDir ?? "Nessuna cartella: copia disattivata"}</span>
        </div>
        <div className="settings-actions mirror-actions">
          <AppButton kind="primary" icon="folder" onClick={chooseMirrorDir}>
            {mirrorDir ? "Cambia cartella" : "Scegli cartella"}
          </AppButton>
          {mirrorDir ? (
            <>
              <AppButton kind="ghost" onClick={() => api.openSessionMirrorDir().catch((err) => setError(String(err)))}>
                Apri cartella
              </AppButton>
              {mirrorWrite && !isAdmin ? (
                <AppButton kind="ghost" onClick={syncMirrorNow}>
                  Aggiorna ora
                </AppButton>
              ) : null}
              <AppButton kind="ghost" onClick={disableMirrorDir}>
                Rimuovi cartella
              </AppButton>
            </>
          ) : null}
        </div>
        {mirrorDir && !isAdmin ? (
          <SettingSwitch
            narrow
            label="Salva le sessioni in questa cartella"
            value={mirrorWrite}
            onLabel="Attivo"
            offLabel="Disattivo"
            onChange={(next) => changeMirrorWrite(next)}
          />
        ) : null}
        {mirrorDir && !mirrorWrite && !isAdmin ? (
          <div className="pd" style={{ marginTop: 8 }}>
            La cartella resta impostata (serve a "Importa da cartella" per gli amministratori), ma le sessioni di questo
            computer non vengono scritte al suo interno.
          </div>
        ) : null}
      </div>

      {isAdmin ? (
        <div className="card set-card">
          <h3>Copia completa delle sessioni (amministratore)</h3>
          <div className="sub">
            Salva automaticamente una copia di ogni sessione registrata su questo computer, come un'esportazione
            completa non compressa: video, audio, screenshot, documentazione, esportazioni e un{" "}
            <code>manifest.json</code> con passaggi ed eventi. Ogni sessione ha una sottocartella con il suo nome,
            rinominata insieme alla sessione; la copia si aggiorna da sola a ogni modifica. Le sessioni importate da
            altri utenti non vengono copiate qui.
          </div>
          {fullCopyDir ? <MirrorStatusBanner /> : null}
          <div className="mirror-path">
            <Icon name="folder" size={16} />
            <span className={fullCopyDir ? "" : "mirror-path-empty"}>
              {fullCopyDir ?? "Nessuna cartella: copia completa disattivata"}
            </span>
          </div>
          <div className="settings-actions mirror-actions">
            <AppButton kind="primary" icon="folder" onClick={chooseFullCopyDir}>
              {fullCopyDir ? "Cambia cartella" : "Scegli cartella"}
            </AppButton>
            {fullCopyDir ? (
              <>
                <AppButton kind="ghost" onClick={() => api.openSessionFullCopyDir().catch((err) => setError(String(err)))}>
                  Apri cartella
                </AppButton>
                <AppButton kind="ghost" onClick={syncMirrorNow}>
                  Aggiorna ora
                </AppButton>
                <AppButton kind="ghost" onClick={removeFullCopyDir}>
                  Rimuovi cartella
                </AppButton>
              </>
            ) : null}
          </div>
        </div>
      ) : null}

      <div className="card set-card">
        <h3>{t("settings.diagnostics.title")}</h3>
        <div className="sub">{t("settings.diagnostics.sub")}</div>
        <div className="settings-actions">
          <AppButton
            kind="ghost"
            onClick={async () => {
              try {
                const path = await api.openLogsFolder();
                setMessage(`Logs folder opened: ${path}`);
              } catch (e: any) {
                setError(`Could not open logs folder: ${e?.message ?? e}`);
              }
            }}
          >
            {t("settings.open_logs")}
          </AppButton>
        </div>
      </div>

      <div className="card set-card">
        <div className="settings-card-head">
          <div>
            <h3>{t("settings.extension.title")}</h3>
            <div className="sub">{t("settings.extension.sub")}</div>
          </div>
          <div className="bridge-pill">
            <span className="bridge-dot" />
            {t("settings.bridge_active")} (127.0.0.1:{bridgeInfo?.port ?? 41789})
          </div>
        </div>

        {extension.status?.supported ? (
          <div style={{ marginTop: 14 }}>
            <BrowserExtensionList status={extension.status} />
            {extensionBlocksRecording(extension.status) ? (
              <div className="warn-card danger">
                <span>🧩</span>
                <div className="warn-card-body">
                  <strong>La registrazione è bloccata finché l'estensione non è installata in tutti i browser
                  usati.</strong>{" "}
                  Contatta il team IT per farla installare.
                </div>
              </div>
            ) : null}
            <div className="settings-actions">
              <AppButton kind="ghost" icon="refresh" disabled={extension.checking} onClick={() => void extension.refresh()}>
                {extension.checking ? "Controllo…" : "Ricontrolla"}
              </AppButton>
            </div>
          </div>
        ) : null}

        {isAdmin ? (
          <>
            {extension.status?.supported ? (
              <SettingSwitch
                narrow
                label="Estensione obbligatoria per registrare (admin)"
                value={extension.status.required}
                onLabel="Sì"
                offLabel="No"
                onChange={async (next) => {
                  await saveSetting("browser_extension_required", String(next));
                  await extension.refresh();
                }}
              />
            ) : null}

            <div className="settings-infobox">
              <div className="settings-infobox-title">{t("settings.extension.install_title")}</div>
              <ol>
                <li>{t("settings.extension.step1")}</li>
                <li>{t("settings.extension.step2")}</li>
                <li>{t("settings.extension.step3")}</li>
              </ol>
              <div className="settings-infobox-note">{t("settings.extension.note")}</div>
            </div>

            <div className="settings-actions">
              <AppButton
                kind="primary"
                onClick={async () => {
                  try {
                    const path = await api.openBrowserExtensionFolder();
                    setMessage(`Extension folder opened: ${path}`);
                  } catch (e: any) {
                    setError(`Could not open extension folder: ${e?.message ?? e}`);
                  }
                }}
              >
                {t("settings.open_extension")}
              </AppButton>
            </div>
          </>
        ) : (
          <div className="pd" style={{ marginTop: 10 }}>
            L'estensione viene installata dal team IT: se manca in uno dei tuoi browser, contattalo.
          </div>
        )}
      </div>

      <div className="card set-card">
        <h3>{t("settings.recording.title")}</h3>
        <div className="sub">{t("settings.recording.sub")}</div>

        <SettingSwitch
          label={t("settings.highlight.label")}
          description={t("settings.highlight.sub")}
          value={highlightClicks}
          onLabel={t("settings.active")}
          offLabel={t("settings.inactive")}
          onChange={(next) => {
            setHighlightClicks(next);
            return saveSetting("highlight_clicks", String(next));
          }}
        />

        <SettingSwitch
          label={t("settings.dedupe.label", "Scarta screenshot duplicati")}
          description={t(
            "settings.dedupe.sub",
            "Confronta ogni screenshot con il precedente pixel per pixel e non salva quelli identici (cursore e hover esclusi).",
          )}
          value={dedupeScreenshots}
          onLabel={t("settings.active")}
          offLabel={t("settings.inactive")}
          onChange={(next) => {
            setDedupeScreenshots(next);
            return saveSetting("dedupe_screenshots", String(next));
          }}
        />

        <SettingSwitch
          label={t("settings.video.label")}
          description={t("settings.video.sub")}
          value={recordFullVideo}
          onLabel={t("settings.active")}
          offLabel={t("settings.inactive")}
          onChange={(next) => {
            setRecordFullVideo(next);
            return saveSetting("record_full_video", String(next));
          }}
        />

        {recordFullVideo && (
          <div className="field setting-row">
            <div>
              <label>{t("settings.fps.label")}</label>
              <div className="setting-desc">{t("settings.fps.sub")}</div>
            </div>
            <div className="seg">
              {["15", "30", "60"].map((fps) => (
                <button
                  key={fps}
                  type="button"
                  className={fullVideoFps === fps ? "on" : ""}
                  onClick={async () => {
                    setFullVideoFps(fps);
                    await saveSetting("full_video_fps", fps);
                  }}
                >
                  {fps} FPS
                </button>
              ))}
            </div>
          </div>
        )}

        <SettingSwitch
          label={t("settings.mic.label")}
          description={t("settings.mic.sub")}
          value={recordAudio}
          onLabel={t("settings.active")}
          offLabel={t("settings.inactive")}
          onChange={(next) => {
            setRecordAudio(next);
            return saveSetting("record_audio", String(next));
          }}
        />

        <SettingSwitch
          label={t("settings.transcription.label")}
          description={t("settings.transcription.sub")}
          value={transcribeAudio}
          onLabel={t("settings.active")}
          offLabel={t("settings.inactive")}
          onChange={(next) => {
            setTranscribeAudio(next);
            return saveSetting("transcribe_audio", String(next));
          }}
        />

        {transcribeAudio && (
          <div className="settings-subpanel">
            <div className="settings-subpanel-title">Configurazione Motore di Trascrizione</div>

            <div className="settings-inline-field">
              <label>{t("settings.transcription_provider")}</label>
              <select
                className="settings-input"
                value={transcriptionProviderId}
                onChange={async (e) => {
                  const val = e.target.value;
                  setTranscriptionProviderId(val);
                  await saveSetting("transcription_provider_id", val);
                }}
              >
                <option value="">Usa Provider Attivo Globale</option>
                {providers.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name} ({p.provider_type})
                  </option>
                ))}
              </select>
            </div>

            <div className="settings-inline-field">
              <label>{t("settings.transcription_model")}</label>
              <input
                type="text"
                className="settings-input"
                placeholder="es. whisper-1, gemini-2.5-flash, whisper, faster-whisper"
                value={transcriptionModel}
                onChange={(e) => setTranscriptionModel(e.target.value)}
                onBlur={(e) => saveSetting("transcription_model", e.target.value)}
              />
            </div>

            <div className="settings-inline-field">
              <label>{t("settings.transcription_url")}</label>
              <input
                type="text"
                className="settings-input"
                placeholder="es. http://localhost:11434 o http://localhost:8000"
                value={transcriptionBaseUrl}
                onChange={(e) => setTranscriptionBaseUrl(e.target.value)}
                onBlur={(e) => saveSetting("transcription_base_url", e.target.value)}
              />
            </div>

            <div className="settings-inline-field">
              <label>{t("settings.transcription_lang")}</label>
              <select
                className="settings-input"
                value={transcriptionLanguage}
                onChange={async (e) => {
                  const val = e.target.value;
                  setTranscriptionLanguage(val);
                  await saveSetting("transcription_language", val);
                }}
              >
                <option value="it">Italiano (it)</option>
                <option value="en">English (en)</option>
                <option value="es">Español (es)</option>
                <option value="fr">Français (fr)</option>
                <option value="de">Deutsch (de)</option>
                <option value="auto">Auto-detect (auto)</option>
              </select>
            </div>

            <div className="settings-note">{t("settings.transcription_note")}</div>
          </div>
        )}

        <SettingSwitch
          label={t("settings.dense.label")}
          description={t("settings.dense.sub")}
          value={captureAllEvents}
          onLabel={t("settings.active")}
          offLabel={t("settings.inactive")}
          onChange={(next) => {
            setCaptureAllEvents(next);
            return saveSetting("capture_all_events", String(next));
          }}
        />

        <div className="field settings-device">
          <label htmlFor="settings-mon-select">{t("settings.monitor.label")}</label>
          <div className="settings-device-row">
            <select
              id="settings-mon-select"
              className="settings-select"
              value={selectedMonitorId}
              onChange={async (e) => {
                const val = e.target.value;
                setSelectedMonitorId(val);
                await saveSetting("selected_monitor_id", val);
              }}
            >
              {monitors.map((mon, i) => (
                <option key={mon.id || i} value={mon.id}>
                  {mon.name} {mon.is_primary ? "(Principale)" : ""} · {mon.width}x{mon.height}
                </option>
              ))}
              {monitors.length === 0 && <option value="">{t("settings.primary_screen")}</option>}
            </select>
            <button type="button" className="settings-outline-btn" onClick={() => refreshMonitors()}>
              {t("settings.detect")}
            </button>
          </div>
        </div>

        <div className="field settings-device">
          <label htmlFor="settings-mic-select">{t("settings.mic_default.label")}</label>
          <div className="settings-device-row">
            <select
              id="settings-mic-select"
              className="settings-select"
              value={selectedMicId}
              onChange={async (e) => {
                const val = e.target.value;
                setSelectedMicId(val);
                await saveSetting("selected_microphone_id", val);
              }}
            >
              <option value="">{t("settings.system_default")}</option>
              {audioDevices.map((d, i) => (
                <option key={d.deviceId || i} value={d.deviceId}>
                  {d.label || `Microfono ${i + 1}`}
                </option>
              ))}
            </select>
            <button
              type="button"
              className="settings-outline-btn"
              onClick={() => {
                navigator.mediaDevices
                  ?.getUserMedia({ audio: true })
                  .then((stream) => {
                    stream.getTracks().forEach((track) => track.stop());
                    return refreshDevices();
                  })
                  .catch(() => undefined);
              }}
            >
              {t("settings.detect")}
            </button>
          </div>
        </div>
      </div>

      <div className="card set-card">
        <h3>Parametri AI & Modalità Thinking</h3>
        <div className="sub">
          Controlla il ragionamento (Think / No-Think) e specifica parametri JSON personalizzati inviati alle API dei modelli AI (LM Studio, Ollama, OpenAI, Claude, ecc.).
        </div>

        <div className="settings-section">
          <label className="settings-label">Modalità Ragionamento (Think / No-Think)</label>
          <div className="seg settings-seg">
            {[
              { value: "auto", label: "Automatico (Consigliato)" },
              { value: "think", label: "Think Abilitato" },
              { value: "no_think", label: "Disabilita Think" },
            ].map((mode) => (
              <button
                key={mode.value}
                type="button"
                className={aiThinkingMode === mode.value ? "on" : ""}
                onClick={async () => {
                  setAiThinkingMode(mode.value);
                  await saveSetting("ai_thinking_mode", mode.value);
                }}
              >
                {mode.label}
              </button>
            ))}
          </div>

          <div className="settings-summary">
            <div className="settings-summary-row">
              <span className="settings-summary-key">📝 Generazione Documento:</span>
              <span className={aiThinkingMode === "no_think" ? "settings-summary-off" : "settings-summary-on"}>
                {aiThinkingMode === "no_think"
                  ? "⚡ No-Think (disattivato da impostazione)"
                  : "🧠 Think attivo (analizza ed elabora le azioni con ragionamento)"}
              </span>
            </div>
            <div className="settings-summary-row">
              <span className="settings-summary-key">🌐 Traduzione Documento:</span>
              <span className="settings-summary-info">
                ⚡ No-Think forzato (traduzione diretta, massima velocità e zero riflessioni interne)
              </span>
            </div>
          </div>
        </div>

        <div className="settings-section">
          <label className="settings-label">Lingua della documentazione generata</label>
          <div className="sub settings-help">
            Lingua in cui l'AI scrive la guida. I titoli delle sezioni seguono la lingua scelta, così l'export HTML li riconosce.
          </div>
          <div className="seg settings-seg-inline">
            {[
              { label: "Italiano", val: "Italian" },
              { label: "English", val: "English" },
            ].map((item) => (
              <button
                key={item.val}
                type="button"
                className={docLanguage === item.val ? "on" : ""}
                onClick={async () => {
                  setDocLanguage(item.val);
                  await saveSetting("documentation_language", item.val);
                  setMessage(`Lingua della documentazione: ${item.label}`);
                }}
              >
                {item.label}
              </button>
            ))}
          </div>
        </div>

        <div className="settings-section">
          <label className="settings-label">Timeout Generazione Documentazione AI</label>
          <div className="sub settings-help">
            Tempo massimo di attesa per la stesura della documentazione (il consolidamento dei passaggi usa metà di questo tempo). Allo scadere viene usato il template base. Per guide lunghe o modelli locali (Ollama / LM Studio) usa 600s o più.
          </div>
          <div className="settings-timeout-row">
            <div className="seg settings-seg-inline">
              {[
                { label: "60s", val: "60" },
                { label: "120s (2m)", val: "120" },
                { label: "180s (3m)", val: "180" },
                { label: "300s (5m)", val: "300" },
                { label: "600s (10m - Consigliato)", val: "600" },
                { label: "1200s (20m)", val: "1200" },
              ].map((item) => (
                <button
                  key={item.val}
                  type="button"
                  className={aiGenerationTimeout === item.val ? "on" : ""}
                  onClick={async () => {
                    setAiGenerationTimeout(item.val);
                    await saveSetting("ai_generation_timeout_seconds", item.val);
                    setMessage(`Timeout AI impostato a ${item.val} secondi`);
                  }}
                >
                  {item.label}
                </button>
              ))}
            </div>
            <div className="settings-timeout-custom">
              <input
                type="number"
                className="settings-number"
                min="30"
                max="3600"
                step="10"
                value={aiGenerationTimeout}
                onChange={async (e) => {
                  const val = e.target.value;
                  setAiGenerationTimeout(val);
                  if (val && !isNaN(Number(val)) && Number(val) >= 10) {
                    await saveSetting("ai_generation_timeout_seconds", val);
                  }
                }}
              />
              <span className="settings-note">sec</span>
            </div>
          </div>
        </div>

        <div className="settings-section">
          <div className="settings-params-head">
            <label className="settings-label">Parametri Personalizzati Richieste (JSON)</label>
            <div className="settings-presets">
              <button type="button" className="settings-preset" onClick={() => applyPreset({ temperature: 0.2 })}>
                Preset Base
              </button>
              <button
                type="button"
                className="settings-preset"
                onClick={() => applyPreset({ temperature: 0.3, top_p: 0.95, max_tokens: 32768 })}
              >
                Preset LM Studio / OpenAI
              </button>
              <button
                type="button"
                className="settings-preset"
                onClick={() => applyPreset({ options: { temperature: 0.2, num_ctx: 32768 } })}
              >
                Preset Ollama
              </button>
              <button type="button" className="settings-preset muted" onClick={() => applyPreset({})}>
                Svuota ({"{}"})
              </button>
            </div>
          </div>

          <div className="settings-note settings-params-help">
            Questi parametri verranno inseriti direttamente nel payload della richiesta API inviata ai modelli AI.
          </div>

          <textarea
            className={`settings-json${customParamsError ? " invalid" : ""}`}
            value={aiCustomParams}
            onChange={(e) => {
              const val = e.target.value;
              setAiCustomParams(val);
              try {
                if (val.trim()) {
                  JSON.parse(val);
                }
                setCustomParamsError(null);
              } catch (err: any) {
                setCustomParamsError(`JSON non valido: ${err?.message ?? err}`);
              }
            }}
            onBlur={() => handleSaveCustomParams(aiCustomParams)}
            rows={5}
            placeholder={'{\n  "temperature": 0.2\n}'}
          />

          <div className="settings-json-status">
            {customParamsError ? (
              <span className="settings-json-error">⚠ {customParamsError}</span>
            ) : (
              <span className="settings-json-ok">✓ JSON valido (salvato automaticamente)</span>
            )}

            <AppButton size="sm" kind="ghost" onClick={() => handleSaveCustomParams(aiCustomParams)}>
              💾 Salva Parametri
            </AppButton>
          </div>
        </div>
      </div>

      {providers.map((provider) => (
        <div key={provider.id} className="card set-card">
          <div className="prov-head">
            <span className="pg">{providerGlyph(provider.provider_type)}</span>
            <div>
              <h3>{provider.name}</h3>
              <div className="pid">{provider.provider_type}</div>
            </div>
            {activeProvider?.id === provider.id ? <span className="active-tag">● Active</span> : null}
          </div>
          <div className="field-row settings-provider-row">
            <div className="field">
              <label htmlFor={`${provider.id}-key`}>API Key</label>
              <input
                id={`${provider.id}-key`}
                type="password"
                placeholder="sk-••••••••••••••••"
                defaultValue={provider.api_key ?? ""}
                onBlur={(event) => saveProvider({ ...provider, api_key: event.target.value })}
              />
            </div>
            <div className="field">
              <label htmlFor={`${provider.id}-model`}>Model</label>
              <input
                id={`${provider.id}-model`}
                defaultValue={provider.model ?? ""}
                onBlur={(event) => saveProvider({ ...provider, model: event.target.value })}
              />
            </div>
          </div>
          <div className="field">
            <label htmlFor={`${provider.id}-url`}>Base URL</label>
            <input
              id={`${provider.id}-url`}
              defaultValue={provider.base_url ?? ""}
              onBlur={(event) => saveProvider({ ...provider, base_url: event.target.value })}
            />
          </div>
          <div className="field-row settings-provider-row">
            {(["input", "output"] as const).map((kind) => (
              <div className="field" key={kind}>
                <label htmlFor={`${provider.id}-price-${kind}`}>
                  {kind === "input" ? "Prezzo input ($ / 1M token)" : "Prezzo output ($ / 1M token)"}
                </label>
                <input
                  id={`${provider.id}-price-${kind}`}
                  inputMode="decimal"
                  placeholder="automatico"
                  value={prices[provider.id]?.[kind] ?? ""}
                  onChange={(event) =>
                    setPrices((prev) => ({
                      ...prev,
                      [provider.id]: { ...(prev[provider.id] ?? { input: "", output: "" }), [kind]: event.target.value },
                    }))
                  }
                  onBlur={(event) =>
                    api
                      .setSetting(`price_${kind}_per_mtok:${provider.id}`, event.target.value.trim())
                      .catch((err) => setError(String(err)))
                  }
                />
              </div>
            ))}
          </div>
          <div className="sub settings-price-note">
            Usati per la stima dei costi. Vuoti = listino noto del modello. Con abbonamenti o proxy (es. GitHub Copilot) o modelli locali imposta 0.
          </div>
          <div className="settings-actions wide">
            <AppButton
              kind={activeProvider?.id === provider.id ? "ghost" : "primary"}
              onClick={() => {
                if (activeProvider?.id !== provider.id) {
                  saveProvider({ ...provider, enabled: 1 });
                }
              }}
            >
              {activeProvider?.id === provider.id ? "Active provider" : "Set Active"}
            </AppButton>
          </div>
        </div>
      ))}
    </div>
  );
}

/** Label + description on the left, a two-option segmented switch on the right. */
function SettingSwitch({
  label,
  description,
  value,
  onLabel,
  offLabel,
  onChange,
  narrow = false,
}: {
  label: string;
  description?: string;
  value: boolean;
  onLabel: string;
  offLabel: string;
  onChange: (next: boolean) => void | Promise<void>;
  narrow?: boolean;
}) {
  return (
    <div className={`field setting-row${narrow ? " narrow" : ""}`}>
      <div>
        <label>{label}</label>
        {description ? <div className="setting-desc">{description}</div> : null}
      </div>
      <div className="seg">
        <button type="button" className={value ? "on" : ""} onClick={() => onChange(true)}>
          {onLabel}
        </button>
        <button type="button" className={!value ? "on" : ""} onClick={() => onChange(false)}>
          {offLabel}
        </button>
      </div>
    </div>
  );
}
