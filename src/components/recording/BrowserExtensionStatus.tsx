import { useCallback, useEffect, useState } from "react";
import { AppButton } from "@/components/ui/AppButton";
import { ConfirmModal } from "@/components/ui/ConfirmModal";
import { api, type BrowserExtensionStatus } from "@/lib/api";

const STATE_LABELS = {
  active: "✅ installata",
  disabled: "⚠️ installata ma disattivata",
  missing: "❌ non installata",
} as const;

/** Recording is refused: the extension is required and missing in a used browser. */
export function extensionBlocksRecording(status: BrowserExtensionStatus | null): boolean {
  return Boolean(status && status.supported && status.required && status.missing.length > 0);
}

/** Text for dialogs: which browsers need the extension and what to do. */
export function describeMissingExtension(status: BrowserExtensionStatus): string {
  const lines = status.browsers
    .filter((browser) => browser.used && browser.extension !== "active")
    .map((browser) => `• ${browser.name}: ${STATE_LABELS[browser.extension].replace(/^\S+ /, "")}`);
  return (
    `Per registrare serve l'estensione "FlowCapture Companion" in tutti i browser che usi:\n${lines.join("\n")}\n\n` +
    "Senza l'estensione FlowCapture non riconosce su cosa fai clic nelle pagine web. " +
    "Contatta il team IT per farla installare, poi premi \"Ricontrolla\"."
  );
}

/**
 * Extension status of the browsers on this computer. Checked again when the window gets the focus
 * back (e.g. after installing the extension in a browser).
 */
export function useBrowserExtensionStatus() {
  const [status, setStatus] = useState<BrowserExtensionStatus | null>(null);
  const [checking, setChecking] = useState(false);

  const refresh = useCallback(async () => {
    setChecking(true);
    try {
      const next = await api.getBrowserExtensionStatus();
      setStatus(next);
      return next;
    } catch {
      return null;
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  return { status, checking, refresh };
}

export function BrowserExtensionList({ status }: { status: BrowserExtensionStatus }) {
  if (status.browsers.length === 0) {
    return <div className="pd">Nessun browser supportato trovato su questo computer.</div>;
  }
  return (
    <ul className="ext-list">
      {status.browsers.map((browser) => (
        <li key={browser.id}>
          <span className="ext-name">{browser.name}</span>
          <span className={`ext-state ${browser.used ? browser.extension : "unused"}`}>
            {browser.used ? STATE_LABELS[browser.extension] : "mai aperto (non controllato)"}
          </span>
        </li>
      ))}
    </ul>
  );
}

/** Home warning while recording is blocked by a missing extension. */
export function BrowserExtensionWarning({
  status,
  checking,
  onRecheck,
}: {
  status: BrowserExtensionStatus | null;
  checking: boolean;
  onRecheck: () => void;
}) {
  if (!status || !extensionBlocksRecording(status)) return null;
  return (
    <div className="warn-card danger">
      <span>🧩</span>
      <div className="warn-card-body">
        <strong>Registrazione bloccata: manca l'estensione del browser.</strong> Senza l'estensione "FlowCapture
        Companion" FlowCapture non riconosce su cosa fai clic nelle pagine web. Contatta il team IT per farla
        installare, poi premi "Ricontrolla".
        <BrowserExtensionList status={status} />
      </div>
      <div className="warn-card-actions">
        <AppButton size="sm" disabled={checking} onClick={onRecheck}>
          {checking ? "Controllo…" : "Ricontrolla"}
        </AppButton>
      </div>
    </div>
  );
}

const NOTICE_SETTING = "browser_extension_notice_shown";

/**
 * Where the extension cannot be checked automatically (macOS, Linux), tells the user once, at the
 * first start, that it is needed.
 */
export function BrowserExtensionFirstRunNotice() {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    Promise.all([api.getBrowserExtensionStatus(), api.getSetting(NOTICE_SETTING)])
      .then(([status, shown]) => {
        if (!status.supported && shown !== "true") setOpen(true);
      })
      .catch(() => undefined);
  }, []);

  function close() {
    setOpen(false);
    void api.setSetting(NOTICE_SETTING, "true").catch(() => undefined);
  }

  return (
    <ConfirmModal
      isOpen={open}
      title="Serve l'estensione del browser"
      message={
        "Per registrare correttamente le attività nelle pagine web, nel browser che usi deve essere installata " +
        "l'estensione \"FlowCapture Companion\".\n\nSe non ce l'hai, contatta il team IT prima di registrare."
      }
      confirmLabel="Ho capito"
      cancelLabel="Chiudi"
      kind="primary"
      onConfirm={close}
      onCancel={close}
    />
  );
}
