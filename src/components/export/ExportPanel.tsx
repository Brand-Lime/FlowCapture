import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState, type CSSProperties, type ReactNode } from "react";
import { AppButton } from "@/components/ui/AppButton";
import { Icon } from "@/components/ui/Icon";
import { Logo } from "@/components/ui/Logo";
import type { ExportRecord, Session, Screenshot, WorkflowStep } from "@/lib/api";
import {
  ACCENT_COLORS,
  DEFAULT_EXPORT_OPTIONS,
  exportAccentColor,
  exportAccentInk,
  toApiExportOptions,
  type ExportAccent,
  type ExportFormat,
  type ExportOptions,
  type ExportPageSize,
  type ExportTheme,
} from "@/lib/exportOptions";
import { exportTypeClass } from "@/lib/icons";
import { formatDuration } from "@/lib/utils";
import { useLanguage } from "@/i18n";

type ExportPanelProps = {
  session: Session;
  steps: WorkflowStep[];
  screenshots: Screenshot[];
  exports: ExportRecord[];
  busy: boolean;
  onExport: (format: string, options?: ReturnType<typeof toApiExportOptions>) => Promise<void>;
  onRevealExport: (path: string) => void;
};

function mdInline(text: string) {
  const parts: ReactNode[] = [];
  const re = /(\*\*[^*]+\*\*)/g;
  let last = 0;
  let match: RegExpExecArray | null;
  while ((match = re.exec(text))) {
    if (match.index > last) parts.push(text.slice(last, match.index));
    parts.push(<b key={match.index}>{match[0].slice(2, -2)}</b>);
    last = match.index + match[0].length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return parts;
}

function renderMdBlocks(text: string) {
  const lines = text.split("\n");
  const blocks: ReactNode[] = [];
  let listItems: string[] = [];

  const flushList = () => {
    if (listItems.length > 0) {
      blocks.push(
        <ul key={`ul-${blocks.length}`}>
          {listItems.map((item, idx) => (
            <li key={idx}>{mdInline(item)}</li>
          ))}
        </ul>
      );
      listItems = [];
    }
  };

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();
    if (!line) {
      flushList();
      continue;
    }
    if (line.startsWith("- ") || line.startsWith("* ")) {
      listItems.push(line.slice(2).trim());
    } else {
      flushList();
      blocks.push(<p key={`p-${blocks.length}`}>{mdInline(line)}</p>);
    }
  }
  flushList();
  return blocks;
}

interface ParsedPreviewStep {
  step: number;
  title: string;
  description: string;
  reason?: string;
  timestamp_ms: number;
  screenshot?: Screenshot;
}

interface ParsedPreviewDoc {
  title: string;
  overview: string;
  prerequisites?: string;
  steps: ParsedPreviewStep[];
  expectedResult?: string;
}

