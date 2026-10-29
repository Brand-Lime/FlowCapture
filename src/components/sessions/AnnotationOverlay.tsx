import { useState } from "react";
import type { AnnotationItem } from "./ImageAnnotationModal";

interface AnnotationOverlayProps {
  items: AnnotationItem[];
  naturalWidth: number;
  naturalHeight: number;
  /** Draw-only mode (e.g. inside the full-screen viewer): no hover, clicks or action buttons. */
  readOnly?: boolean;
  onAnnotationClick?: (item: AnnotationItem) => void;
  onOpenEditor?: () => void;
  onChangeScreenshot?: () => void;
  onFullscreen?: () => void;
}

export function AnnotationOverlay({
  items,
  naturalWidth,
  naturalHeight,
  readOnly = false,
  onAnnotationClick,
  onOpenEditor,
  onChangeScreenshot,
  onFullscreen,
}: AnnotationOverlayProps) {
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const interactive = !readOnly && Boolean(onAnnotationClick);
  const groupStyle = interactive ? { pointerEvents: "all" as const, cursor: "pointer" } : { pointerEvents: "none" as const };

  const viewBoxW = naturalWidth || 1920;
  const viewBoxH = naturalHeight || 1080;

  return (
    <div className="annotation-overlay">
      {/* Interactive SVG layer mapped directly over the image */}
      <svg className="annotation-overlay-svg" viewBox={`0 0 ${viewBoxW} ${viewBoxH}`}>
        <defs>
          <filter id="glow" x="-20%" y="-20%" width="140%" height="140%">
            <feDropShadow dx="0" dy="0" stdDeviation="6" floodColor="#38bdf8" floodOpacity="0.8" />
          </filter>
        </defs>

        {items.map((item) => {
          const isHovered = hoveredId === item.id;
          const filter = isHovered ? "url(#glow)" : undefined;

          if (item.type === "click") {
            const cx = item.x;
            const cy = item.y;
            const r = 26;

            return (
              <g
                key={item.id}
                style={groupStyle}
                onMouseEnter={() => setHoveredId(item.id)}
                onMouseLeave={() => setHoveredId(null)}
                onClick={(e) => {
                  e.stopPropagation();
                  onAnnotationClick?.(item);
                }}
              >
                <circle
                  cx={cx}
                  cy={cy}
                  r={r}
                  fill="none"
                  stroke={item.color || "#ef4444"}
                  strokeWidth={isHovered ? 4 : 3}
                  filter={filter}
                />
                <circle
                  cx={cx}
                  cy={cy}
                  r={r + 8}
                  fill="none"
                  stroke={item.color || "#ef4444"}
                  strokeWidth="1.5"
                  strokeDasharray="4 3"
                  opacity="0.75"
                />
                <circle cx={cx} cy={cy} r={5} fill={item.color || "#ef4444"} />
                {/* Crosshair lines */}
                <line
                  x1={cx - r - 4}
                  y1={cy}
                  x2={cx - 8}
                  y2={cy}
                  stroke={item.color || "#ef4444"}
                  strokeWidth="2"
                />
                <line
                  x1={cx + 8}
                  y1={cy}
                  x2={cx + r + 4}
                  y2={cy}
                  stroke={item.color || "#ef4444"}
                  strokeWidth="2"
                />
                <line
                  x1={cx}
                  y1={cy - r - 4}
                  x2={cx}
                  y2={cy - 8}
                  stroke={item.color || "#ef4444"}
                  strokeWidth="2"
                />
                <line
                  x1={cx}
                  y1={cy + 8}
                  x2={cx}
                  y2={cy + r + 4}
                  stroke={item.color || "#ef4444"}
                  strokeWidth="2"
                />

                {isHovered && (
                  <g transform={`translate(${cx + 32}, ${cy - 12})`}>
                    <rect
                      x="0"
                      y="-16"
                      width="160"
                      height="26"
                      rx="4"
                      fill="#0f172a"
                      stroke="#38bdf8"
                      strokeWidth="1"
                    />
                    <text
                      x="8"
                      y="1"
                      fill="#f8fafc"
                      fontSize="12"
                      fontWeight="600"
                      fontFamily="sans-serif"
                    >
                      Modifica click (clicca)
                    </text>
                  </g>
                )}
              </g>
            );
          }

          if (item.type === "badge") {
            const bx = item.x;
            const by = item.y;
            const badgeRadius = 22;

            return (
              <g
                key={item.id}
                style={groupStyle}
                onMouseEnter={() => setHoveredId(item.id)}
                onMouseLeave={() => setHoveredId(null)}
                onClick={(e) => {
                  e.stopPropagation();
                  onAnnotationClick?.(item);
                }}
              >
                <circle
                  cx={bx}
                  cy={by}
                  r={badgeRadius}
                  fill={item.color || "#ef4444"}
                  stroke="#ffffff"
                  strokeWidth="2.5"
                  filter={filter}
                />
                <text
                  x={bx}
                  y={by + 6}
                  fill="#ffffff"
                  fontSize="16"
                  fontWeight="bold"
                  textAnchor="middle"
                  fontFamily="sans-serif"
                >
                  {item.badgeNumber ?? 1}
                </text>

                {item.badgeText && (
                  <g transform={`translate(${bx + (item.textOffsetX ?? 28)}, ${by + (item.textOffsetY ?? -10)})`}>
                    <rect
                      x="0"
                      y="-16"
                      width={Math.max(80, item.badgeText.length * 8 + 16)}
                      height="28"
                      rx="5"
                      fill="#1e293b"
                      stroke={item.color || "#ef4444"}
                      strokeWidth="1.5"
                    />
                    <text
                      x="8"
                      y="3"
                      fill="#f8fafc"
                      fontSize="13"
                      fontWeight="500"
                      fontFamily="sans-serif"
                    >
                      {item.badgeText}
                    </text>
                  </g>
                )}
              </g>
            );
          }

          if (item.type === "rect") {
            const rx = item.x;
            const ry = item.y;
            const rw = item.w || 80;
            const rh = item.h || 50;

            return (
              <g
                key={item.id}
                style={groupStyle}
                onMouseEnter={() => setHoveredId(item.id)}
                onMouseLeave={() => setHoveredId(null)}
                onClick={(e) => {
                  e.stopPropagation();
                  onAnnotationClick?.(item);
                }}
              >
                <rect
                  x={rw < 0 ? rx + rw : rx}
                  y={rh < 0 ? ry + rh : ry}
                  width={Math.abs(rw)}
                  height={Math.abs(rh)}
                  fill="none"
                  stroke={item.color || "#ef4444"}
                  strokeWidth={isHovered ? (item.strokeWidth || 4) + 2 : item.strokeWidth || 4}
                  rx="4"
                  filter={filter}
                />
              </g>
            );
          }

          if (item.type === "circle") {
            const cx = item.x;
            const cy = item.y;
            const rw = Math.abs(item.w || 60);
            const rh = Math.abs(item.h || 60);

            return (
              <g
                key={item.id}
                style={groupStyle}
                onMouseEnter={() => setHoveredId(item.id)}
                onMouseLeave={() => setHoveredId(null)}
                onClick={(e) => {
                  e.stopPropagation();
                  onAnnotationClick?.(item);
                }}
              >
                <ellipse
                  cx={cx}
                  cy={cy}
                  rx={rw}
                  ry={rh}
                  fill="none"
                  stroke={item.color || "#ef4444"}
                  strokeWidth={isHovered ? (item.strokeWidth || 4) + 2 : item.strokeWidth || 4}
                  filter={filter}
                />
              </g>
            );
          }

          if (item.type === "highlight") {
            const hx = item.x;
            const hy = item.y;
            const hw = item.w || 100;
            const hh = item.h || 30;

            return (
              <g
                key={item.id}
                style={groupStyle}
                onMouseEnter={() => setHoveredId(item.id)}
                onMouseLeave={() => setHoveredId(null)}
                onClick={(e) => {
                  e.stopPropagation();
                  onAnnotationClick?.(item);
                }}
              >
                <rect
                  x={hw < 0 ? hx + hw : hx}
                  y={hh < 0 ? hy + hh : hy}
                  width={Math.abs(hw)}
                  height={Math.abs(hh)}
                  fill={item.color || "#facc15"}
                  fillOpacity={isHovered ? 0.45 : 0.28}
                  stroke={item.color || "#facc15"}
                  strokeWidth={isHovered ? 2 : 1}
                  rx="3"
                  filter={filter}
                />
              </g>
            );
          }

          if (item.type === "text" && item.text) {
            return (
              <g
                key={item.id}
                style={groupStyle}
                onMouseEnter={() => setHoveredId(item.id)}
                onMouseLeave={() => setHoveredId(null)}
                onClick={(e) => {
                  e.stopPropagation();
                  onAnnotationClick?.(item);
                }}
              >
                <rect
                  x={item.x - 4}
                  y={item.y - 18}
                  width={item.text.length * 9 + 12}
                  height="26"
                  rx="4"
                  fill="rgba(15, 23, 42, 0.85)"
                  stroke={item.color || "#ef4444"}
                  strokeWidth="1.5"
                  filter={filter}
                />
                <text
                  x={item.x + 2}
                  y={item.y}
                  fill={item.color || "#ffffff"}
                  fontSize="14"
                  fontWeight="600"
                  fontFamily="sans-serif"
                >
                  {item.text}
                </text>
              </g>
            );
          }

          return null;
        })}
      </svg>

      {!readOnly && (onChangeScreenshot || onOpenEditor || onFullscreen) ? (
        <div className="annotation-overlay-actions">
          {onFullscreen ? (
            <button
              type="button"
              className="ao-btn"
              onClick={onFullscreen}
              title="Ingrandisci a schermo intero (anche con doppio clic o tasto destro)"
            >
              <span>⛶</span>
              <span>Schermo intero</span>
            </button>
          ) : null}
          {onChangeScreenshot ? (
            <button
              type="button"
              className="ao-btn"
              onClick={onChangeScreenshot}
              title="Cambia l'immagine associata a questo passo (oppure tasto destro sullo screenshot)"
            >
              <span>🖼️</span>
              <span>Cambia immagine</span>
            </button>
          ) : null}
          {onOpenEditor ? (
            <button
              type="button"
              className="ao-btn primary"
              onClick={onOpenEditor}
              title="Apri l'editor per aggiungere o modificare numeri, rettangoli, evidenziazioni o click"
            >
              <span>🎨</span>
              <span>Annota {items.length > 0 ? `(${items.length})` : ""}</span>
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
