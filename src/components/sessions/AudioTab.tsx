import { convertFileSrc } from "@tauri-apps/api/core";
import { useMemo, useRef, useState } from "react";
import { AppButton } from "@/components/ui/AppButton";
import { LogPanel } from "@/components/ui/LogPanel";
import type { AudioSegment, Session } from "@/lib/api";

export type AudioTranscriptionStatus = "idle" | "transcribing" | "success" | "error";

interface AudioTabProps {
  session: Session;
  busy: boolean;
  transcribing: boolean;
  onTranscribe: () => void;
  showLog: boolean;
  onCloseLog: () => void;
  status: AudioTranscriptionStatus;
  logs: string[];
}

/** "Audio" tab of a session: microphone track, transcription log and timed transcript. */
export function AudioTab({
  session,
  busy,
  transcribing,
  onTranscribe,
  showLog,
  onCloseLog,
  status,
  logs,
}: AudioTabProps) {
  const [audioViewMode, setAudioViewMode] = useState<"segments" | "text">("segments");
  const audioPlayerRef = useRef<HTMLAudioElement | null>(null);

  const audioSegments: AudioSegment[] = useMemo(() => {
    if (!session.audio_segments_json) return [];
    try {
      const parsed = JSON.parse(session.audio_segments_json);
      return Array.isArray(parsed) ? parsed : [];
    } catch {
      return [];
    }
  }, [session.audio_segments_json]);

  return (
    <div className="card panel">
      <div className="panel-head">
        <div>
          <h3>Registrazione audio e trascrizione</h3>
          <div className="pd">Riascolta l'audio del microfono e leggi la trascrizione del parlato fatta dall'AI.</div>
        </div>
        {session.audio_path ? (
          <AppButton
            kind="primary"
            icon="sparkles"
            size="sm"
            disabled={transcribing || busy}
            onClick={onTranscribe}
          >
            {transcribing ? "Trascrizione in corso…" : session.audio_transcript ? "Trascrivi di nuovo" : "Trascrivi con AI"}
          </AppButton>
        ) : null}
      </div>

      {showLog && (
        <LogPanel
          status={status === "success" || status === "error" ? status : "running"}
          messages={{
            running: "Trascrizione dell'audio in corso con il servizio AI…",
            success: "Trascrizione completata.",
            error: "Errore durante la trascrizione",
          }}
          logs={logs}
          onClose={onCloseLog}
        />
      )}

      {session.audio_path ? (
        <div style={{ marginTop: "16px", display: "flex", flexDirection: "column", gap: "16px" }}>
          <div className="sub-box">
            <div className="sub-box-title">Traccia audio del microfono</div>
            <audio
              ref={audioPlayerRef}
              controls
              src={convertFileSrc(session.audio_path)}
              style={{ width: "100%", height: "40px" }}
            />
          </div>

          <div className="sub-box">
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "12px", flexWrap: "wrap", gap: "8px" }}>
              <div style={{ display: "flex", alignItems: "center", gap: "10px" }}>
                <div className="sub-box-title" style={{ marginBottom: 0 }}>
                  Trascrizione del parlato
                </div>
                {session.audio_transcript ? <span className="chip ok">● Trascritto</span> : null}
                {audioSegments.length > 0 ? (
                  <span className="chip info">{audioSegments.length} segmenti con orario</span>
                ) : null}
              </div>

              {session.audio_transcript && audioSegments.length > 0 && (
                <div className="seg seg-sm">
                  <button
                    type="button"
                    className={audioViewMode === "segments" ? "on" : ""}
                    onClick={() => setAudioViewMode("segments")}
                  >
                    Segmenti ({audioSegments.length})
                  </button>
                  <button
                    type="button"
                    className={audioViewMode === "text" ? "on" : ""}
                    onClick={() => setAudioViewMode("text")}
                  >
                    Testo continuo
                  </button>
                </div>
              )}
            </div>
            {session.audio_transcript ? (
              audioViewMode === "segments" && audioSegments.length > 0 ? (
                <div style={{ display: "flex", flexDirection: "column", gap: "8px", maxHeight: "480px", overflowY: "auto", paddingRight: "4px" }}>
                  {audioSegments.map((seg, idx) => {
                    const startSec = Math.floor(seg.start_ms / 1000);
                    const endSec = Math.floor(seg.end_ms / 1000);
                    const fmtTime = (s: number) => `${Math.floor(s / 60).toString().padStart(2, "0")}:${(s % 60).toString().padStart(2, "0")}`;
                    return (
                      <div
                        key={idx}
                        style={{
                          display: "flex",
                          alignItems: "flex-start",
                          gap: "12px",
                          padding: "10px 14px",
                          borderRadius: "6px",
                          background: "var(--surface)",
                          border: "1px solid var(--hair)",
                          transition: "background 0.15s",
                        }}
                      >
                        <button
                          type="button"
                          title="Ascolta questo segmento"
                          onClick={() => {
                            if (audioPlayerRef.current) {
                              audioPlayerRef.current.currentTime = seg.start_ms / 1000;
                              audioPlayerRef.current.play();
                            }
                          }}
                          style={{
                            display: "flex",
                            alignItems: "center",
                            gap: "5px",
                            background: "rgba(108, 198, 255, 0.12)",
                            border: "1px solid rgba(108, 198, 255, 0.3)",
                            borderRadius: "4px",
                            padding: "3px 8px",
                            color: "var(--ice)",
                            fontSize: "11px",
                            fontFamily: "monospace",
                            cursor: "pointer",
                            flexShrink: 0,
                            marginTop: "1px",
                          }}
                        >
                          <span>▶</span>
                          <span>{fmtTime(startSec)} - {fmtTime(endSec)}</span>
                        </button>
                        <div style={{ flex: 1, fontSize: "13.5px", lineHeight: "1.5", color: "var(--text)" }}>
                          {seg.text}
                        </div>
                        {typeof seg.avg_logprob === "number" && seg.avg_logprob !== 0 && (
                          <span
                            title={`Affidabilità stimata della trascrizione (logprob ${seg.avg_logprob.toFixed(2)})`}
                            style={{
                              fontSize: "10px",
                              padding: "2px 6px",
                              borderRadius: "4px",
                              background: seg.avg_logprob > -0.6 ? "rgba(52, 211, 153, 0.1)" : "rgba(251, 191, 36, 0.1)",
                              color: seg.avg_logprob > -0.6 ? "#34d399" : "#fbbf24",
                              fontFamily: "monospace",
                              flexShrink: 0,
                            }}
                          >
                            {Math.round(Math.min(100, Math.max(0, Math.exp(seg.avg_logprob) * 100)))}%
                          </span>
                        )}
                      </div>
                    );
                  })}
                </div>
              ) : (
                <div>
                  {audioSegments.length === 0 && (
                    <div className="warn-card" style={{ marginTop: 0, marginBottom: 12 }}>
                      <span>💡</span>
                      <div className="warn-card-body">
                        Questa trascrizione è solo testo, senza l'orario di ogni frase. Premi{" "}
                        <strong>"Trascrivi di nuovo"</strong> in alto a destra per dividerla in segmenti con l'orario.
                      </div>
                    </div>
                  )}
                  <div
                    style={{
                      whiteSpace: "pre-wrap",
                      fontSize: "14px",
                      lineHeight: "1.6",
                      color: "var(--text)",
                      background: "var(--surface)",
                      padding: "14px 16px",
                      borderRadius: "6px",
                      border: "1px solid var(--hair)",
                    }}
                  >
                    {session.audio_transcript}
                  </div>
                </div>
              )
            ) : (
              <div style={{ color: "var(--dim)", fontSize: "13.5px" }}>
                Nessuna trascrizione ancora. Premi "Trascrivi con AI" in alto a destra per convertire la voce in testo.
              </div>
            )}
          </div>
        </div>
      ) : (
        <div className="pd" style={{ marginTop: "16px" }}>
          Nessun audio del microfono registrato per questa sessione. Per registrare l'audio attiva "Registra audio microfono" prima di iniziare la registrazione.
        </div>
      )}
    </div>
  );
}