function parseDocumentationMarkdown(
  md: string,
  screenshots: Screenshot[],
  sessionTitle: string,
  fallbackSteps: WorkflowStep[],
  defaultOverview: string,
): ParsedPreviewDoc {
  if (!md || !md.trim()) {
    return {
      title: sessionTitle || "Workflow Documentation",
      overview: defaultOverview,
      steps: fallbackSteps.map((s, idx) => ({
        step: s.step || idx + 1,
        title: s.title,
        description: s.description,
        reason: s.reason?.trim() || undefined,
        timestamp_ms: s.timestamp_ms,
        screenshot: s.screenshot_ids[0]
          ? screenshots.find((shot) => shot.id === s.screenshot_ids[0])
          : undefined,
      })),
    };
  }

  let title = sessionTitle || "Workflow Documentation";
  const overviewLines: string[] = [];
  const prereqLines: string[] = [];
  const expectedLines: string[] = [];
  const parsedSteps: ParsedPreviewStep[] = [];

  const h1Re = /^#\s+(.+)$/;
  const h2Re = /^##\s+(.+)$/;
  const stepRe = /^#{2,4}\s+(?:Step|Passo)?\s*(\d+)[:.]\s*(.*)$/i;
  const whyRe = /^>\s*\*\*(?:Why|Perch[ée])\s*:\*\*\s*(.*)$/i;
  const imgRe = /!\[.*?\]\((.*?)\)/;

  type Section = "none" | "overview" | "prereq" | "steps" | "expected" | "other";
  let currentSection: Section = "none";
  let currentStep: ParsedPreviewStep | null = null;

  for (const line of md.split("\n")) {
    const trimmed = line.trim();

    const h1Match = h1Re.exec(trimmed);
    if (h1Match) {
      title = h1Match[1].trim() || title;
      currentSection = "overview";
      continue;
    }

    const h2Match = h2Re.exec(trimmed);
    if (h2Match) {
      if (currentStep) {
        parsedSteps.push(currentStep);
        currentStep = null;
      }
      const h2Text = h2Match[1].trim().toLowerCase();
      if (
        h2Text.includes("prerequisit") ||
        h2Text.includes("pre-requisit") ||
        h2Text.includes("requisiti")
      ) {
        currentSection = "prereq";
      } else if (
        h2Text.includes("step") ||
        h2Text.includes("passagg") ||
        h2Text.includes("procedura") ||
        h2Text.includes("istruzioni")
      ) {
        currentSection = "steps";
      } else if (
        h2Text.includes("expected") ||
        h2Text.includes("risultato") ||
        h2Text.includes("conclusione") ||
        h2Text.includes("result")
      ) {
        currentSection = "expected";
      } else if (
        h2Text.includes("overview") ||
        h2Text.includes("panoramica") ||
        h2Text.includes("introduzione")
      ) {
        currentSection = "overview";
      } else {
        currentSection = "other";
      }
      continue;
    }

    const stepMatch = stepRe.exec(trimmed);
    if (stepMatch) {
      if (currentStep) {
        parsedSteps.push(currentStep);
      }
      const num = parseInt(stepMatch[1], 10) || parsedSteps.length + 1;
      const sTitle = stepMatch[2].trim() || `Step ${num}`;
      currentStep = {
        step: num,
        title: sTitle,
        description: "",
        timestamp_ms: 0,
      };
      currentSection = "steps";
      continue;
    }

    switch (currentSection) {
      case "overview":
        if (trimmed) overviewLines.push(trimmed);
        break;
      case "prereq":
        if (trimmed) prereqLines.push(trimmed);
        break;
      case "expected":
        if (trimmed) expectedLines.push(trimmed);
        break;
      case "steps":
        if (currentStep) {
          const whyMatch = whyRe.exec(trimmed);
          if (whyMatch) {
            currentStep.reason = whyMatch[1].trim();
          } else {
            const imgMatch = imgRe.exec(trimmed);
            if (imgMatch) {
              const imgRef = imgMatch[1].trim();
              const foundShot = imgRef && screenshots.find(
                (s) =>
                  s.path === imgRef ||
                  s.path.endsWith(imgRef) ||
                  (imgRef && s.path.endsWith(imgRef.split(/[/\\]/).pop() || "___none___")),
              );
              if (foundShot) {
                currentStep.screenshot = foundShot;
              }
            } else if (trimmed) {
              currentStep.description = currentStep.description
                ? `${currentStep.description}\n${trimmed}`
                : trimmed;
            }
          }
        }
        break;
    }
  }

  if (currentStep) {
    parsedSteps.push(currentStep);
  }

  if (parsedSteps.length === 0 && fallbackSteps.length > 0) {
    for (let i = 0; i < fallbackSteps.length; i++) {
      const fs = fallbackSteps[i];
      parsedSteps.push({
        step: fs.step || i + 1,
        title: fs.title,
        description: fs.description,
        reason: fs.reason?.trim() || undefined,
        timestamp_ms: fs.timestamp_ms,
        screenshot: fs.screenshot_ids[0]
          ? screenshots.find((shot) => shot.id === fs.screenshot_ids[0])
          : undefined,
      });
    }
  }

  // Same rules as the Rust exporter: fill gaps from the stored step with the same number,
  // never from an unrelated screenshot at the same position.
  for (const st of parsedSteps) {
    const stored = fallbackSteps.find((fs) => fs.step === st.step);
    if (!st.screenshot && stored?.screenshot_ids[0]) {
      st.screenshot = screenshots.find((shot) => shot.id === stored.screenshot_ids[0]);
    }
    if (!st.timestamp_ms && stored?.timestamp_ms) {
      st.timestamp_ms = stored.timestamp_ms;
    }
    if (!st.timestamp_ms && st.screenshot) {
      st.timestamp_ms = st.screenshot.timestamp_ms;
    }
  }

  return {
    title,
    overview: overviewLines.join("\n") || defaultOverview,
    prerequisites: prereqLines.length > 0 ? prereqLines.join("\n") : undefined,
    steps: parsedSteps,
    expectedResult: expectedLines.length > 0 ? expectedLines.join("\n") : undefined,
  };
}

