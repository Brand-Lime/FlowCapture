import { useEffect, useRef, useState, type CSSProperties } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { AppButton } from "@/components/ui/AppButton";
import { useLanguage } from "@/i18n";
import { api, type Screenshot, type NewStepPayload } from "@/lib/api";

interface ImageAnnotationModalProps {
  sessionId: string;
  screenshot: Screenshot;
  stepIndex?: number;
  stepAnnotationsJson?: string | null;
  initialSelectedId?: string | null;
  onClose: () => void;
  onSaved: (updatedScreenshotId: string) => void;
}

export type ToolType = "select" | "click" | "badge" | "rect" | "circle" | "highlight" | "text";

const ITEM_TYPE_LABELS: Record<string, string> = {
  click: "click",
  badge: "numero del passo",
  rect: "rettangolo",
  circle: "cerchio",
  highlight: "evidenziatore",
  text: "testo",
};

export interface AnnotationItem {
  id: string;
  type: ToolType;
  x: number;
  y: number;
  w?: number;
  h?: number;
  color: string;
  strokeWidth: number;
  badgeNumber?: number;
  badgeText?: string;
  textOffsetX?: number;
  textOffsetY?: number;
  text?: string;
}

export function ImageAnnotationModal({
  sessionId,
  screenshot,
  stepIndex,
  stepAnnotationsJson,
  initialSelectedId,
  onClose,
  onSaved,
}: ImageAnnotationModalProps) {
  const { t } = useLanguage();

  const PALETTE = [
    { label: t("annotation.color.red", "Red"), val: "#ef4444" },
    { label: t("annotation.color.yellow", "Yellow"), val: "#facc15" },
    { label: t("annotation.color.blue", "Blue"), val: "#3b82f6" },
    { label: t("annotation.color.green", "Green"), val: "#22c55e" },
    { label: t("annotation.color.purple", "Purple"), val: "#a855f7" },
    { label: t("annotation.color.orange", "Orange"), val: "#f97316" },
    { label: "Azzurro", val: "#06b6d4" },
    { label: t("annotation.color.white", "White"), val: "#ffffff" },
  ];
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const imageRef = useRef<HTMLImageElement | null>(null);

  const [activeTool, setActiveTool] = useState<ToolType>("select");
  const [activeColor, setActiveColor] = useState<string>("#ef4444");
  const [strokeWidth, setStrokeWidth] = useState<number>(4);

  const [items, setItems] = useState<AnnotationItem[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const [isInteracting, setIsInteracting] = useState(false);
  const [dragMode, setDragMode] = useState<"draw" | "move_item" | "move_badge_text" | null>(null);
  const [dragStartPos, setDragStartPos] = useState<{ x: number; y: number } | null>(null);
  const [initialItemState, setInitialItemState] = useState<AnnotationItem | null>(null);

  const [badgeCounter, setBadgeCounter] = useState(1);

  const [asNewStep, setAsNewStep] = useState(false);
  const [stepTitle, setStepTitle] = useState(
    screenshot.trigger ? t("md.step_prefix", "Passo:") + " " + screenshot.trigger : t("md.step_new", "Nuovo passo")
  );
  const [stepDescription, setStepDescription] = useState("");
  const [scopeToStep, setScopeToStep] = useState<boolean>(Boolean(stepIndex));

  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let initialItems: AnnotationItem[] = [];
    const jsonToParse = stepAnnotationsJson || screenshot.annotations_json;
    if (jsonToParse) {
      try {
        const parsed = JSON.parse(jsonToParse);
        if (Array.isArray(parsed)) {
          initialItems = parsed;
        }
      } catch {
        // ignore
      }
    }

    if (initialItems.length === 0 && screenshot.click_x != null && screenshot.click_y != null) {
      initialItems.push({
        id: "click_primary",
        type: "click",
        x: screenshot.click_x,
        y: screenshot.click_y,
        color: "#ef4444",
        strokeWidth: 3,
      });
    }

    setItems(initialItems);

    if (initialSelectedId) {
      setSelectedId(initialSelectedId);
      setActiveTool("select");
    }

    const highestBadge = initialItems
      .filter((i) => i.type === "badge" && i.badgeNumber != null)
      .reduce((max, i) => Math.max(max, i.badgeNumber || 0), 0);
    setBadgeCounter(highestBadge + 1);

    const img = new Image();
    img.crossOrigin = "anonymous";
    const cleanSrc = screenshot.path.replace(/(.[a-zA-Z0-9]+)$/, "_clean$1");
    img.src = convertFileSrc(cleanSrc) + `?t=${Date.now()}`;
    img.onerror = () => {
      img.src = convertFileSrc(screenshot.path) + `?t=${Date.now()}`;
    };
    const initialSelectedItem = initialSelectedId
      ? initialItems.find((i) => i.id === initialSelectedId) || null
      : null;

    img.onload = () => {
      imageRef.current = img;
      redraw(img, initialItems, null, initialSelectedItem);
    };
  }, [screenshot, stepAnnotationsJson, initialSelectedId]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if ((e.key === "Delete" || e.key === "Backspace") && selectedId) {
        const activeTag = document.activeElement?.tagName.toLowerCase();
        if (activeTag === "input" || activeTag === "textarea") return;
        e.preventDefault();
        deleteSelectedItem();
      } else if (e.key === "Escape") {
        setSelectedId(null);
        setActiveTool("select");
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [selectedId, items]);

  function deleteSelectedItem() {
    if (!selectedId) return;
    const next = items.filter((it) => it.id !== selectedId);
    setItems(next);
    setSelectedId(null);
    redraw(imageRef.current, next, null, null);
  }

  function redraw(
    img: HTMLImageElement | null,
    currentItems: AnnotationItem[],
    activeSelectedId: string | null,
    preview: AnnotationItem | null
  ) {
    const canvas = canvasRef.current;
    if (!canvas || !img) return;

    if (canvas.width !== img.naturalWidth || canvas.height !== img.naturalHeight) {
      canvas.width = img.naturalWidth;
      canvas.height = img.naturalHeight;
    }

    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(img, 0, 0);

    for (const item of currentItems) {
      drawItem(ctx, item, item.id === activeSelectedId);
    }

    if (preview) {
      drawItem(ctx, preview, false);
    }
  }

  function drawItem(ctx: CanvasRenderingContext2D, item: AnnotationItem, isSelected: boolean) {
    ctx.save();
    ctx.strokeStyle = item.color;
    ctx.lineWidth = item.strokeWidth;
    ctx.fillStyle = item.color;

    if (item.type === "click") {
      ctx.beginPath();
      ctx.arc(item.x, item.y, 28, 0, Math.PI * 2);
      ctx.fillStyle = hexToRgba(item.color, 0.28);
      ctx.fill();
      ctx.lineWidth = 3;
      ctx.strokeStyle = item.color;
      ctx.stroke();

      ctx.beginPath();
      ctx.arc(item.x, item.y, 16, 0, Math.PI * 2);
      ctx.lineWidth = 2.5;
      ctx.strokeStyle = "#ffffff";
      ctx.stroke();

      ctx.beginPath();
      ctx.arc(item.x, item.y, 6, 0, Math.PI * 2);
      ctx.fillStyle = item.color;
      ctx.fill();
      ctx.lineWidth = 2;
      ctx.strokeStyle = "#ffffff";
      ctx.stroke();

      if (isSelected) {
        drawSelectionBox(ctx, item.x - 34, item.y - 34, 68, 68);
      }
    } else if (item.type === "rect" && item.w !== undefined && item.h !== undefined) {
      const rx = item.w < 0 ? item.x + item.w : item.x;
      const ry = item.h < 0 ? item.y + item.h : item.y;
      const rw = Math.abs(item.w);
      const rh = Math.abs(item.h);

      ctx.strokeRect(rx, ry, rw, rh);
      if (isSelected) {
        drawSelectionBox(ctx, rx - 4, ry - 4, rw + 8, rh + 8);
      }
    } else if (item.type === "highlight" && item.w !== undefined && item.h !== undefined) {
      const rx = item.w < 0 ? item.x + item.w : item.x;
      const ry = item.h < 0 ? item.y + item.h : item.y;
      const rw = Math.abs(item.w);
      const rh = Math.abs(item.h);

      ctx.fillStyle = hexToRgba(item.color, 0.35);
      ctx.fillRect(rx, ry, rw, rh);
      ctx.strokeRect(rx, ry, rw, rh);

      if (isSelected) {
        drawSelectionBox(ctx, rx - 4, ry - 4, rw + 8, rh + 8);
      }
    } else if (item.type === "circle" && item.w !== undefined && item.h !== undefined) {
      const radiusX = Math.abs(item.w / 2);
      const radiusY = Math.abs(item.h / 2);
      const centerX = item.x + item.w / 2;
      const centerY = item.y + item.h / 2;

      ctx.beginPath();
      ctx.ellipse(centerX, centerY, radiusX, radiusY, 0, 0, Math.PI * 2);
      ctx.stroke();

      if (isSelected) {
        drawSelectionBox(ctx, centerX - radiusX - 4, centerY - radiusY - 4, radiusX * 2 + 8, radiusY * 2 + 8);
      }
    } else if (item.type === "badge") {
      const radius = 20;

      ctx.shadowColor = "rgba(0,0,0,0.5)";
      ctx.shadowBlur = 8;
      ctx.beginPath();
      ctx.arc(item.x, item.y, radius, 0, Math.PI * 2);
      ctx.fillStyle = item.color;
      ctx.fill();
      ctx.lineWidth = 2.5;
      ctx.strokeStyle = "#ffffff";
      ctx.stroke();
      ctx.shadowBlur = 0;

      ctx.fillStyle = "#ffffff";
      ctx.font = "bold 16px sans-serif";
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(String(item.badgeNumber || 1), item.x, item.y);

      if (item.badgeText && item.badgeText.trim()) {
        const textX = item.x + (item.textOffsetX ?? 28);
        const textY = item.y + (item.textOffsetY ?? -14);

        ctx.font = "bold 13.5px sans-serif";
        const metrics = ctx.measureText(item.badgeText);
        const padX = 8;
        const boxW = metrics.width + padX * 2;
        const boxH = 24;

        if (Math.hypot(item.textOffsetX ?? 28, item.textOffsetY ?? -14) > 35) {
          ctx.beginPath();
          ctx.moveTo(item.x, item.y);
          ctx.lineTo(textX, textY + boxH / 2);
          ctx.strokeStyle = item.color;
          ctx.lineWidth = 1.5;
          ctx.setLineDash([3, 3]);
          ctx.stroke();
          ctx.setLineDash([]);
        }

        ctx.fillStyle = "rgba(15, 23, 42, 0.92)";
        ctx.fillRect(textX, textY, boxW, boxH);
        ctx.strokeStyle = item.color;
        ctx.lineWidth = 1.5;
        ctx.strokeRect(textX, textY, boxW, boxH);

        ctx.fillStyle = "#ffffff";
        ctx.textAlign = "left";
        ctx.textBaseline = "middle";
        ctx.fillText(item.badgeText, textX + padX, textY + boxH / 2);

        if (isSelected) {
          ctx.strokeStyle = "#38bdf8";
          ctx.lineWidth = 1;
          ctx.setLineDash([2, 2]);
          ctx.strokeRect(textX - 2, textY - 2, boxW + 4, boxH + 4);
          ctx.setLineDash([]);
        }
      }

      if (isSelected) {
        drawSelectionBox(ctx, item.x - radius - 4, item.y - radius - 4, (radius + 4) * 2, (radius + 4) * 2);
      }
    } else if (item.type === "text" && item.text) {
      ctx.font = "bold 16px sans-serif";
      const metrics = ctx.measureText(item.text);
      const padding = 6;
      const textW = metrics.width + padding * 2;
      const textH = 26;

      ctx.fillStyle = "rgba(0, 0, 0, 0.8)";
      ctx.fillRect(item.x - padding, item.y - 18, textW, textH);
      ctx.strokeStyle = item.color;
      ctx.lineWidth = 1;
      ctx.strokeRect(item.x - padding, item.y - 18, textW, textH);

      ctx.fillStyle = item.color;
      ctx.textAlign = "left";
      ctx.textBaseline = "alphabetic";
      ctx.fillText(item.text, item.x, item.y);

      if (isSelected) {
        drawSelectionBox(ctx, item.x - padding - 3, item.y - 21, textW + 6, textH + 6);
      }
    }

    ctx.restore();
  }

  function drawSelectionBox(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number) {
    ctx.save();
    ctx.strokeStyle = "#06b6d4";
    ctx.lineWidth = 1.8;
    ctx.setLineDash([4, 4]);
    ctx.strokeRect(x, y, w, h);
    ctx.setLineDash([]);

    ctx.fillStyle = "#ffffff";
    const sz = 6;
    ctx.fillRect(x - sz / 2, y - sz / 2, sz, sz);
    ctx.fillRect(x + w - sz / 2, y - sz / 2, sz, sz);
    ctx.fillRect(x - sz / 2, y + h - sz / 2, sz, sz);
    ctx.fillRect(x + w - sz / 2, y + h - sz / 2, sz, sz);
    ctx.restore();
  }

  function hexToRgba(hex: string, alpha: number) {
    if (!hex.startsWith("#")) return `rgba(239, 68, 68, ${alpha})`;
    const h = hex.replace("#", "");
    const bigint = parseInt(h, 16);
    const r = (bigint >> 16) & 255;
    const g = (bigint >> 8) & 255;
    const b = bigint & 255;
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  }

  function getCanvasCoords(e: React.MouseEvent<HTMLCanvasElement>) {
    const canvas = canvasRef.current;
    if (!canvas) return { x: 0, y: 0 };
    const rect = canvas.getBoundingClientRect();
    const scaleX = canvas.width / rect.width;
    const scaleY = canvas.height / rect.height;
    return {
      x: Math.round((e.clientX - rect.left) * scaleX),
      y: Math.round((e.clientY - rect.top) * scaleY),
    };
  }

  function hitTest(x: number, y: number): { item: AnnotationItem | null; isBadgeText: boolean } {
    for (let i = items.length - 1; i >= 0; i--) {
      const it = items[i];

      if (it.type === "click") {
        const dist = Math.hypot(x - it.x, y - it.y);
        if (dist <= 30) return { item: it, isBadgeText: false };
      } else if (it.type === "badge") {
        if (it.badgeText && it.badgeText.trim()) {
          const textX = it.x + (it.textOffsetX ?? 28);
          const textY = it.y + (it.textOffsetY ?? -14);
          const textW = Math.max(80, it.badgeText.length * 8 + 16);
          const textH = 26;
          if (x >= textX && x <= textX + textW && y >= textY && y <= textY + textH) {
            return { item: it, isBadgeText: true };
          }
        }
        const dist = Math.hypot(x - it.x, y - it.y);
        if (dist <= 22) return { item: it, isBadgeText: false };
      } else if (it.type === "rect" || it.type === "highlight") {
        if (it.w !== undefined && it.h !== undefined) {
          const minX = Math.min(it.x, it.x + it.w);
          const maxX = Math.max(it.x, it.x + it.w);
          const minY = Math.min(it.y, it.y + it.h);
          const maxY = Math.max(it.y, it.y + it.h);
          if (x >= minX && x <= maxX && y >= minY && y <= maxY) {
            return { item: it, isBadgeText: false };
          }
        }
      } else if (it.type === "circle") {
        if (it.w !== undefined && it.h !== undefined) {
          const rx = Math.abs(it.w / 2);
          const ry = Math.abs(it.h / 2);
          const cx = it.x + it.w / 2;
          const cy = it.y + it.h / 2;
          const normalized = Math.pow(x - cx, 2) / Math.pow(rx, 2) + Math.pow(y - cy, 2) / Math.pow(ry, 2);
          if (normalized <= 1.2) {
            return { item: it, isBadgeText: false };
          }
        }
      } else if (it.type === "text" && it.text) {
        const textW = Math.max(60, it.text.length * 9 + 12);
        const textH = 26;
        if (x >= it.x - 6 && x <= it.x + textW && y >= it.y - 18 && y <= it.y + textH - 18) {
          return { item: it, isBadgeText: false };
        }
      }
    }
    return { item: null, isBadgeText: false };
  }

  function handleMouseDown(e: React.MouseEvent<HTMLCanvasElement>) {
    const coords = getCanvasCoords(e);
    setDragStartPos(coords);

    const hit = hitTest(coords.x, coords.y);

    if (activeTool === "select" || hit.item) {
      if (hit.item) {
        setSelectedId(hit.item.id);
        setInitialItemState({ ...hit.item });
        setIsInteracting(true);
        if (hit.isBadgeText) {
          setDragMode("move_badge_text");
        } else {
          setDragMode("move_item");
        }
        redraw(imageRef.current, items, hit.item.id, null);
        return;
      } else if (activeTool === "select") {
        setSelectedId(null);
        redraw(imageRef.current, items, null, null);
        return;
      }
    }

    if (activeTool === "click") {
      const withoutClick = items.filter((i) => i.type !== "click");
      const newClickItem: AnnotationItem = {
        id: `click_${Date.now()}`,
        type: "click",
        x: coords.x,
        y: coords.y,
        color: activeColor,
        strokeWidth: strokeWidth,
      };
      const updated = [...withoutClick, newClickItem];
      setItems(updated);
      setSelectedId(newClickItem.id);
      setActiveTool("select");
      redraw(imageRef.current, updated, newClickItem.id, null);
      return;
    }

    if (activeTool === "badge") {
      const newBadge: AnnotationItem = {
        id: `badge_${Date.now()}`,
        type: "badge",
        x: coords.x,
        y: coords.y,
        color: activeColor,
        strokeWidth: strokeWidth,
        badgeNumber: badgeCounter,
        badgeText: `Passo ${badgeCounter}`,
        textOffsetX: 28,
        textOffsetY: -12,
      };
      const updated = [...items, newBadge];
      setItems(updated);
      setBadgeCounter((prev) => prev + 1);
      setSelectedId(newBadge.id);
      setActiveTool("select");
      redraw(imageRef.current, updated, newBadge.id, null);
      return;
    }

    if (activeTool === "text") {
      const userText = prompt(t("annotation.prompt_text", "Enter annotation text:")) || "";
      if (!userText.trim()) return;
      const newTextItem: AnnotationItem = {
        id: `text_${Date.now()}`,
        type: "text",
        x: coords.x,
        y: coords.y,
        color: activeColor,
        strokeWidth: strokeWidth,
        text: userText.trim(),
      };
      const updated = [...items, newTextItem];
      setItems(updated);
      setSelectedId(newTextItem.id);
      setActiveTool("select");
      redraw(imageRef.current, updated, newTextItem.id, null);
      return;
    }

    setIsInteracting(true);
    setDragMode("draw");
  }

  function handleMouseMove(e: React.MouseEvent<HTMLCanvasElement>) {
    if (!isInteracting || !dragStartPos) return;
    const coords = getCanvasCoords(e);
    const dx = coords.x - dragStartPos.x;
    const dy = coords.y - dragStartPos.y;

    if (dragMode === "move_item" && selectedId && initialItemState) {
      const updated = items.map((it) => {
        if (it.id !== selectedId) return it;
        return {
          ...it,
          x: initialItemState.x + dx,
          y: initialItemState.y + dy,
        };
      });
      setItems(updated);
      redraw(imageRef.current, updated, selectedId, null);
    } else if (dragMode === "move_badge_text" && selectedId && initialItemState) {
      const initOffsetX = initialItemState.textOffsetX ?? 28;
      const initOffsetY = initialItemState.textOffsetY ?? -12;
      const updated = items.map((it) => {
        if (it.id !== selectedId) return it;
        return {
          ...it,
          textOffsetX: initOffsetX + dx,
          textOffsetY: initOffsetY + dy,
        };
      });
      setItems(updated);
      redraw(imageRef.current, updated, selectedId, null);
    } else if (dragMode === "draw") {
      const preview: AnnotationItem = {
        id: "temp_preview",
        type: activeTool,
        x: dragStartPos.x,
        y: dragStartPos.y,
        w: dx,
        h: dy,
        color: activeColor,
        strokeWidth,
      };
      redraw(imageRef.current, items, selectedId, preview);
    }
  }

  function handleMouseUp(e: React.MouseEvent<HTMLCanvasElement>) {
    if (!isInteracting) return;
    const coords = getCanvasCoords(e);
    setIsInteracting(false);

    if (dragMode === "draw" && dragStartPos) {
      const w = coords.x - dragStartPos.x;
      const h = coords.y - dragStartPos.y;

      if (Math.abs(w) > 5 || Math.abs(h) > 5) {
        const newItem: AnnotationItem = {
          id: `shape_${Date.now()}`,
          type: activeTool,
          x: dragStartPos.x,
          y: dragStartPos.y,
          w,
          h,
          color: activeColor,
          strokeWidth,
        };
        const updated = [...items, newItem];
        setItems(updated);
        setSelectedId(newItem.id);
        setActiveTool("select");
        redraw(imageRef.current, updated, newItem.id, null);
      } else {
        redraw(imageRef.current, items, selectedId, null);
      }
    }

    setDragMode(null);
    setDragStartPos(null);
    setInitialItemState(null);
  }

  function handleColorChange(newColor: string) {
    setActiveColor(newColor);
    if (selectedId) {
      const updated = items.map((it) => {
        if (it.id !== selectedId) return it;
        return { ...it, color: newColor };
      });
      setItems(updated);
      redraw(imageRef.current, updated, selectedId, null);
    }
  }

  function updateSelectedBadgeProps(props: Partial<AnnotationItem>) {
    if (!selectedId) return;
    const updated = items.map((it) => {
      if (it.id !== selectedId) return it;
      return { ...it, ...props };
    });
    setItems(updated);
    redraw(imageRef.current, updated, selectedId, null);
  }

  function handleReset() {
    setItems([]);
    setSelectedId(null);
    redraw(imageRef.current, [], null, null);
  }

  async function handleSave() {
    const canvas = canvasRef.current;
    if (!canvas) return;

    setSaving(true);
    setError(null);

    try {
      redraw(imageRef.current, items, null, null);

      const dataUrl = canvas.toDataURL("image/png");
      const base64 = dataUrl.replace(/^data:image\/\w+;base64,/, "");

      const clickItem = items.find((i) => i.type === "click");
      const finalClickX = clickItem ? Math.round(clickItem.x) : null;
      const finalClickY = clickItem ? Math.round(clickItem.y) : null;

      const annotationsJson = JSON.stringify(items);

      let newStepPayload: NewStepPayload | undefined = undefined;
      if (asNewStep) {
        newStepPayload = {
          title: stepTitle.trim() || "Nuovo passo",
          description: stepDescription.trim() || "Passaggio aggiunto manualmente dall'editor.",
        };
      }

      if (stepIndex != null && scopeToStep) {
        await api.saveStepAnnotations(sessionId, stepIndex, annotationsJson);
      }

      await api.saveAnnotatedScreenshot(
        sessionId,
        screenshot.id,
        base64,
        finalClickX,
        finalClickY,
        newStepPayload,
        annotationsJson
      );

      onSaved(screenshot.id);
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  const selectedItem = items.find((it) => it.id === selectedId);

  const tools: { id: ToolType; label: string; title: string }[] = [
    { id: "select", label: "↖ Seleziona e sposta", title: "Seleziona e sposta gli elementi con il mouse" },
    { id: "click", label: "🎯 Punto di click", title: "Posiziona o sposta il punto in cui è stato fatto click" },
    { id: "badge", label: `➊ Numero passo (#${badgeCounter})`, title: "Inserisci un numero di passo con un testo facoltativo" },
    { id: "rect", label: "▭ Rettangolo", title: "Disegna un rettangolo" },
    { id: "circle", label: "◯ Cerchio", title: "Disegna un cerchio" },
    { id: "highlight", label: "░ Evidenziatore", title: "Evidenzia un'area con un colore fluorescente" },
    { id: "text", label: "🔤 Testo libero", title: "Inserisci una didascalia" },
  ];

  const badgePositions = [
    { label: "➡ Destra", x: 28, y: -12 },
    { label: "⬅ Sinistra", x: -140, y: -12 },
    { label: "⬇ Sotto", x: -40, y: 28 },
    { label: "⬆ Sopra", x: -40, y: -48 },
  ];

  const canvasCursor =
    activeTool === "select" ? "tool-select" : activeTool === "badge" || activeTool === "text" ? "tool-place" : "tool-draw";

  return (
    <div className="anno-modal">
      <div className="anno-header">
        <div className="anno-header-info">
          <span className="anno-header-icon">🎨</span>
          <h3>Modifica screenshot e annotazioni</h3>
          {stepIndex != null && <span className="anno-step-pill">Passo {stepIndex}</span>}
          <span className="anno-count">
            {items.length} elementi · {selectedItem ? `Selezionato: ${ITEM_TYPE_LABELS[selectedItem.type] ?? selectedItem.type}` : "Nessuna selezione"}
          </span>
        </div>

        <div className="anno-header-actions">
          <AppButton
            size="sm"
            kind="ghost"
            onClick={deleteSelectedItem}
            disabled={!selectedId}
            title="Elimina l'elemento selezionato (Canc / Backspace)"
          >
            🗑️ Elimina elemento
          </AppButton>
          <AppButton size="sm" kind="ghost" onClick={handleReset} title="Rimuovi tutte le annotazioni">
            ↺ Ripristina
          </AppButton>
          <AppButton size="sm" kind="ghost" onClick={onClose}>
            Chiudi
          </AppButton>
          <AppButton size="sm" kind="primary" icon="save" onClick={handleSave} disabled={saving}>
            {saving ? "Salvataggio…" : "Salva modifiche"}
          </AppButton>
        </div>
      </div>

      {error ? (
        <div className="anno-error">
          <span>{error}</span>
          <button type="button" onClick={() => setError(null)} title="Chiudi">
            ✕
          </button>
        </div>
      ) : null}

      <div className="anno-body">
        <div className="anno-sidebar">
          {stepIndex != null && (
            <div className="anno-scope">
              <label className="anno-scope-label">
                <input type="checkbox" checked={scopeToStep} onChange={(e) => setScopeToStep(e.target.checked)} />
                <span>Associa al passo {stepIndex}</span>
              </label>
              <div className="anno-scope-note">Le annotazioni valgono solo per questo passo.</div>
            </div>
          )}

          <div>
            <div className="anno-section-title">Strumenti</div>
            <div className="anno-tools">
              {tools.map((tool) => (
                <button
                  key={tool.id}
                  type="button"
                  className={`btn btn-sm anno-tool ${activeTool === tool.id ? "btn-primary" : "btn-ghost"}`}
                  onClick={() => setActiveTool(tool.id)}
                  title={tool.title}
                >
                  {tool.label}
                </button>
              ))}
            </div>
          </div>

          <div>
            <div className="anno-section-title">Colore {selectedItem ? "(elemento)" : "(nuovo)"}</div>
            <div className="anno-palette">
              {PALETTE.map((c) => {
                const isCur = selectedItem ? selectedItem.color === c.val : activeColor === c.val;
                return (
                  <button
                    key={c.val}
                    type="button"
                    className={`anno-swatch${isCur ? " current" : ""}`}
                    onClick={() => handleColorChange(c.val)}
                    style={{ "--swatch": c.val } as CSSProperties}
                    title={c.label}
                  />
                );
              })}
            </div>
          </div>

          <div>
            <div className="anno-section-title">
              {t("annotation.stroke", "Thickness")}: {strokeWidth}px
            </div>
            <input
              type="range"
              className="anno-range"
              min="2"
              max="10"
              step="1"
              value={strokeWidth}
              onChange={(e) => setStrokeWidth(Number(e.target.value))}
            />
          </div>

          <div className="anno-footnote">
            💡 Clicca su un elemento per selezionarlo e spostarlo. Trascina il testo del numero per posizionarlo dove preferisci.
          </div>
        </div>

        <div className="anno-stage">
          <canvas
            ref={canvasRef}
            className={`anno-canvas ${canvasCursor}`}
            onMouseDown={handleMouseDown}
            onMouseMove={handleMouseMove}
            onMouseUp={handleMouseUp}
          />
        </div>

        <div className="anno-inspector">
          <div className="anno-inspector-block">
            <div className="anno-section-title">Elemento selezionato</div>

            {selectedItem ? (
              <div className="anno-stack">
                <div className="anno-selected-head">
                  <span>
                    Tipo: <span className="anno-type">{(ITEM_TYPE_LABELS[selectedItem.type] ?? selectedItem.type).toUpperCase()}</span>
                  </span>
                  <button type="button" className="btn btn-ghost btn-sm anno-delete" onClick={deleteSelectedItem}>
                    🗑️ Elimina
                  </button>
                </div>

                {selectedItem.type === "badge" && (
                  <div className="anno-stack tight">
                    <div>
                      <label className="anno-label">Numero del passo:</label>
                      <input
                        type="number"
                        className="anno-input"
                        min="1"
                        value={selectedItem.badgeNumber ?? 1}
                        onChange={(e) => updateSelectedBadgeProps({ badgeNumber: Number(e.target.value) })}
                      />
                    </div>

                    <div>
                      <label className="anno-label">Testo accanto al numero:</label>
                      <input
                        type="text"
                        className="anno-input"
                        placeholder="es. Clicca sul pulsante Salva"
                        value={selectedItem.badgeText ?? ""}
                        onChange={(e) => updateSelectedBadgeProps({ badgeText: e.target.value })}
                      />
                    </div>

                    <div>
                      <div className="anno-label">Posizione del testo:</div>
                      <div className="anno-positions">
                        {badgePositions.map((pos) => (
                          <button
                            key={pos.label}
                            type="button"
                            className="btn btn-ghost btn-sm anno-position"
                            onClick={() => updateSelectedBadgeProps({ textOffsetX: pos.x, textOffsetY: pos.y })}
                          >
                            {pos.label}
                          </button>
                        ))}
                      </div>
                      <div className="anno-hint">💡 Puoi anche trascinare il testo con il mouse direttamente sull'immagine.</div>
                    </div>
                  </div>
                )}

                {selectedItem.type === "click" && (
                  <div className="anno-muted">
                    Posizione click: <code>X: {Math.round(selectedItem.x)}, Y: {Math.round(selectedItem.y)}</code>. Trascinalo per
                    riposizionarlo o eliminalo per togliere il click da questo screenshot.
                  </div>
                )}

                {selectedItem.type === "text" && (
                  <div>
                    <label className="anno-label">Testo:</label>
                    <input
                      type="text"
                      className="anno-input"
                      value={selectedItem.text ?? ""}
                      onChange={(e) => {
                        const val = e.target.value;
                        const updated = items.map((it) => (it.id === selectedId ? { ...it, text: val } : it));
                        setItems(updated);
                        redraw(imageRef.current, updated, selectedId, null);
                      }}
                    />
                  </div>
                )}
              </div>
            ) : (
              <div className="anno-muted anno-empty">
                Nessun elemento selezionato. Clicca su una forma, un numero o il punto di click per modificarlo, spostarlo o cambiarne il colore.
              </div>
            )}
          </div>

          <div>
            <div className="anno-section-title">Inserimento nella guida</div>

            <label className="anno-newstep">
              <input type="checkbox" checked={asNewStep} onChange={(e) => setAsNewStep(e.target.checked)} />
              <div>
                <div className="anno-newstep-title">Aggiungi come nuovo passo</div>
                <div className="anno-note">
                  Inserisce questa immagine modificata come nuovo passo nella guida
                </div>
              </div>
            </label>

            {asNewStep && (
              <div className="anno-stack tight anno-newstep-fields">
                <div>
                  <label className="anno-label">Titolo del passo:</label>
                  <input type="text" className="anno-input" value={stepTitle} onChange={(e) => setStepTitle(e.target.value)} />
                </div>

                <div>
                  <label className="anno-label">Descrizione del passo:</label>
                  <textarea
                    rows={3}
                    className="anno-input anno-textarea"
                    value={stepDescription}
                    onChange={(e) => setStepDescription(e.target.value)}
                    placeholder="Descrivi l'operazione in dettaglio…"
                  />
                </div>
              </div>
            )}
          </div>

          <div className="anno-footnote">
            💾 I dati vettoriali delle modifiche rimangono salvati e modificabili in ogni momento.
          </div>
        </div>
      </div>
    </div>
  );
}
