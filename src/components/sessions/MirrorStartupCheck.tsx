import { useCallback, useEffect, useRef, useState } from "react";
import { ConfirmModal } from "@/components/ui/ConfirmModal";
import { NETWORK_HINT } from "@/components/sessions/MirrorStatusBanner";
import { api } from "@/lib/api";

/** How often the folder is checked again while the dialog is open (the user is connecting). */
const AUTO_RETRY_MS = 5_000;

/**
 * At startup, checks that the sessions folder (a network share) can be reached. If not, a dialog
 * that cannot be dismissed asks to connect to the Univet network or VPN; it checks again by itself
 * every few seconds (and with "Riprova") and closes as soon as the folder is reachable.
 */
export function MirrorStartupCheck() {
  const [unreachablePath, setUnreachablePath] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [attempts, setAttempts] = useState(0);
  const checkingRef = useRef(false);

  const check = useCallback(async () => {
    if (checkingRef.current) return;
    checkingRef.current = true;
    setChecking(true);
    try {
      const result = await api.checkSessionMirrorDir();
      if (!result.configured || result.reachable) {
        setUnreachablePath((previous) => {
          // Reachable again: copy what could not be copied while offline.
          if (previous !== null && result.configured) void api.syncSessionMirror().catch(() => undefined);
          return null;
        });
      } else {
        setUnreachablePath(result.path ?? "");
        setAttempts((count) => count + 1);
      }
    } catch {
      // Checked again at the next attempt.
    } finally {
      checkingRef.current = false;
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    void check();
  }, [check]);

  useEffect(() => {
    if (unreachablePath === null) return;
    const timer = window.setInterval(() => void check(), AUTO_RETRY_MS);
    return () => window.clearInterval(timer);
  }, [unreachablePath, check]);

  return (
    <ConfirmModal
      isOpen={unreachablePath !== null}
      title="Cartella condivisa non raggiungibile"
      message={
        `Non riesco a raggiungere la cartella dove vengono salvate le sessioni:\n${unreachablePath ?? ""}\n\n` +
        `${NETWORK_HINT} Questo messaggio si chiude da solo appena la cartella è raggiungibile ` +
        `(controllo automatico ogni ${AUTO_RETRY_MS / 1000} secondi).\n\n` +
        "Se il problema continua, contatta il team IT."
      }
      confirmLabel={checking ? "Controllo in corso…" : "Riprova"}
      kind="warning"
      isLoading={checking}
      hideCancel
      onConfirm={() => void check()}
      onCancel={() => undefined}
    >
      {attempts > 1 ? (
        <div className="mirror-progress-note" style={{ marginTop: 10 }}>
          Tentativi: {attempts} · la cartella non è ancora raggiungibile.
        </div>
      ) : null}
    </ConfirmModal>
  );
}
