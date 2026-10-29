import { useCallback, useEffect, useMemo, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { AppButton } from "@/components/ui/AppButton";
import { ImageLightbox } from "@/components/ui/ImageLightbox";
import type { Screenshot } from "@/lib/api";

interface ScreenshotPickerModalProps {
  currentScreenshotId?: string;
  screenshots: Screenshot[];
  /** Step number shown in the title; omit when picking an image for free-form Markdown. */
  stepIndex?: number;
  title?: string;
  confirmLabel?: string;
  onSelect: (screenshotId: string) => Promise<void> | void;
  onClose: () => void;
}

export function ScreenshotPickerModal({
  currentScreenshotId,
  screenshots,
  stepIndex,
  title,
  confirmLabel,
  onSelect,
  onClose,
}: ScreenshotPickerModalProps) {
  const [selectedId, setSelectedId] = useState<string>(currentScreenshotId || (screenshots[0]?.id ?? ""));
  const [previewId, setPreviewId] = useState<string>(selectedId);
  const [saving, setSaving] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);

  const previewIndex = useMemo(
    () => screenshots.findIndex((s) => s.id === previewId),
    [screenshots, previewId],
  );
  const previewScreenshot = previewIndex >= 0 ? screenshots[previewIndex] : undefined;

  const handleConfirm = async () => {
    if (!selectedId) return;
    setSaving(true);
    try {
      await onSelect(selectedId);
      onClose();
    } finally {
      setSaving(false);
    }
  };

  const move = useCallback(
    (delta: number) => {
      if (screenshots.length === 0) return;
      const base = previewIndex >= 0 ? previewIndex : 0;
      const next = screenshots[Math.max(0, Math.min(screenshots.length - 1, base + delta))];
      setPreviewId(next.id);
      setSelectedId(next.id);
    },
    [screenshots, previewIndex],
  );

  useEffect(() => {
    if (fullscreen) return;
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
        return;
      }
      // On a focused button or field, Enter/arrows keep their own meaning: Enter on "Annulla"
      // must cancel, not apply the selection as well.
      const target = event.target as HTMLElement | null;
      if (target?.closest("button, input, textarea, select, [contenteditable='true']")) return;
      if (event.key === "ArrowLeft" || event.key === "ArrowUp") move(-1);
      else if (event.key === "ArrowRight" || event.key === "ArrowDown") move(1);
      else if (event.key === "Enter" && selectedId && !saving) void handleConfirm();
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const heading =
    title ?? (stepIndex != null ? `Scegli lo screenshot per il passo ${stepIndex}` : "Scegli uno screenshot");

  return (
    <div
      className="fc-modal-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="fc-modal picker-modal">
        <div className="fc-modal-head">
          <div>
            <h3>{heading}</h3>
            <div className="fc-modal-sub">
              Seleziona un'immagine tra gli screenshot acquisiti. Doppio clic o tasto destro per vederla a
              schermo intero · frecce per scorrere · Invio per applicare.
            </div>
          </div>
          <div className="fc-modal-actions">
            <AppButton size="sm" kind="ghost" onClick={onClose}>
              Annulla
            </AppButton>
            <AppButton size="sm" kind="primary" disabled={!selectedId || saving} onClick={handleConfirm}>
              {saving ? "Aggiornamento…" : confirmLabel ?? (stepIndex != null ? "Applica al passo" : "Usa questa immagine")}
            </AppButton>
          </div>
        </div>

        <div className="picker-body">
          <div className="picker-grid">
            {screenshots.map((s, idx) => {
              const isCurrent = s.id === currentScreenshotId;
              const isSelected = s.id === selectedId;
              return (
                <div
                  key={s.id}
                  className={`picker-thumb${isSelected ? " selected" : ""}${isCurrent ? " current" : ""}`}
                  onClick={() => {
                    setSelectedId(s.id);
                    setPreviewId(s.id);
                  }}
                  onDoubleClick={() => {
                    setPreviewId(s.id);
                    setFullscreen(true);
                  }}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    setPreviewId(s.id);
                    setFullscreen(true);
                  }}
                  onMouseEnter={() => setPreviewId(s.id)}
                >
                  <div className="picker-thumb-img">
                    <img src={convertFileSrc(s.path)} alt={`Screenshot ${idx + 1}`} loading="lazy" />
                    {isCurrent ? <div className="picker-tag current">Attuale</div> : null}
                    {isSelected ? <div className="picker-tag selected">✓ Selezionato</div> : null}
                  </div>
                  <div className="picker-thumb-cap">
                    <span>#{idx + 1}</span>
                    <span>{s.trigger || "click"}</span>
                  </div>
                </div>
              );
            })}
          </div>

          <div className="picker-preview">
            <div className="picker-preview-head">
              <span>
                Anteprima {previewScreenshot ? `· #${previewIndex + 1} ${previewScreenshot.trigger ?? ""}` : ""}
              </span>
              {previewScreenshot ? (
                <button type="button" className="fc-chip-btn" onClick={() => setFullscreen(true)}>
                  ⛶ Schermo intero
                </button>
              ) : null}
            </div>
            {previewScreenshot ? (
              <div
                className="picker-preview-stage"
                onDoubleClick={() => setFullscreen(true)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  setFullscreen(true);
                }}
                title="Doppio clic o tasto destro per ingrandire a schermo intero"
              >
                <img src={convertFileSrc(previewScreenshot.path)} alt="Anteprima" />
              </div>
            ) : (
              <div className="picker-preview-empty">Nessuna immagine selezionata</div>
            )}
          </div>
        </div>
      </div>

      {fullscreen && previewScreenshot ? (
        <ImageLightbox
          src={convertFileSrc(previewScreenshot.path)}
          alt={`Screenshot #${previewIndex + 1}`}
          caption={`Screenshot #${previewIndex + 1} · ${previewScreenshot.trigger ?? ""}`}
          onClose={() => setFullscreen(false)}
          onPrev={previewIndex > 0 ? () => move(-1) : undefined}
          onNext={previewIndex < screenshots.length - 1 ? () => move(1) : undefined}
        />
      ) : null}
    </div>
  );
}
