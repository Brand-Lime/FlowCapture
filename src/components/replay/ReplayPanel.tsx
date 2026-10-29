import { useCallback, useState, type Dispatch, type SetStateAction } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { MarkdownEditor } from "@/components/documentation/MarkdownEditor";
import { MarkdownPreview } from "@/components/documentation/MarkdownPreview";
import { AnnotationOverlay } from "@/components/sessions/AnnotationOverlay";
import { ImageAnnotationModal } from "@/components/sessions/ImageAnnotationModal";
import { ScreenshotPickerModal } from "@/components/sessions/ScreenshotPickerModal";
import { AppButton } from "@/components/ui/AppButton";
import { ContextMenu } from "@/components/ui/ContextMenu";
import { ImageLightbox } from "@/components/ui/ImageLightbox";
import { useReplayEditor } from "@/hooks/useReplayEditor";
import type { Screenshot, WorkflowStep } from "@/lib/api";

type ReplayPanelProps = {
  sessionId: string;
  hasDocumentation: boolean;
  steps: WorkflowStep[];
  setSteps: Dispatch<SetStateAction<WorkflowStep[]>>;
  screenshots: Screenshot[];
  refresh: (options?: { syncMarkdown?: boolean }) => Promise<void>;
  onToast: (message: string) => void;
  onError: (message: string) => void;
};