function sessionStartMs(startedAt: string) {
  const value = Date.parse(startedAt);
  return Number.isNaN(value) ? 0 : value;
}

function stepTime(stepMs: number, startMs: number) {
  const secs = Math.max(0, Math.floor((stepMs - startMs) / 1000));
  return `${String(Math.floor(secs / 60)).padStart(2, "0")}:${String(secs % 60).padStart(2, "0")}`;
}

function sessionDate(startedAt: string) {
  return new Date(startedAt).toLocaleDateString(undefined, {
    month: "long",
    day: "numeric",
    year: "numeric",
  });
}

/** Mirrors `export_image_path` in the Rust exporter: annotated or pristine variant when present. */
function ExportShotImage({ shot, annotations, alt }: { shot: Screenshot; annotations: boolean; alt: string }) {
  const variant = shot.path.replace(/(\.[a-zA-Z0-9]+)$/, annotations ? "_annotated$1" : "_clean$1");
  const [src, setSrc] = useState(convertFileSrc(variant));
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    setSrc(convertFileSrc(variant));
    setFailed(false);
  }, [variant]);

  if (failed) return null;
  return (
    <img
      src={src}
      alt={alt}
      onError={() => {
        const original = convertFileSrc(shot.path);
        if (src !== original) setSrc(original);
        else setFailed(true);
      }}
    />
  );
}

function Toggle({ active, onClick }: { active: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      className={`tg${active ? " on" : ""}`}
      aria-pressed={active}
      onClick={onClick}
    />
  );
}

const ACCENT_NAMES: Record<ExportAccent, string> = {
  mint: "menta",
  blue: "blu",
  violet: "viola",
  amber: "ambra",
};

