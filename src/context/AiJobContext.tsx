import {
  createContext,
  useContext,
  useState,
  useEffect,
  useCallback,
  type ReactNode,
} from "react";
import { api, type AiProgressEvent, type AiLogEvent, type GenerateDocumentationResult } from "@/lib/api";

export interface ActiveAiJob {
  sessionId: string;
  sessionTitle: string;
  stage: string;
  completedStages: string[];
  failedStage: string | null;
  logs: string[];
  isDone: boolean;
  error: string | null;
  result: GenerateDocumentationResult | null;
}

interface AiJobContextValue {
  activeJob: ActiveAiJob | null;
  startJob: (sessionId: string, sessionTitle: string) => Promise<GenerateDocumentationResult>;
  cancelJob: (sessionId?: string) => Promise<void>;
  clearJob: () => void;
}

const AiJobContext = createContext<AiJobContextValue | null>(null);

export function AiJobProvider({ children }: { children: ReactNode }) {
  const [activeJob, setActiveJob] = useState<ActiveAiJob | null>(null);

  useEffect(() => {
    if (!activeJob || activeJob.isDone || activeJob.error) return;

    // `listen` resolves asynchronously: if the effect is cleaned up first (StrictMode runs it
    // twice), the late unlisten must still run or every log line arrives twice.
    let disposed = false;
    const unlisteners: (() => void)[] = [];
    const keep = (unlisten: () => void) => {
      if (disposed) unlisten();
      else unlisteners.push(unlisten);
    };

    void api.onAiProgress(activeJob.sessionId, (evt: AiProgressEvent) => {
      setActiveJob((prev) => {
        if (!prev || prev.sessionId !== evt.session_id) return prev;
        const newCompleted =
          evt.status === "completed" && !prev.completedStages.includes(evt.stage)
            ? [...prev.completedStages, evt.stage]
            : prev.completedStages;
        const newFailed = evt.status === "failed" ? evt.stage : prev.failedStage;
        return {
          ...prev,
          stage: evt.stage,
          completedStages: newCompleted,
          failedStage: newFailed,
        };
      });
    }).then(keep);

    void api.onAiLog(activeJob.sessionId, (evt: AiLogEvent) => {
      setActiveJob((prev) => {
        if (!prev || prev.sessionId !== evt.session_id) return prev;
        return {
          ...prev,
          logs: [...prev.logs, evt.message],
        };
      });
    }).then(keep);

    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [activeJob?.sessionId, activeJob?.isDone, activeJob?.error]);

  const startJob = useCallback(
    async (sessionId: string, sessionTitle: string) => {
      const initialJob: ActiveAiJob = {
        sessionId,
        sessionTitle,
        stage: "pipeline_queued",
        completedStages: [],
        failedStage: null,
        logs: [`[${new Date().toLocaleTimeString()}] Generazione avviata`],
        isDone: false,
        error: null,
        result: null,
      };
      setActiveJob(initialJob);

      try {
        const result = await api.generateDocumentation(sessionId);
        setActiveJob((prev) =>
          prev && prev.sessionId === sessionId
            ? {
                ...prev,
                stage: "completed",
                isDone: true,
                result,
                logs: [...prev.logs, `[${new Date().toLocaleTimeString()}] Documentazione generata con successo!`],
              }
            : prev
        );
        return result;
      } catch (err) {
        const errMsg = String(err);
        setActiveJob((prev) =>
          prev && prev.sessionId === sessionId
            ? {
                ...prev,
                isDone: true,
                error: errMsg,
                logs: [...prev.logs, `[${new Date().toLocaleTimeString()}] ERRORE: ${errMsg}`],
              }
            : prev
        );
        throw err;
      }
    },
    []
  );

  const cancelJob = useCallback(
    async (sessionId?: string) => {
      const targetId = sessionId || activeJob?.sessionId;
      if (!targetId) return;

      try {
        await api.cancelDocumentation(targetId);
      } catch (err) {
        console.error("Failed to invoke cancelDocumentation:", err);
      }

      setActiveJob((prev) =>
        prev && prev.sessionId === targetId
          ? {
              ...prev,
              isDone: true,
              error: "Generazione annullata dall'utente",
              logs: [
                ...prev.logs,
                `[${new Date().toLocaleTimeString()}] Generazione annullata dall'utente.`,
              ],
            }
          : prev
      );
    },
    [activeJob?.sessionId]
  );

  const clearJob = useCallback(() => {
    setActiveJob(null);
  }, []);

  return (
    <AiJobContext.Provider value={{ activeJob, startJob, cancelJob, clearJob }}>
      {children}
    </AiJobContext.Provider>
  );
}

export function useAiJob() {
  const ctx = useContext(AiJobContext);
  if (!ctx) {
    throw new Error("useAiJob must be used within an AiJobProvider");
  }
  return ctx;
}
