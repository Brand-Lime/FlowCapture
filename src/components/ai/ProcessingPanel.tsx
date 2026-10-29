import { AppButton } from "@/components/ui/AppButton";
import { Icon } from "@/components/ui/Icon";

export const STAGES = [
  { id: "timeline_builder", label: "Ricostruzione della sequenza di azioni" },
  { id: "screenshot_selector", label: "Scelta degli screenshot per ogni passo" },
  { id: "technical_writer", label: "Scrittura della guida con l'AI" },
  { id: "quality_reviewer", label: "Revisione finale" },
] as const;

/** Italian label of a pipeline stage id (e.g. "technical_writer"). */
export function stageLabel(stageId: string) {
  if (stageId === "pipeline_queued") return "Avvio";
  if (stageId === "completed") return "Completata";
  return STAGES.find((stage) => stage.id === stageId)?.label ?? stageId.replaceAll("_", " ");
}

type ProcessingPanelProps = {
  activeStage?: string | null;
  completedStages: string[];
  failedStage?: string | null;
  logs?: string[];
  onCancel?: () => void;
};

export function ProcessingPanel({
  activeStage,
  completedStages,
  failedStage,
  logs = [],
  onCancel,
}: ProcessingPanelProps) {
  const activeIndex = STAGES.findIndex((stage) => stage.id === activeStage);
  const completedCount = completedStages.length;
  const step =
    activeIndex >= 0
      ? activeIndex
      : completedCount >= STAGES.length
        ? STAGES.length
        : completedCount;

  return (
    <div className="proc" style={{ maxWidth: "600px", margin: "0 auto" }}>
      <div className="spinner" />
      <h3>Generazione della documentazione</h3>
      <p>FlowCapture sta trasformando la sessione in una guida passo-passo.</p>
      <div className="steps">
        {STAGES.map((stage, index) => {
          const isDone = completedStages.includes(stage.id);
          const isActive = activeStage === stage.id;
          const isFailed = failedStage === stage.id;
          const state = isDone ? "done" : isActive ? "active" : index < step ? "done" : "";

          return (
            <div key={stage.id} className={`ps ${state}${isFailed ? " active" : ""}`}>
              <span className="pc">
                {isDone ? <Icon name="check" size={12} /> : index + 1}
              </span>
              {stage.label}
              {isFailed ? (
                <span style={{ marginLeft: "auto", color: "var(--rose)", fontSize: 12 }}>
                  Non riuscito
                </span>
              ) : null}
            </div>
          );
        })}
      </div>

      {onCancel && (
        <div style={{ marginTop: "16px", display: "flex", justifyContent: "center" }}>
          <AppButton kind="danger" size="sm" icon="x" onClick={onCancel}>
            Annulla generazione
          </AppButton>
        </div>
      )}

      {logs.length > 0 && (
        <div className="log-box proc-log">
          <div className="log-box-title">Avanzamento in tempo reale</div>
          {logs.map((log, i) => (
            <div key={i} className={log.includes("ERRORE") ? "log-line error" : "log-line"}>
              {log}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
