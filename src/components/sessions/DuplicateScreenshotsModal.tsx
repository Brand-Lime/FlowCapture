import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { AppButton } from "@/components/ui/AppButton";
import type { DuplicateScreenshotGroup, Screenshot } from "@/lib/api";
import { SimilarityThreshold } from "./SimilarityThreshold";

interface DuplicateScreenshotsModalProps {
  groups: DuplicateScreenshotGroup[];
  screenshots: Screenshot[];
  onMerge: (keepId: string, removeIds: string[]) => Promise<void>;
  onClose: () => void;
  /** Threshold (%) the groups were computed with. */
  minSimilarity: number;
  /** Re-runs the search with another threshold. */
  onRescan: (minSimilarity: number) => Promise<void>;
  rescanning?: boolean;
}

export function DuplicateScreenshotsModal({
  groups,
  screenshots,
  onMerge,
  onClose,
  minSimilarity,
  onRescan,
  rescanning = false,
}: DuplicateScreenshotsModalProps) {
  const [threshold, setThreshold] = useState(minSimilarity);
  const [activeGroupIndex, setActiveGroupIndex] = useState(0);
  const [merging, setMerging] = useState(false);
  const [remainingGroups, setRemainingGroups] = useState<DuplicateScreenshotGroup[]>(groups);

  const currentGroup = remainingGroups[activeGroupIndex] || remainingGroups[0];

  // For the current group: keepId defaults to the first screenshot
  const [keepId, setKeepId] = useState<string>(
    currentGroup?.screenshot_ids[0] || ""
  );

  // Set of IDs selected for removal (defaults to all others in the group)
  const [removeIds, setRemoveIds] = useState<Set<string>>(
    () => new Set(currentGroup?.screenshot_ids.slice(1) || [])
  );

  const handleGroupChange = (idx: number) => {
    setActiveGroupIndex(idx);
    const grp = remainingGroups[idx];
    if (grp && grp.screenshot_ids.length > 0) {
      const first = grp.screenshot_ids[0];
      setKeepId(first);
      setRemoveIds(new Set(grp.screenshot_ids.slice(1)));
    }
  };

  const handleSetKeep = (id: string) => {
    setKeepId(id);
    setRemoveIds((prev) => {
      const next = new Set(prev);
      next.delete(id);
      // Ensure other screenshot ids are in removeIds
      currentGroup?.screenshot_ids.forEach((sid) => {
        if (sid !== id) next.add(sid);
      });
      return next;
    });
  };

  const handleToggleRemove = (id: string) => {
    if (id === keepId) return; // cannot remove the kept screenshot
    setRemoveIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const handleMergeCurrent = async () => {
    if (!currentGroup || !keepId || removeIds.size === 0) return;
    setMerging(true);
    try {
      await onMerge(keepId, Array.from(removeIds));

      // Remove this group from remaining
      const nextGroups = remainingGroups.filter((g) => g.group_id !== currentGroup.group_id);
      setRemainingGroups(nextGroups);

      if (nextGroups.length === 0) {
        onClose();
      } else {
        const nextIdx = Math.min(activeGroupIndex, nextGroups.length - 1);
        handleGroupChange(nextIdx);
      }
    } finally {
      setMerging(false);
    }
  };

  if (!currentGroup || remainingGroups.length === 0) {
    return null;
  }

  const groupScreenshots = currentGroup.screenshot_ids
    .map((id) => screenshots.find((s) => s.id === id))
    .filter((s): s is Screenshot => Boolean(s));

  return (
    <div
      style={{
        position: "fixed",
        top: 0,
        left: 0,
        width: "100vw",
        height: "100vh",
        backgroundColor: "rgba(0, 0, 0, 0.85)",
        zIndex: 9999,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        backdropFilter: "blur(6px)",
        padding: "24px",
      }}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        style={{
          width: "92vw",
          maxWidth: "1200px",
          height: "88vh",
          background: "var(--surface)",
          border: "1px solid var(--hair-2)",
          borderRadius: "14px",
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
          boxShadow: "0 24px 60px rgba(0,0,0,0.7)",
        }}
      >
        {/* Header */}
        <div
          style={{
            padding: "16px 24px",
            borderBottom: "1px solid var(--hair)",
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
            background: "var(--bg-2)",
          }}
        >
          <div>
            <div style={{ display: "flex", alignItems: "center", gap: "10px" }}>
              <span style={{ fontSize: "18px" }}>🔍</span>
              <h3 style={{ margin: 0, fontSize: "16px", fontWeight: 600 }}>
                Screenshot duplicati (somiglianza ≥ {minSimilarity}%)
              </h3>
              <span
                style={{
                  fontSize: "12px",
                  padding: "2px 8px",
                  background: "rgba(95, 233, 184, 0.15)",
                  color: "var(--mint)",
                  borderRadius: "10px",
                  fontWeight: 600,
                }}
              >
                {remainingGroups.length} {remainingGroups.length === 1 ? "gruppo" : "gruppi"}
              </span>
            </div>
            <div style={{ fontSize: "12px", color: "var(--dim)", marginTop: "4px" }}>
              Seleziona quale screenshot tenere e quali unire/eliminare. I riferimenti nei passaggi e nella guida verranno reindirizzati all'immagine mantenuta.
            </div>
          </div>

          <div style={{ display: "flex", gap: "10px", alignItems: "center", flexWrap: "wrap", justifyContent: "flex-end" }}>
            <SimilarityThreshold value={threshold} onChange={setThreshold} disabled={rescanning || merging} />
            <AppButton
              size="sm"
              kind="ghost"
              disabled={rescanning || merging || threshold === minSimilarity}
              onClick={() => onRescan(threshold)}
              title="Ripeti la ricerca con la nuova soglia"
            >
              {rescanning ? "Ricalcolo…" : "Ricalcola"}
            </AppButton>
            <AppButton size="sm" kind="ghost" onClick={onClose}>
              Annulla
            </AppButton>
            <AppButton
              size="sm"
              kind="primary"
              disabled={merging || removeIds.size === 0}
              onClick={handleMergeCurrent}
            >
              {merging ? "Unione in corso…" : `Unisci ed elimina selezionati (${removeIds.size})`}
            </AppButton>
          </div>
        </div>

        {/* Group Selector Bar if > 1 */}
        {remainingGroups.length > 1 && (
          <div
            style={{
              display: "flex",
              gap: "8px",
              padding: "10px 24px",
              background: "rgba(255,255,255,0.02)",
              borderBottom: "1px solid var(--hair)",
              overflowX: "auto",
            }}
          >
            {remainingGroups.map((g, idx) => (
              <button
                key={g.group_id}
                type="button"
                onClick={() => handleGroupChange(idx)}
                style={{
                  padding: "6px 14px",
                  borderRadius: "6px",
                  fontSize: "12px",
                  fontWeight: 500,
                  cursor: "pointer",
                  border: activeGroupIndex === idx ? "1px solid var(--mint)" : "1px solid var(--hair)",
                  background: activeGroupIndex === idx ? "rgba(95, 233, 184, 0.15)" : "transparent",
                  color: activeGroupIndex === idx ? "var(--mint)" : "var(--dim)",
                }}
              >
                Gruppo {idx + 1} ({g.screenshot_ids.length} screenshot · {g.similarity_pct.toFixed(1)}%)
              </button>
            ))}
          </div>
        )}

        {/* Group Info Bar */}
        <div
          style={{
            padding: "10px 24px",
            background: "rgba(0,0,0,0.2)",
            borderBottom: "1px solid var(--hair)",
            display: "flex",
            justifyContent: "space-between",
            alignItems: "center",
          }}
        >
          <div style={{ fontSize: "13px", fontWeight: 600, color: "var(--text)" }}>
            Confronto gruppo {activeGroupIndex + 1}: {groupScreenshots.length} screenshot simili
          </div>
          <div style={{ fontSize: "12px", color: "#34d399", fontWeight: 600 }}>
            Somiglianza stimata: ~{currentGroup.similarity_pct.toFixed(1)}%
          </div>
        </div>

        {/* Content: Side-by-side or Grid comparison */}
        <div
          style={{
            flex: 1,
            overflowY: "auto",
            padding: "20px 24px",
            display: "grid",
            gridTemplateColumns: `repeat(${Math.min(groupScreenshots.length, 3)}, 1fr)`,
            gap: "20px",
          }}
        >
          {groupScreenshots.map((s, idx) => {
            const isKeep = s.id === keepId;
            const isRemove = removeIds.has(s.id);

            return (
              <div
                key={s.id}
                style={{
                  display: "flex",
                  flexDirection: "column",
                  borderRadius: "10px",
                  overflow: "hidden",
                  border: isKeep
                    ? "2px solid #34d399"
                    : isRemove
                    ? "2px solid #f87171"
                    : "1px solid var(--hair)",
                  background: isKeep
                    ? "rgba(52, 211, 153, 0.05)"
                    : isRemove
                    ? "rgba(248, 113, 113, 0.05)"
                    : "var(--bg-2)",
                  boxShadow: "0 6px 16px rgba(0,0,0,0.3)",
                  transition: "all 0.15s",
                }}
              >
                {/* Status Header */}
                <div
                  style={{
                    padding: "10px 14px",
                    display: "flex",
                    justifyContent: "space-between",
                    alignItems: "center",
                    borderBottom: "1px solid var(--hair)",
                    background: isKeep
                      ? "rgba(52, 211, 153, 0.15)"
                      : isRemove
                      ? "rgba(248, 113, 113, 0.15)"
                      : "transparent",
                  }}
                >
                  <label
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: "8px",
                      cursor: "pointer",
                      fontWeight: 600,
                      fontSize: "13px",
                      color: isKeep ? "#34d399" : "var(--text)",
                    }}
                  >
                    <input
                      type="radio"
                      name={`keep_${currentGroup.group_id}`}
                      checked={isKeep}
                      onChange={() => handleSetKeep(s.id)}
                    />
                    <span>{isKeep ? "⭐ DA TENERE" : `Opzione ${idx + 1}`}</span>
                  </label>

                  {!isKeep && (
                    <label
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: "6px",
                        cursor: "pointer",
                        fontSize: "12px",
                        color: isRemove ? "#f87171" : "var(--dim)",
                      }}
                    >
                      <input
                        type="checkbox"
                        checked={isRemove}
                        onChange={() => handleToggleRemove(s.id)}
                      />
                      <span>Unisci ed elimina</span>
                    </label>
                  )}
                </div>

                {/* Image */}
                <div
                  style={{
                    position: "relative",
                    flex: 1,
                    minHeight: "220px",
                    background: "#080c14",
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    padding: "10px",
                  }}
                >
                  <img
                    src={convertFileSrc(s.path)}
                    alt={`Screenshot ${idx + 1}`}
                    style={{
                      maxWidth: "100%",
                      maxHeight: "360px",
                      objectFit: "contain",
                      borderRadius: "6px",
                    }}
                  />
                </div>

                {/* Metadata Footer */}
                <div
                  style={{
                    padding: "8px 14px",
                    borderTop: "1px solid var(--hair)",
                    fontSize: "11px",
                    color: "var(--dim)",
                    display: "flex",
                    justifyContent: "space-between",
                  }}
                >
                  <span>Evento: {s.trigger || "click"}</span>
                  <span>ID: {s.id.slice(0, 8)}...</span>
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
