import type { ReactNode } from "react";

export interface ConfirmModalProps {
  isOpen: boolean;
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  kind?: "danger" | "warning" | "primary";
  onConfirm: () => void | Promise<void>;
  onCancel: () => void;
  isLoading?: boolean;
  /** Only the confirm button: the dialog cannot be dismissed (no cancel, no click outside). */
  hideCancel?: boolean;
  /** Extra content under the message (e.g. a progress bar). */
  children?: ReactNode;
}

export function ConfirmModal({
  isOpen,
  title,
  message,
  confirmLabel = "Conferma",
  cancelLabel = "Annulla",
  kind = "warning",
  onConfirm,
  onCancel,
  isLoading = false,
  hideCancel = false,
  children,
}: ConfirmModalProps) {
  if (!isOpen) return null;

  const isDanger = kind === "danger";
  const accentColor = isDanger ? "#ef4444" : kind === "warning" ? "#f59e0b" : "var(--mint)";

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        backgroundColor: "rgba(0, 0, 0, 0.75)",
        zIndex: 10000,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        backdropFilter: "blur(6px)",
        padding: "16px",
      }}
      onClick={(e) => {
        if (e.target === e.currentTarget && !isLoading && !hideCancel) {
          onCancel();
        }
      }}
    >
      <div
        style={{
          width: "100%",
          maxWidth: "460px",
          background: "var(--surface)",
          border: `1px solid ${isDanger ? "rgba(239, 68, 68, 0.4)" : "var(--hair)"}`,
          borderRadius: "12px",
          boxShadow: "0 20px 50px rgba(0, 0, 0, 0.6)",
          padding: "24px",
          display: "flex",
          flexDirection: "column",
          gap: "16px",
          animation: "fadeIn 0.15s ease-out",
        }}
      >
        <div style={{ display: "flex", alignItems: "flex-start", gap: "14px" }}>
          <div
            style={{
              width: "40px",
              height: "40px",
              borderRadius: "10px",
              background: `${accentColor}1a`,
              border: `1px solid ${accentColor}40`,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: "20px",
              flexShrink: 0,
            }}
          >
            {isDanger ? "🗑️" : kind === "warning" ? "⚠️" : "ℹ️"}
          </div>
          <div>
            <h3 style={{ margin: "0 0 6px 0", fontSize: "16px", color: "var(--text, #f8fafc)", fontWeight: 600 }}>
              {title}
            </h3>
            <p style={{ margin: 0, fontSize: "13.5px", color: "var(--text-2, #94a3b8)", lineHeight: "1.55", whiteSpace: "pre-line" }}>
              {message}
            </p>
            {children}
          </div>
        </div>

        <div style={{ display: "flex", justifyContent: "flex-end", gap: "10px", marginTop: "8px" }}>
          <button
            type="button"
            className="btn btn-ghost"
            onClick={onCancel}
            disabled={isLoading}
            style={{
              display: hideCancel ? "none" : undefined,
              padding: "7px 14px",
              fontSize: "13px",
              borderRadius: "6px",
              background: "rgba(255, 255, 255, 0.05)",
              border: "1px solid var(--hair)",
              color: "var(--text)",
              cursor: "pointer",
            }}
          >
            {cancelLabel}
          </button>
          <button
            type="button"
            onClick={onConfirm}
            disabled={isLoading}
            style={{
              padding: "7px 16px",
              fontSize: "13px",
              fontWeight: 600,
              borderRadius: "6px",
              border: "none",
              background: isDanger ? "#dc2626" : kind === "warning" ? "#d97706" : "var(--mint)",
              color: isDanger || kind === "warning" ? "#ffffff" : "var(--mint-ink)",
              cursor: isLoading ? "wait" : "pointer",
              display: "flex",
              alignItems: "center",
              gap: "6px",
              boxShadow: "0 2px 8px rgba(0,0,0,0.25)",
            }}
          >
            {isLoading && (
              <span
                style={{
                  display: "inline-block",
                  width: "12px",
                  height: "12px",
                  border: "2px solid #fff",
                  borderTopColor: "transparent",
                  borderRadius: "50%",
                  animation: "spin 0.8s linear infinite",
                }}
              />
            )}
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
