import { Icon } from "@/components/ui/Icon";

export type LogPanelStatus = "running" | "success" | "error";

type LogPanelProps = {
  status: LogPanelStatus;
  /** Headline for each status, e.g. { running: "Traduzione in corso…", … }. */
  messages: Record<LogPanelStatus, string>;
  logs: string[];
  onClose: () => void;
};

/** Progress box of a long AI operation (translation, transcription) with its log lines. */
export function LogPanel({ status, messages, logs, onClose }: LogPanelProps) {
  return (
    <div className={`log-panel${status === "running" ? "" : ` ${status}`}`}>
      <div className="log-panel-head">
        <div className="log-panel-status">
          {status === "running" && <span className="log-panel-spinner" />}
          {status === "success" && <Icon name="check" size={15} />}
          {status === "error" && <Icon name="alert" size={15} />}
          <span>{messages[status]}</span>
        </div>
        <button type="button" className="error-card-close log-panel-close" onClick={onClose}>
          Chiudi ✕
        </button>
      </div>
      <div className="log-box">
        {logs.map((log, idx) => (
          <div
            key={idx}
            className={
              log.includes("ERRORE") ? "log-line error" : log.includes("successo") ? "log-line success" : "log-line"
            }
          >
            {log}
          </div>
        ))}
      </div>
    </div>
  );
}