export function ExportPanel({
  session,
  steps,
  screenshots,
  exports,
  busy,
  onExport,
  onRevealExport,
}: ExportPanelProps) {
  const { t } = useLanguage();
  const [format, setFormat] = useState<ExportFormat>("pdf");
  const [options, setOptions] = useState<ExportOptions>(DEFAULT_EXPORT_OPTIONS);
  const [copied, setCopied] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);

  const startMs = sessionStartMs(session.started_at);
  const accentColor = exportAccentColor(options.accent, options.theme);
  const accentInk = exportAccentInk(options.theme);

  const defaultOverview = t(
    "export.overview",
    "A step-by-step guide generated from your recorded session with screenshots and workflow context.",
  );

  const parsedDoc = useMemo(
    () =>
      parseDocumentationMarkdown(
        session.documentation_md || "",
        screenshots,
        session.title,
        steps,
        defaultOverview,
      ),
    [session.documentation_md, session.title, screenshots, steps, defaultOverview],
  );

  const docTitle = parsedDoc.title;
  const overview = parsedDoc.overview;

  function patchOptions(patch: Partial<ExportOptions>) {
    setOptions((current) => ({ ...current, ...patch }));
  }

  async function handlePrimaryExport() {
    await onExport(format, toApiExportOptions(options));
  }

  async function handleCopyPath() {
    const latest = exports[0];
    if (!latest) return;
    try {
      await navigator.clipboard.writeText(latest.path);
    } catch {
      return;
    }
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1400);
  }

  const docClass = [
    "export-doc",
    `theme-${options.theme}`,
    `mode-${format}`,
    options.pageSize === "a4" ? "size-a4" : "size-letter",
    options.branding ? "" : "no-brand",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div className={`export-studio${sidebarOpen ? "" : " sidebar-collapsed"}`}>
      <aside className="config" aria-hidden={!sidebarOpen}>
        <div className="cfg-head">
          <Logo size={34} />
          <div>
            <div className="lt">{t("session.export.btn", "Esporta")}</div>
            <div className="ls">{t("export.options", "Export Options")}</div>
          </div>
        </div>

        <div className="cfg-body">
          <div className="cfg-sec">
            <h4>{t("export.format", "Format")}</h4>
            <div className="seg">
              {(["pdf", "html"] as ExportFormat[]).map((value) => (
                <button
                  key={value}
                  type="button"
                  className={format === value ? "on" : ""}
                  onClick={() => setFormat(value)}
                >
                  <Icon name={value === "pdf" ? "file" : "edit"} size={15} />
                  {value.toUpperCase()}
                </button>
              ))}
            </div>
          </div>

          <div className="cfg-sec">
            <h4>{t("export.theme", "Theme")}</h4>
            <div className="seg">
              {(["dark", "light"] as ExportTheme[]).map((value) => (
                <button
                  key={value}
                  type="button"
                  className={options.theme === value ? "on" : ""}
                  onClick={() => patchOptions({ theme: value })}
                >
                  {value === "dark" ? "Scuro" : "Chiaro"}
                </button>
              ))}
            </div>
          </div>

          <div className="cfg-sec">
            <h4>{t("export.accent", "Accent Color")}</h4>
            <div className="swatches">
              {(Object.keys(ACCENT_COLORS) as ExportAccent[]).map((accent) => (
                <button
                  key={accent}
                  type="button"
                  className={`sw${options.accent === accent ? " on" : ""}`}
                  style={{ background: ACCENT_COLORS[accent].dark }}
                  aria-label={`Colore ${ACCENT_NAMES[accent]}`}
                  title={ACCENT_NAMES[accent]}
                  onClick={() => patchOptions({ accent })}
                />
              ))}
            </div>
          </div>

          <div className={`cfg-sec${format === "html" ? " dimmed" : ""}`}>
            <h4>{t("export.page_size", "Page Size")}</h4>
            <select
              className="selectish"
              value={options.pageSize}
              disabled={format === "html"}
              onChange={(event) =>
                patchOptions({ pageSize: event.target.value as ExportPageSize })
              }
            >
              <option value="letter">Lettera USA · 8,5 × 11 in</option>
              <option value="a4">A4 · 210 × 297 mm</option>
            </select>
          </div>

          <div className="cfg-sec">
            <h4>Includi</h4>
            <div className="row">
              <span className="cfg-label">Copertina</span>
              <Toggle active={options.cover} onClick={() => patchOptions({ cover: !options.cover })} />
            </div>
            <div className="row">
              <span className="cfg-label">Screenshot</span>
              <Toggle
                active={options.screenshots}
                onClick={() => patchOptions({ screenshots: !options.screenshots })}
              />
            </div>
            <div className="row">
              <span className="cfg-label">Numeri dei passi</span>
              <Toggle
                active={options.stepNumbers}
                onClick={() => patchOptions({ stepNumbers: !options.stepNumbers })}
              />
            </div>
            <div className="row">
              <span className="cfg-label">Orari</span>
              <Toggle
                active={options.timestamps}
                onClick={() => patchOptions({ timestamps: !options.timestamps })}
              />
            </div>
            <div className="row">
              <div>
                <span className="cfg-label">Annotazioni grafiche</span>
                <div className="cfg-note">Includi evidenziazioni, click e numeri disegnati</div>
              </div>
              <Toggle
                active={options.annotations}
                onClick={() => patchOptions({ annotations: !options.annotations })}
              />
            </div>
          </div>

          <div className="cfg-sec">
            <h4>Marchio</h4>
            <div className="row">
              <span className="cfg-label">Logo FlowCapture</span>
              <Toggle
                active={options.branding}
                onClick={() => patchOptions({ branding: !options.branding })}
              />
            </div>
            <div style={{ fontSize: 12, color: "var(--dim)", lineHeight: 1.5, marginTop: 4 }}>
              Disattivalo per esportare un documento neutro, senza logo né piè di pagina di FlowCapture.
            </div>
          </div>
        </div>

        <div className="cfg-foot">
          <AppButton kind="primary" icon="download" disabled={busy} onClick={handlePrimaryExport}>
            {format === "pdf" ? "Salva come PDF" : "Salva come HTML"}
          </AppButton>
          <div className="btn-row">
            <AppButton
              disabled={busy}
              onClick={() => onExport("markdown", toApiExportOptions(options))}
            >
              Markdown
            </AppButton>
            <AppButton disabled={busy} onClick={() => onExport("video")}>
              Video
            </AppButton>
          </div>
          <AppButton icon="copy" disabled={!exports.length} onClick={handleCopyPath}>
            {copied ? "Percorso copiato" : "Copia percorso ultimo file"}
          </AppButton>
        </div>
      </aside>

      <div className="stage">
        <button
          type="button"
          className="sidebar-toggle"
          aria-label={sidebarOpen ? "Nascondi opzioni di esportazione" : "Mostra opzioni di esportazione"}
          title={sidebarOpen ? "Nascondi opzioni di esportazione" : "Mostra opzioni di esportazione"}
          aria-expanded={sidebarOpen}
          onClick={() => setSidebarOpen((open) => !open)}
        >
          <Icon name={sidebarOpen ? "chevronLeft" : "chevronRight"} size={16} />
        </button>
        <div className="stage-scroll">
          <div className="stage-inner">
            <div className="doc-fit">
          <div
            className={docClass}
            style={
              {
                "--p-accent": accentColor,
                "--p-accent-ink": accentInk,
                "--p-accent-soft": `color-mix(in srgb, ${accentColor} 13%, transparent)`,
                "--p-accent-line": `color-mix(in srgb, ${accentColor} 32%, transparent)`,
              } as CSSProperties
            }
          >
            <section className="export-sheet">
              {options.cover ? (
                <div className="cover">
                  <div className="cover-band">
                    <div className="cover-brand" data-brand>
                      <Logo size={32} variant="accent" />
                      <div>
                        <div className="bn">FlowCapture</div>
                        <div className="bt">Workflow documentation</div>
                      </div>
                    </div>
                    <div className="cover-kicker">Standard Operating Procedure</div>
                    <h1>{docTitle}</h1>
                    <p className="lede">{overview}</p>
                    <div className="cover-meta">
                      <span className="cmeta">
                        <Icon name="file" size={14} />
                        {sessionDate(session.started_at)}
                      </span>
                      <span className="cmeta">
                        <Icon name="edit" size={14} />
                        {parsedDoc.steps.length || "No"} steps
                      </span>
                      <span className="cmeta">
                        <Icon name="monitor" size={14} />
                        {formatDuration(session.duration)} recording
                      </span>
                    </div>
                  </div>
                </div>
              ) : null}

              <div className="sheet-pad">
                <div className="callout">
                  <Icon name="shield" size={20} />
                  <div className="ct">
                    This guide was generated automatically
                    {options.branding ? (
                      <>
                        {" "}
                        by <b>FlowCapture</b>
                      </>
                    ) : null}{" "}
                    from screen, mouse, keyboard and window events.
                  </div>
                </div>

                {parsedDoc.prerequisites ? (
                  <div className="prereq-card">
                    <div className="prereq-header">
                      <Icon name="clipboard" size={15} />
                      <span>{t("export.prerequisites", "Prerequisiti · Prerequisites")}</span>
                    </div>
                    <div className="prereq-body">
                      {renderMdBlocks(parsedDoc.prerequisites)}
                    </div>
                  </div>
                ) : null}

                <div className="steps-title">Steps</div>

                {parsedDoc.steps.length === 0 ? (
                  <div className="step no-num">
                    <div>
                      <p>Genera prima la documentazione per vedere qui i passi.</p>
                    </div>
                  </div>
                ) : (
                  parsedDoc.steps.map((step, index) => {
                    const shot = step.screenshot;
                    return (
                      <div
                        key={`${step.step}-${index}`}
                        className={`step${options.stepNumbers ? "" : " no-num"}`}
                      >
                        {options.stepNumbers ? (
                          <div className="step-n">{index + 1}</div>
                        ) : null}
                        <div>
                          <div className="step-head">
                            <h3>{step.title}</h3>
                            {options.timestamps ? (
                              <span className="ts">{stepTime(step.timestamp_ms, startMs)}</span>
                            ) : null}
                          </div>
                          <div className="step-desc">{renderMdBlocks(step.description)}</div>
                          {step.reason ? (
                            <div className="step-why">
                              <span className="why-badge">WHY</span>
                              <span className="why-text">{mdInline(step.reason)}</span>
                            </div>
                          ) : null}
                          {options.screenshots && shot ? (
                            <div className="shot">
                              <div className="shot-img has-img">
                                <ExportShotImage shot={shot} annotations={options.annotations} alt={step.title} />
                              </div>
                              <div className="shot-cap">
                                <Icon name="window" size={14} />
                                {shot.trigger ?? "capture"}
                              </div>
                            </div>
                          ) : null}
                        </div>
                      </div>
                    );
                  })
                )}

                {parsedDoc.expectedResult ? (
                  <div className="expected-card">
                    <div className="expected-header">
                      <Icon name="check" size={15} />
                      <span>{t("export.expectedResult", "Risultato Atteso · Expected Result")}</span>
                    </div>
                    <div className="expected-body">
                      {renderMdBlocks(parsedDoc.expectedResult)}
                    </div>
                  </div>
                ) : null}
              </div>

              <div className="run-foot">
                <span>
                  {options.branding ? "FlowCapture · " : ""}
                  Generated documentation
                </span>
                <span>{sessionDate(session.started_at)}</span>
              </div>
            </section>
          </div>
            </div>
          </div>
        </div>
      </div>

      <div className="export-history">
        <h3>Esportazioni fatte</h3>
        <div className="pd" style={{ color: "var(--muted)", fontSize: 14, marginBottom: 14 }}>
          Da qui puoi aprire la cartella dei file esportati.
        </div>
        <div className="exp-list">
          {exports.length === 0 ? (
            <div className="pd">Nessuna esportazione. Scegli le opzioni a sinistra e salva il documento.</div>
          ) : (
            exports.map((record) => (
              <div className="exp" key={record.id}>
                <div className="eh">
                  <span className={`etype ${exportTypeClass(record.format)}`}>
                    {record.format}
                  </span>
                </div>
                <div className="epath">{record.path}</div>
                <div className="ef">
                  <span className="et">{record.created_at}</span>
                  <button
                    type="button"
                    className="show-folder"
                    onClick={() => onRevealExport(record.path)}
                  >
                    <Icon name="folder" size={15} /> Mostra nella cartella
                  </button>
                </div>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
