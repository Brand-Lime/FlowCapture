import { useEffect, useState } from "react";
import { AppButton } from "@/components/ui/AppButton";
import { Icon } from "@/components/ui/Icon";
import { api, type RecordingPermissions } from "@/lib/api";

type PermissionsBannerProps = {
  onPermissionsChange?: (permissions: RecordingPermissions) => void;
};

export function PermissionsBanner({ onPermissionsChange }: PermissionsBannerProps) {
  const [permissions, setPermissions] = useState<RecordingPermissions | null>(null);
  const [loading, setLoading] = useState(false);

  async function refresh() {
    const next = await api.getRecordingPermissions();
    setPermissions(next);
    onPermissionsChange?.(next);
  }

  async function prepare() {
    setLoading(true);
    try {
      const next = await api.prepareRecordingPermissions();
      setPermissions(next);
      onPermissionsChange?.(next);
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh().catch(() => undefined);
  }, []);

  if (!permissions) {
    return null;
  }

  if (permissions.can_record) {
    return (
      <div className="banner">
        <Icon name="shield" />
        <span className="bt">
          Permessi di registrazione a posto: puoi registrare il lavoro in tutte le applicazioni.
        </span>
      </div>
    );
  }

  return (
    <div
      className="card set-card"
      style={{ marginTop: 18, borderColor: "rgba(255,138,138,.35)" }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <Icon name="alert" style={{ color: "var(--rose)" }} />
        <h3 style={{ color: "var(--rose)" }}>Servono i permessi di registrazione</h3>
      </div>
      <div className="sub">{permissions.message}</div>
      <div className="sub" style={{ marginTop: 14 }}>
        Registrazione schermo: {permissions.screen_recording_granted ? "concesso" : "mancante"}
        <br />
        Accessibilità: {permissions.accessibility_granted ? "concesso" : "mancante"}
        <br />
        Nelle impostazioni di sistema cerca:{" "}
        {permissions.settings_app_names.map((name) => `\`${name}\``).join(", ")}
      </div>
      {permissions.is_dev_mode ? (
        <div
          className="sub"
          style={{
            marginTop: 12,
            padding: "12px 14px",
            border: "1px solid var(--hair)",
            borderRadius: "var(--r-sm)",
          }}
        >
          In modalità sviluppo viene usato l'eseguibile `{permissions.process_name}`, non l'app
          FlowCapture. Se non è nell'elenco, premi "Richiedi permessi" con FlowCapture in primo piano,
          poi usa "Mostra l'app nel Finder" e il pulsante + per aggiungerlo a mano.
        </div>
      ) : null}
      <div className="sub mono" style={{ marginTop: 10, wordBreak: "break-all" }}>
        {permissions.executable_path}
      </div>
      <ul style={{ margin: "14px 0 0", paddingLeft: 20, color: "var(--muted)", fontSize: 14 }}>
        {permissions.help_steps.map((step) => (
          <li key={step} style={{ marginBottom: 8 }}>
            {step}
          </li>
        ))}
      </ul>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 10, marginTop: 18 }}>
        <AppButton kind="primary" disabled={loading} onClick={() => prepare()}>
          Richiedi permessi
        </AppButton>
        <AppButton
          disabled={loading}
          onClick={async () => {
            setLoading(true);
            try {
              await api.revealExecutableInFinder();
            } finally {
              setLoading(false);
            }
          }}
        >
          Mostra l'app nel Finder
        </AppButton>
        <AppButton
          disabled={loading}
          onClick={async () => {
            setLoading(true);
            try {
              await api.prepareRecordingPermissions();
              await api.openScreenRecordingSettings();
            } finally {
              setLoading(false);
            }
          }}
        >
          Apri impostazioni registrazione schermo
        </AppButton>
        <AppButton
          disabled={loading}
          onClick={async () => {
            setLoading(true);
            try {
              await api.requestAccessibilityPermission();
              await api.openAccessibilitySettings();
            } finally {
              setLoading(false);
            }
          }}
        >
          Apri impostazioni accessibilità
        </AppButton>
        <AppButton disabled={loading} onClick={() => refresh()}>
          Ricontrolla
        </AppButton>
      </div>
    </div>
  );
}
