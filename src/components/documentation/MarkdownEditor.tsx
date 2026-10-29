import { useCallback, useEffect, useRef, useState } from "react";
import type { Screenshot } from "@/lib/api";
import { insertStepAt, renumberSteps } from "@/lib/markdownSteps";
import { MarkdownPreview, type PreviewImageActions } from "./MarkdownPreview";
import { useLanguage } from "@/i18n";

interface MarkdownEditorProps {
  value: string;
  onChange: (value: string) => void;
  screenshots: Screenshot[];
  disabled?: boolean;
  minHeight?: string;
  maxHeight?: string;
  hideStepTemplate?: boolean;
  compact?: boolean;
  showSaveButton?: boolean;
  saving?: boolean;
  onSave?: () => void;
  onCancel?: () => void;
  /** Right-click / hover actions on images in the live preview. */
  imageActions?: PreviewImageActions;
  imageVersion?: number;
}

export function MarkdownEditor({
  value,
  onChange,
  screenshots,
  disabled = false,
  minHeight = "580px",
  maxHeight = "850px",
  hideStepTemplate = false,
  compact = false,
  showSaveButton = false,
  saving = false,
  onSave,
  onCancel,
  imageActions,
  imageVersion,
}: MarkdownEditorProps) {
  const { t } = useLanguage();
  const [viewMode, setViewMode] = useState<"split" | "edit" | "preview">("split");
  const [zoom, setZoom] = useState<number>(100);
  const [editablePreview, setEditablePreview] = useState<boolean>(true);

  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const previewContainerRef = useRef<HTMLDivElement | null>(null);
  const mirrorRef = useRef<HTMLDivElement | null>(null);
  const isSyncingScroll = useRef<boolean>(false);
  // Until the user places the cursor, new content goes to the end (after the last step).
  const lastCursor = useRef<number>(Number.MAX_SAFE_INTEGER);
  const [textareaWidth, setTextareaWidth] = useState<number>(0);

  const [showScreenshotPicker, setShowScreenshotPicker] = useState(false);

  // Keep mirror div width aligned with textarea clientWidth
  useEffect(() => {
    if (!textareaRef.current) return;
    const updateWidth = () => {
      if (textareaRef.current) {
        setTextareaWidth(textareaRef.current.clientWidth);
      }
    };
    updateWidth();
    const ro = new ResizeObserver(updateWidth);
    ro.observe(textareaRef.current);
    return () => ro.disconnect();
  }, [viewMode]);

  // Non-linear, element-aware landmark mapping between Editor and Preview
  const getLandmarks = useCallback(() => {
    const textarea = textareaRef.current;
    const preview = previewContainerRef.current;
    const mirror = mirrorRef.current;
    if (!textarea || !preview) return [];

    const maxScrollEditor = Math.max(0, textarea.scrollHeight - textarea.clientHeight);
    const maxScrollPreview = Math.max(0, preview.scrollHeight - preview.clientHeight);

    const previewRect = preview.getBoundingClientRect();
    const previewScrollTop = preview.scrollTop;

    const elements = preview.querySelectorAll<HTMLElement>("[data-source-line]");
    const rawLandmarks: { line: number; editorY: number; previewY: number }[] = [];

    // Top anchor
    rawLandmarks.push({ line: 1, editorY: 0, previewY: 0 });

    elements.forEach((el) => {
      const lineAttr = el.getAttribute("data-source-line");
      if (!lineAttr) return;
      const line = parseInt(lineAttr, 10);
      if (isNaN(line) || line < 1) return;

      const elRect = el.getBoundingClientRect();
      const previewY = elRect.top - previewRect.top + previewScrollTop;

      let editorY = 0;
      if (mirror && mirror.children[line - 1]) {
        const lineNode = mirror.children[line - 1] as HTMLElement;
        editorY = lineNode.offsetTop;
      } else {
        editorY = Math.max(0, (line - 1) * 21.6);
      }

      rawLandmarks.push({ line, editorY, previewY });
    });

    // Sort by editorY ascending
    rawLandmarks.sort((a, b) => a.editorY - b.editorY);

    // Filter out duplicates and non-increasing entries to keep monotonic progression
    const landmarks: { line: number; editorY: number; previewY: number }[] = [];
    for (const lm of rawLandmarks) {
      if (
        landmarks.length === 0 ||
        (lm.editorY > landmarks[landmarks.length - 1].editorY &&
          lm.previewY >= landmarks[landmarks.length - 1].previewY)
      ) {
        landmarks.push(lm);
      }
    }

    // Bottom anchor
    landmarks.push({
      line: 999999,
      editorY: maxScrollEditor,
      previewY: maxScrollPreview,
    });

    return landmarks;
  }, []);

  // Synchronized scroll from Editor to Preview taking into account different heights and images
  function handleEditorScroll() {
    if (isSyncingScroll.current || viewMode !== "split") return;
    const textarea = textareaRef.current;
    const preview = previewContainerRef.current;
    if (!textarea || !preview) return;

    const maxScrollEditor = textarea.scrollHeight - textarea.clientHeight;
    const maxScrollPreview = preview.scrollHeight - preview.clientHeight;
    if (maxScrollEditor <= 0 || maxScrollPreview <= 0) return;

    isSyncingScroll.current = true;

    const scrollTop = textarea.scrollTop;
    if (scrollTop <= 3) {
      preview.scrollTop = 0;
    } else if (scrollTop >= maxScrollEditor - 3) {
      preview.scrollTop = maxScrollPreview;
    } else {
      const landmarks = getLandmarks();
      if (landmarks.length < 2) {
        const scrollPercentage = scrollTop / maxScrollEditor;
        preview.scrollTop = scrollPercentage * maxScrollPreview;
      } else {
        let i = 0;
        while (i < landmarks.length - 1 && landmarks[i + 1].editorY <= scrollTop) {
          i++;
        }
        const l1 = landmarks[i];
        const l2 = landmarks[Math.min(i + 1, landmarks.length - 1)];
        const editorSpan = l2.editorY - l1.editorY;
        const previewSpan = l2.previewY - l1.previewY;
        const ratio = editorSpan > 0 ? (scrollTop - l1.editorY) / editorSpan : 0;
        const targetPreviewScroll = l1.previewY + ratio * previewSpan;
        preview.scrollTop = Math.max(0, Math.min(maxScrollPreview, targetPreviewScroll));
      }
    }

    requestAnimationFrame(() => {
      isSyncingScroll.current = false;
    });
  }

  // Synchronized scroll from Preview to Editor taking into account different heights and images
  function handlePreviewScroll() {
    if (isSyncingScroll.current || viewMode !== "split") return;
    const textarea = textareaRef.current;
    const preview = previewContainerRef.current;
    if (!textarea || !preview) return;

    const maxScrollEditor = textarea.scrollHeight - textarea.clientHeight;
    const maxScrollPreview = preview.scrollHeight - preview.clientHeight;
    if (maxScrollEditor <= 0 || maxScrollPreview <= 0) return;

    isSyncingScroll.current = true;

    const scrollTop = preview.scrollTop;
    if (scrollTop <= 3) {
      textarea.scrollTop = 0;
    } else if (scrollTop >= maxScrollPreview - 3) {
      textarea.scrollTop = maxScrollEditor;
    } else {
      const landmarks = getLandmarks();
      if (landmarks.length < 2) {
        const scrollPercentage = scrollTop / maxScrollPreview;
        textarea.scrollTop = scrollPercentage * maxScrollEditor;
      } else {
        let i = 0;
        while (i < landmarks.length - 1 && landmarks[i + 1].previewY <= scrollTop) {
          i++;
        }
        const l1 = landmarks[i];
        const l2 = landmarks[Math.min(i + 1, landmarks.length - 1)];
        const previewSpan = l2.previewY - l1.previewY;
        const editorSpan = l2.editorY - l1.editorY;
        const ratio = previewSpan > 0 ? (scrollTop - l1.previewY) / previewSpan : 0;
        const targetEditorScroll = l1.editorY + ratio * editorSpan;
        textarea.scrollTop = Math.max(0, Math.min(maxScrollEditor, targetEditorScroll));
      }
    }

    requestAnimationFrame(() => {
      isSyncingScroll.current = false;
    });
  }

  function handlePreviewTextChange(oldText: string, newText: string) {
    if (!oldText || !newText || oldText === newText) return;
    if (value.includes(oldText)) {
      onChange(value.replace(oldText, newText));
    } else if (value.includes(oldText.trim())) {
      onChange(value.replace(oldText.trim(), newText.trim()));
    } else {
      // Find matching line by stripping common markdown prefixes
      const lines = value.split("\n");
      const trimmedOld = oldText.trim();
      let matchedIdx = -1;
      for (let i = 0; i < lines.length; i++) {
        const stripped = lines[i].replace(/^(\s*#+\s*|\s*[-*+]\s*|\s*\d+\.\s*|\s*>\s*)/, "").trim();
        if (stripped === trimmedOld || lines[i].includes(trimmedOld)) {
          matchedIdx = i;
          break;
        }
      }
      if (matchedIdx !== -1) {
        const line = lines[matchedIdx];
        const matchPrefix = line.match(/^(\s*#+\s*|\s*[-*+]\s*|\s*\d+\.\s*|\s*>\s*)/);
        const prefix = matchPrefix ? matchPrefix[0] : "";
        lines[matchedIdx] = `${prefix}${newText.trim()}`;
        onChange(lines.join("\n"));
      }
    }
  }

  function currentCursor() {
    const textarea = textareaRef.current;
    // In "preview only" mode the textarea is unmounted: fall back to the last known cursor.
    return textarea ? textarea.selectionStart : lastCursor.current;
  }

  function focusSelection(start: number, end: number) {
    setTimeout(() => {
      const textarea = textareaRef.current;
      if (!textarea) return;
      textarea.focus();
      textarea.setSelectionRange(start, end);
    }, 10);
  }

  function insertFormatting(prefix: string, suffix = "", defaultText = "") {
    const textarea = textareaRef.current;
    const fallback = Math.min(lastCursor.current, value.length);
    const start = textarea ? textarea.selectionStart : fallback;
    const end = textarea ? textarea.selectionEnd : fallback;
    const selectedText = value.substring(start, end) || defaultText;
    const replacement = `${prefix}${selectedText}${suffix}`;

    onChange(value.substring(0, start) + replacement + value.substring(end));

    const newCursor = start + prefix.length + selectedText.length;
    lastCursor.current = newCursor;
    focusSelection(selectedText ? start + prefix.length : newCursor, newCursor);
  }

  function insertLinePrefix(prefix: string) {
    const start = Math.min(currentCursor(), value.length);
    const lineStart = value.lastIndexOf("\n", start - 1) + 1;
    onChange(value.substring(0, lineStart) + prefix + value.substring(lineStart));
    lastCursor.current = start + prefix.length;
    focusSelection(start + prefix.length, start + prefix.length);
  }

  function insertScreenshotMarkdown(shot: Screenshot, index: number) {
    const filename = shot.path.split(/[/\\]/).pop() ?? `screenshot_${index + 1}.jpg`;
    const snippet = `\n\n![${t("md.step", "Step")} ${index + 1} - ${shot.trigger || "Screenshot"}](${filename})\n`;
    insertFormatting(snippet, "", "");
    setShowScreenshotPicker(false);
  }

  /** Adds a step after the one holding the cursor and renumbers every following step. */
  function insertStepTemplate() {
    const result = insertStepAt(value, currentCursor(), {
      title: t("md.step_new", "New Step"),
      body: `${t("md.step_desc", "Describe in detail what the user needs to do in this step.")}\n\n- **${t("md.action", "Action")}**: ${t("md.click_on", "Click on...")}\n- **${t("md.expected", "Expected result")}**: ${t("md.window_opens", "The window opens...")}`,
      fallbackWord: t("md.step", "Step"),
    });
    onChange(result.markdown);
    lastCursor.current = result.selectionEnd;
    focusSelection(result.selectionStart, result.selectionEnd);
  }

  function handleRenumberSteps() {
    const next = renumberSteps(value);
    if (next !== value) onChange(next);
  }

  function insertTable() {
    const tableSnippet = `\n\n| ${t("md.param", "Parameter")} | ${t("md.desc", "Description")} | ${t("md.required", "Required")} |\n| :--- | :--- | :--- |\n| ${t("md.val1", "Value 1")} | ${t("md.desc1", "Explanation of the first field")} | ${t("md.yes", "Yes")} |\n| ${t("md.val2", "Value 2")} | ${t("md.desc2", "Explanation of the second field")} | ${t("md.no", "No")} |\n`;
    insertFormatting(tableSnippet, "", "");
  }

  function handleEditorKeyDown(event: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (!(event.ctrlKey || event.metaKey)) return;
    const key = event.key.toLowerCase();
    if (key === "b") {
      event.preventDefault();
      insertFormatting("**", "**", t("md.bold_text", "bold text"));
    } else if (key === "i") {
      event.preventDefault();
      insertFormatting("*", "*", t("md.italic_text", "italic text"));
    } else if (key === "s" && onSave) {
      event.preventDefault();
      onSave();
    }
  }

  const previewVisible = viewMode === "split" || viewMode === "preview";

  return (
    <div className="fc-markdown-editor" style={{ minHeight }}>
      <div className="mde-toolbar">
        <div className="mde-group">
          <button
            type="button"
            className="btn btn-ghost btn-sm mde-btn mde-bold"
            onClick={() => insertFormatting("**", "**", t("md.bold_text", "bold text"))}
            title={t("md.bold_title", "Bold (Ctrl+B)")}
          >
            B
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-sm mde-btn mde-italic"
            onClick={() => insertFormatting("*", "*", t("md.italic_text", "italic text"))}
            title={t("md.italic_title", "Italic (Ctrl+I)")}
          >
            I
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-sm mde-btn mde-strike"
            onClick={() => insertFormatting("~~", "~~", t("md.strike_text", "strikethrough text"))}
            title={t("md.strike_title", "Strikethrough")}
          >
            S
          </button>

          <span className="mde-sep" />

          {(["# ", "## ", "### "] as const).map((prefix, idx) => (
            <button
              key={prefix}
              type="button"
              className="btn btn-ghost btn-sm mde-btn mde-heading"
              onClick={() => insertLinePrefix(prefix)}
              title={t(`md.h${idx + 1}_title`, `Titolo ${idx + 1}`)}
            >
              H{idx + 1}
            </button>
          ))}

          <span className="mde-sep" />

          <button
            type="button"
            className="btn btn-ghost btn-sm mde-btn"
            onClick={() => insertLinePrefix("- ")}
            title={t("md.ul_title", "Bulleted list")}
          >
            • {t("md.list", "List")}
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-sm mde-btn"
            onClick={() => insertLinePrefix("1. ")}
            title={t("md.ol_title", "Numbered list")}
          >
            1. {t("md.list", "List")}
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-sm mde-btn"
            onClick={() => insertLinePrefix("- [ ] ")}
            title={t("md.task_title", "Task list")}
          >
            ☑ {t("md.task", "Task")}
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-sm mde-btn"
            onClick={() => insertLinePrefix("> ")}
            title={t("md.quote_title", "Quote / Callout")}
          >
            ❝ {t("md.quote", "Quote")}
          </button>

          <span className="mde-sep" />

          <button
            type="button"
            className="btn btn-ghost btn-sm mde-btn mde-mono"
            onClick={() => insertFormatting("`", "`", "codice")}
            title="Codice inline"
          >
            `code`
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-sm mde-btn mde-mono"
            onClick={() => insertFormatting("```\n", "\n```", "blocco di codice")}
            title="Blocco di codice"
          >
            ```
          </button>
          <button type="button" className="btn btn-ghost btn-sm mde-btn" onClick={insertTable} title="Inserisci una tabella">
            ⊞ Tabella
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-sm mde-btn"
            onClick={() => insertFormatting("[", "](https://example.com)", "testo del link")}
            title="Inserisci un link"
          >
            🔗 Link
          </button>

          <span className="mde-sep" />

          <div className="mde-dropdown">
            <button
              type="button"
              className="btn btn-ghost btn-sm mde-btn"
              onClick={() => setShowScreenshotPicker(!showScreenshotPicker)}
              title="Inserisci uno screenshot nella posizione del cursore"
            >
              🖼️ Inserisci screenshot {screenshots.length > 0 ? `(${screenshots.length})` : ""} ▾
            </button>

            {showScreenshotPicker && (
              <div className="mde-dropdown-menu">
                {screenshots.length === 0 ? (
                  <div className="mde-dropdown-empty">Nessuno screenshot disponibile in questa sessione.</div>
                ) : (
                  screenshots.map((shot, idx) => (
                    <button
                      key={shot.id}
                      type="button"
                      className="mde-dropdown-item"
                      onClick={() => insertScreenshotMarkdown(shot, idx)}
                    >
                      <span className="mde-dropdown-num">#{idx + 1}</span>
                      <div className="mde-dropdown-text">
                        <div>{shot.trigger || "screenshot"}</div>
                        <div className="mde-dropdown-file">{shot.path.split(/[/\\]/).pop()}</div>
                      </div>
                    </button>
                  ))
                )}
              </div>
            )}
          </div>

          {!hideStepTemplate && (
            <>
              <button
                type="button"
                className="btn btn-ghost btn-sm mde-btn mde-accent"
                onClick={insertStepTemplate}
                title="Aggiunge un nuovo passo dopo quello in cui si trova il cursore e rinumera i passi successivi"
              >
                + Aggiungi passo
              </button>
              <button
                type="button"
                className="btn btn-ghost btn-sm mde-btn"
                onClick={handleRenumberSteps}
                title="Rinumera tutti i passi in ordine (1, 2, 3…)"
              >
                🔢 Rinumera
              </button>
            </>
          )}
        </div>

        <div className="mde-group">
          {previewVisible && (
            <div className="mde-zoom" title="Regola la dimensione del testo dell'anteprima">
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                onClick={() => setZoom((z) => Math.max(70, z - 10))}
                title="Riduci la dimensione del testo"
              >
                A−
              </button>
              <button
                type="button"
                className={`btn btn-ghost btn-sm mde-zoom-value${zoom === 100 ? "" : " changed"}`}
                onClick={() => setZoom(100)}
                title="Torna alla dimensione predefinita (100%)"
              >
                {zoom}%
              </button>
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                onClick={() => setZoom((z) => Math.min(160, z + 10))}
                title="Aumenta la dimensione del testo"
              >
                A+
              </button>
            </div>
          )}

          {previewVisible && (
            <button
              type="button"
              className={`mde-toggle${editablePreview ? " on" : ""}`}
              onClick={() => setEditablePreview(!editablePreview)}
              aria-pressed={editablePreview}
              title="Quando è attiva puoi cliccare sull'anteprima e modificare il testo direttamente"
            >
              <span>✏️</span>
              <span>{compact ? "Anteprima modificabile" : "Modifica nell'anteprima"}</span>
            </button>
          )}

          <div className="seg mde-seg">
            <button
              type="button"
              className={viewMode === "split" ? "on" : ""}
              onClick={() => setViewMode("split")}
              title="Editor e anteprima affiancati e sincronizzati"
            >
              Affiancati
            </button>
            <button
              type="button"
              className={viewMode === "edit" ? "on" : ""}
              onClick={() => setViewMode("edit")}
              title="Mostra solo il testo Markdown"
            >
              Solo testo
            </button>
            <button
              type="button"
              className={viewMode === "preview" ? "on" : ""}
              onClick={() => setViewMode("preview")}
              title="Mostra solo l'anteprima formattata"
            >
              Solo anteprima
            </button>
          </div>

          {saving && (
            <span className="mde-saving">
              <span className="fc-spinner" />
              Salvataggio…
            </span>
          )}

          {showSaveButton && (
            <div className="mde-save-group">
              {onCancel && (
                <button type="button" className="btn btn-ghost btn-sm mde-btn" onClick={onCancel} disabled={saving}>
                  Annulla
                </button>
              )}
              {onSave && (
                <button type="button" className="mde-save" onClick={onSave} disabled={saving}>
                  {saving ? "Salvataggio…" : "Salva"}
                </button>
              )}
            </div>
          )}
        </div>
      </div>

      <div className="mde-body" style={{ minHeight, maxHeight }}>
        {/* Off-screen mirror to measure exact textarea line heights and word wraps */}
        <div
          ref={mirrorRef}
          aria-hidden="true"
          className="mde-mirror"
          style={{ width: textareaWidth ? `${textareaWidth}px` : "100%" }}
        >
          {value.split("\n").map((line, idx) => (
            <div key={idx} data-line={idx + 1}>
              {line || " "}
            </div>
          ))}
        </div>

        {(viewMode === "split" || viewMode === "edit") && (
          <div className={`mde-source${viewMode === "split" ? " split" : ""}`}>
            <textarea
              ref={textareaRef}
              value={value}
              onChange={(e) => onChange(e.target.value)}
              onSelect={(e) => {
                lastCursor.current = e.currentTarget.selectionStart;
              }}
              onScroll={handleEditorScroll}
              onKeyDown={handleEditorKeyDown}
              disabled={disabled}
              placeholder="Scrivi qui il testo della guida in Markdown, oppure usa la barra degli strumenti qui sopra…"
              style={{ minHeight }}
            />
          </div>
        )}

        {previewVisible && (
          <div
            ref={previewContainerRef}
            onScroll={handlePreviewScroll}
            className={`mde-preview${viewMode === "split" ? " split" : ""}`}
            style={{ minHeight, maxHeight }}
          >
            <MarkdownPreview
              markdown={value}
              screenshots={screenshots}
              editable={editablePreview}
              onTextChange={handlePreviewTextChange}
              zoom={zoom}
              imageActions={imageActions}
              imageVersion={imageVersion}
            />
          </div>
        )}
      </div>

      {previewVisible && (editablePreview || imageActions) ? (
        <div className="mde-hint">
          {editablePreview ? (
            <span>✏️ Clicca su titoli, paragrafi o elenchi dell'anteprima per modificarli: le modifiche si sincronizzano con il Markdown.</span>
          ) : null}
          <span>🖼️ Tasto destro su un'immagine per ingrandirla{imageActions?.onChange ? ", cambiarla" : ""}{imageActions?.onAnnotate ? " o annotarla" : ""}.</span>
        </div>
      ) : null}
    </div>
  );
}