export function ReplayPanel({
  sessionId,
  hasDocumentation,
  steps,
  setSteps,
  screenshots,
  refresh,
  onToast,
  onError,
}: ReplayPanelProps) {
  const replay = useReplayEditor({ sessionId, steps, setSteps, screenshots, refresh, onToast, onError });
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  const { step, screenshot } = replay;

  return (
    <div className="card panel">
      <div className="rp-head">
        <div>
          <h3>Passi della guida</h3>
          <div className="pd">
            Scorri e modifica i passi generati uno alla volta, con il testo formattato e le annotazioni.
          </div>
        </div>
        {steps.length === 0 && hasDocumentation ? (
          <AppButton size="sm" kind="primary" icon="sparkles" onClick={replay.syncStepsFromDoc} disabled={replay.syncing}>
            Recupera i passi dalla guida
          </AppButton>
        ) : null}
      </div>

      {steps.length === 0 || !step ? (
        <div className="rp-empty">
          <div className="rp-empty-msg">Nessun passo trovato per questa sessione.</div>
          {hasDocumentation ? (
            <AppButton size="sm" kind="primary" onClick={replay.syncStepsFromDoc} disabled={replay.syncing}>
              Recupera i passi dalla guida
            </AppButton>
          ) : (
            <div className="rp-empty-sub">Genera la documentazione per vedere qui i passi.</div>
          )}
        </div>
      ) : (
        <>
          <div className="rp-progress" style={{ marginTop: 18 }}>
            <i style={{ width: `${((replay.index + 1) / steps.length) * 100}%` }} />
          </div>
          <div className="rp-step">
            <div className="rp-step-top">
              <div className="rpn">
                Passo {replay.index + 1} di {steps.length}
              </div>
              {!replay.editing ? (
                <button type="button" className="rp-link-btn" onClick={replay.startEditing}>
                  ✏️ Modifica testo del passo
                </button>
              ) : (
                <div className="rp-shot-tools">
                  <button type="button" className="rp-link-btn muted" onClick={replay.cancelEditing}>
                    Annulla
                  </button>
                  <button type="button" className="rp-save-btn" onClick={replay.saveEditing} disabled={replay.saving}>
                    {replay.saving ? "Salvataggio…" : "Salva"}
                  </button>
                </div>
              )}
            </div>

            {replay.editing ? (
              <div className="rp-edit">
                <div>
                  <div className="rp-edit-label">Titolo del passo:</div>
                  <input
                    type="text"
                    className="rp-edit-input"
                    value={replay.editTitle}
                    onChange={(e) => replay.setEditTitle(e.target.value)}
                    placeholder="Titolo del passo…"
                  />
                </div>
                <div>
                  <div className="rp-edit-label">Descrizione:</div>
                  <MarkdownEditor
                    value={replay.editDesc}
                    onChange={replay.setEditDesc}
                    screenshots={screenshots}
                    minHeight="280px"
                    maxHeight="520px"
                    compact={true}
                    hideStepTemplate={true}
                    showSaveButton={true}
                    saving={replay.saving}
                    onSave={replay.saveEditing}
                    onCancel={replay.cancelEditing}
                  />
                </div>
              </div>
            ) : (
              <>
                <h3>{step.title}</h3>
                <div className="rpd rp-desc">
                  <MarkdownPreview
                    markdown={step.description}
                    screenshots={screenshots}
                    editable={true}
                    onTextChange={replay.applyInlineTextChange}
                  />
                </div>
              </>
            )}

            <div
              className="rp-shot rp-shot-live"
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY });
              }}
              onDoubleClick={() => screenshot && replay.openFullscreen()}
              title="Doppio clic per lo schermo intero · tasto destro per altre opzioni"
            >
              {screenshot ? (
                <>
                  <img
                    className="rp-shot-img"
                    src={convertFileSrc(screenshot.path)}
                    alt={step.title}
                    onLoad={(e) => replay.onImageLoad(e.currentTarget)}
                    onError={(event) => {
                      (event.target as HTMLImageElement).style.display = "none";
                    }}
                  />
                  <AnnotationOverlay
                    items={replay.annotations}
                    naturalWidth={replay.naturalDims.w}
                    naturalHeight={replay.naturalDims.h}
                    onAnnotationClick={(item) => replay.openAnnotations(item.id)}
                    onOpenEditor={() => replay.openAnnotations(null)}
                    onChangeScreenshot={replay.openScreenshotPicker}
                    onFullscreen={replay.openFullscreen}
                  />
                </>
              ) : (
                <div className="rwin" />
              )}
            </div>

            <div className="rp-shot-footer">
              <div className="fc-hint">
                💡 Clicca su titoli e paragrafi per modificarli direttamente · doppio clic sull'immagine per lo schermo
                intero · tasto destro per cambiarla o annotarla · ←/→ per scorrere i passi.
              </div>
              <div className="rp-shot-tools">
                {screenshot ? (
                  <button
                    type="button"
                    className="btn btn-ghost btn-sm"
                    onClick={replay.openFullscreen}
                    title="Ingrandisci lo screenshot a schermo intero"
                  >
                    ⛶ Schermo intero
                  </button>
                ) : null}
                <button
                  type="button"
                  className="btn btn-ghost btn-sm"
                  onClick={replay.openScreenshotPicker}
                  title="Seleziona un altro screenshot tra quelli acquisiti"
                >
                  🖼️ Cambia immagine
                </button>
                {screenshot ? (
                  <button
                    type="button"
                    className="btn btn-ghost btn-sm"
                    onClick={() => replay.openAnnotations(null)}
                    title="Modifica questo screenshot, sposta click o aggiungi annotazioni grafiche"
                  >
                    🎨 Annota
                  </button>
                ) : null}
              </div>
            </div>

            <div className="rp-nav">
              <AppButton size="sm" disabled={!replay.hasPrev} onClick={replay.goPrev}>
                Passo precedente
              </AppButton>
              <AppButton size="sm" kind="primary" disabled={!replay.hasNext} onClick={replay.goNext}>
                Passo successivo
              </AppButton>
              <span className="rp-counter">
                {replay.index + 1} / {steps.length}
              </span>
            </div>
          </div>
        </>
      )}

      {menu && step ? (
        <ContextMenu
          position={menu}
          onClose={closeMenu}
          items={[
            { label: "Ingrandisci a schermo intero", icon: "⛶", onSelect: replay.openFullscreen, disabled: !screenshot },
            { label: "Cambia immagine…", icon: "🖼️", onSelect: replay.openScreenshotPicker },
            {
              label: "Annota / evidenzia…",
              icon: "🎨",
              onSelect: () => replay.openAnnotations(null),
              disabled: !screenshot,
            },
          ]}
        />
      ) : null}

      {replay.fullscreen && screenshot && step ? (
        <ImageLightbox
          src={convertFileSrc(screenshot.path)}
          alt={step.title}
          caption={`Passo ${replay.index + 1} di ${steps.length} · ${step.title}`}
          overlay={
            <AnnotationOverlay
              items={replay.annotations}
              naturalWidth={replay.naturalDims.w}
              naturalHeight={replay.naturalDims.h}
              readOnly
            />
          }
          onClose={replay.closeFullscreen}
          onPrev={replay.hasPrev ? replay.goPrev : undefined}
          onNext={replay.hasNext ? replay.goNext : undefined}
        />
      ) : null}

      {replay.pickingScreenshot && step ? (
        <ScreenshotPickerModal
          stepIndex={step.step}
          currentScreenshotId={screenshot?.id}
          screenshots={screenshots}
          onSelect={replay.changeScreenshot}
          onClose={replay.closeScreenshotPicker}
        />
      ) : null}

      {replay.annotating && screenshot && step ? (
        <ImageAnnotationModal
          sessionId={sessionId}
          screenshot={screenshot}
          stepIndex={step.step}
          stepAnnotationsJson={step.annotations_json || null}
          initialSelectedId={replay.selectedAnnotationId}
          onClose={replay.closeAnnotations}
          onSaved={replay.onAnnotationsSaved}
        />
      ) : null}
    </div>
  );
}
