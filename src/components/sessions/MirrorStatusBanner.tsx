import { useCallback, useEffect, useState } from "react";
import { AppButton } from "@/components/ui/AppButton";
import { api, type MirrorStatus } from "@/lib/api";

const POLL_MS = 3_000;

/** Status of the background copy to the sessions folder, refreshed every `pollMs`. */
export function useMirrorStatus(pollMs = POLL_MS) {
  const [status, setStatus] = useState<MirrorStatus | null>(null);

  const refresh = useCallback(async () => {
    try {
      const next = await api.getSessionMirrorStatus();
      setStatus(next);
      return next;
    } catch {
      return null;
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), pollMs);
    return () => window.clearInterval(timer);
  }, [refresh, pollMs]);

  return { status, refresh };
}

/** Something still has to reach the sessions folder: a copy running or waiting, or a video being encoded. */
export function mirrorBusy(status: MirrorStatus | null): boolean {
  return Boolean(status && (status.syncing || status.processing_videos > 0));
}

/** Where the user must be connected for the sessions folder to be reachable. */
export const NETWORK_HINT = "Collegati alla rete Univet oppure alla VPN Univet.";

/** What to do when the sessions folder cannot be reached (shown in banners and dialogs). */
export const MIRROR_UNREACHABLE_HELP =
  `${NETWORK_HINT} Le sessioni restano salvate su questo computer e vengono copiate da sole appena la cartella ` +
  "torna raggiungibile. Se il problema continua, apri la sessione, premi \"Esporta Sessione (.flowcapture)\" e " +
  "invia il file esportato al team IT.";

function formatBytes(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(0, Math.round(bytes / 1024))} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

/** Progress of the copy to the sessions folder: session, step, bar, queue, videos still encoding. */
export function MirrorProgress({ status }: { status: MirrorStatus }) {
  const percent =
    status.phase && status.total_bytes > 0
      ? Math.min(100, Math.round((status.done_bytes / status.total_bytes) * 100))
      : null;
  const step =
    status.phase === "files"
      ? "Copia dei file"
      : status.phase === "archive"
        ? "Creazione dell'archivio .flowcapture"
        : status.syncing
          ? "Preparazione"
          : null;
  const others = Math.max(0, status.queued - 1);

  return (
    <div className="mirror-progress">
      {status.current_session ? <div className="mirror-progress-title">"{status.current_session}"</div> : null}
      {step ? (
        <div className="mirror-progress-step">
          {step}
          {percent !== null ? ` · ${percent}% (${formatBytes(status.done_bytes)} di ${formatBytes(status.total_bytes)})` : "…"}
        </div>
      ) : null}
      {step ? (
        <div className={`mirror-progress-bar${percent === null ? " indeterminate" : ""}`}>
          <i style={percent === null ? undefined : { width: `${percent}%` }} />
        </div>
      ) : null}
      {others > 0 ? <div className="mirror-progress-note">Altre {others} sessioni in coda</div> : null}
      {status.processing_videos > 0 ? (
        <div className="mirror-progress-note">
          🎬 Elaborazione del video in corso: verrà salvato nella cartella appena pronto
        </div>
      ) : null}
    </div>
  );
}

/**
 * Warns when sessions could not be copied to the sessions folder (typically a network share with
 * the VPN off). The copy is retried automatically; "Riprova ora" forces it.
 */
export function MirrorStatusBanner() {
  const { status, refresh } = useMirrorStatus();
  const [retrying, setRetrying] = useState(false);

  if (!status || status.pending_sessions === 0) return null;

  async function retry() {
    setRetrying(true);
    try {
      await api.syncSessionMirror();
    } catch {
      // Shown again by the next refresh.
    } finally {
      // The copy runs in the background, a moment after the request.
      window.setTimeout(() => {
        void refresh();
        setRetrying(false);
      }, 5_000);
    }
  }

  const count = status.pending_sessions;
  return (
    <div className="warn-card danger">
      <span>☁️</span>
      <div className="warn-card-body">
        <strong>
          {count === 1 ? "1 sessione non è stata salvata" : `${count} sessioni non sono state salvate`} nella cartella
          sessioni condivisa: la cartella non è raggiungibile.
        </strong>{" "}
        {MIRROR_UNREACHABLE_HELP}
        {status.last_error ? <div style={{ marginTop: 4, fontSize: 11.5, color: "var(--dim)" }}>{status.last_error}</div> : null}
      </div>
      <div className="warn-card-actions">
        <AppButton size="sm" disabled={retrying || status.syncing} onClick={() => void retry()}>
          {retrying || status.syncing ? "Copia in corso…" : "Riprova ora"}
        </AppButton>
      </div>
    </div>
  );
}

/** Panel (bottom left) while sessions are being saved to the sessions folder. */
export function MirrorSyncIndicator() {
  const { status } = useMirrorStatus(1_000);
  if (!status || !mirrorBusy(status)) return null;
  return (
    <div className="mirror-sync-indicator" role="status">
      <div className="mirror-sync-head">
        <span className="pulse" style={{ width: 8, height: 8 }} />
        <strong>Salvataggio nella cartella condivisa</strong>
      </div>
      <MirrorProgress status={status} />
      <div className="mirror-progress-note">Non chiudere FlowCapture fino al termine.</div>
    </div>
  );
}
