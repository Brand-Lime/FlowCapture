import { convertFileSrc } from "@tauri-apps/api/core";
import { useCallback, useMemo, useState } from "react";
import { AppButton } from "@/components/ui/AppButton";
import { Icon } from "@/components/ui/Icon";
import type { Screenshot, SessionEvent } from "@/lib/api";
import { timelineIcon } from "@/lib/icons";

interface TimelineTabProps {
  events: SessionEvent[];
  screenshots: Screenshot[];
  onOpenScreenshot: (shot: Screenshot) => void;
}

/** "Timeline" tab of a session: compressed events, each with the screenshots taken around it. */
export function TimelineTab({ events, screenshots, onOpenScreenshot }: TimelineTabProps) {
  const [hideAllScreenshots, setHideAllScreenshots] = useState(false);
  const [collapsedEvents, setCollapsedEvents] = useState<Record<number, boolean>>({});

  const eventScreenshotsMap = useMemo(() => {
    const map = new Map<number, Screenshot[]>();
    if (!events.length) return map;

    for (let i = 0; i < events.length; i++) {
      map.set(i, []);
    }

    for (const shot of screenshots) {
      let targetIdx = -1;
      for (let i = 0; i < events.length; i++) {
        const currentStart = i === 0 ? -Infinity : events[i].timestamp_ms - 150;
        const nextStart = i < events.length - 1 ? events[i + 1].timestamp_ms - 150 : Infinity;
        if (shot.timestamp_ms >= currentStart && shot.timestamp_ms < nextStart) {
          targetIdx = i;
          break;
        }
      }
      if (targetIdx === -1) {
        targetIdx = events.length - 1;
      }
      map.get(targetIdx)?.push(shot);
    }
    return map;
  }, [events, screenshots]);

  const isEventCollapsed = useCallback(
    (index: number) => {
      if (collapsedEvents[index] !== undefined) {
        return collapsedEvents[index];
      }
      return hideAllScreenshots;
    },
    [collapsedEvents, hideAllScreenshots],
  );

  const toggleEventScreenshots = useCallback(
    (index: number) => {
      setCollapsedEvents((prev) => {
        const current = prev[index] !== undefined ? prev[index] : hideAllScreenshots;
        return { ...prev, [index]: !current };
      });
    },
    [hideAllScreenshots],
  );

  const toggleAllScreenshots = useCallback(() => {
    setHideAllScreenshots((prev) => {
      const next = !prev;
      setCollapsedEvents({});
      return next;
    });
  }, []);

  return (
    <div className="card panel">
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginBottom: "14px",
          flexWrap: "wrap",
          gap: "10px",
        }}
      >
        <div>
          <h3 style={{ margin: 0 }}>Cronologia degli eventi ({events.length})</h3>
          <div className="pd">
            {events.length} eventi registrati · {screenshots.length} screenshot associati.
          </div>
        </div>
        {screenshots.length > 0 && (
          <AppButton
            size="sm"
            kind="ghost"
            icon={hideAllScreenshots ? "eye" : "eyeOff"}
            onClick={toggleAllScreenshots}
            title={hideAllScreenshots ? "Mostra tutti gli screenshot per gli eventi" : "Nascondi tutti gli screenshot per gli eventi"}
          >
            {hideAllScreenshots ? "Mostra tutti gli screenshot" : "Nascondi tutti gli screenshot"}
          </AppButton>
        )}
      </div>

      <div className="tl-list">
        {events.length === 0 ? (
          <div className="pd">Nessun evento registrato.</div>
        ) : (
          events.map((event, index) => {
            const eventShots = eventScreenshotsMap.get(index) ?? [];
            const isCollapsed = isEventCollapsed(index);

            return (
              <div
                key={`${event.timestamp_ms}-${index}`}
                className={`tl-evt${event.event_type.includes("click") ? " click" : ""}`}
              >
                <div className="tl-evt-header">
                  <span className="tk">
                    <Icon name={timelineIcon(event.event_type)} size={17} />
                  </span>
                  <div className="tinfo">
                    <div className="tt">{event.event_type.replaceAll("_", " ")}</div>
                    <div className="td">
                      {event.app_name ?? "App sconosciuta"} · {JSON.stringify(event.payload)}
                    </div>
                  </div>

                  <div className="tl-evt-meta">
                    {eventShots.length > 0 ? (
                      <button
                        type="button"
                        className={`tl-evt-toggle-btn${isCollapsed ? " collapsed" : ""}`}
                        onClick={() => toggleEventScreenshots(index)}
                        title={isCollapsed ? "Mostra gli screenshot di questo evento" : "Nascondi gli screenshot di questo evento"}
                      >
                        <Icon name="camera" size={13} />
                        <span>
                          {eventShots.length} {eventShots.length === 1 ? "screenshot" : "screenshot"}
                        </span>
                        <Icon name={isCollapsed ? "chevronDown" : "chevronUp"} size={12} />
                      </button>
                    ) : (
                      <span className="tl-evt-no-shots">Nessuno screenshot</span>
                    )}

                    <div className="ttime">{event.timestamp_ms}ms</div>
                  </div>
                </div>

                {!isCollapsed && eventShots.length > 0 && (
                  <div className="tl-evt-shots">
                    {eventShots.map((shot) => {
                      const hasClick = shot.click_x != null && shot.click_y != null;
                      let annotationsCount = 0;
                      if (shot.annotations_json) {
                        try {
                          const parsed = JSON.parse(shot.annotations_json);
                          if (Array.isArray(parsed)) annotationsCount = parsed.length;
                        } catch {}
                      }
                      const deltaMs = shot.timestamp_ms - event.timestamp_ms;
                      const timeLabel =
                        deltaMs === 0
                          ? `${shot.timestamp_ms}ms`
                          : deltaMs > 0
                          ? `+${deltaMs}ms`
                          : `${deltaMs}ms`;

                      return (
                        <div
                          key={shot.id}
                          className="tl-shot-card"
                          onClick={() => onOpenScreenshot(shot)}
                          title="Clicca per aprire lo screenshot a schermo intero o annotare"
                        >
                          <div className="tl-shot-thumb">
                            <img
                              src={convertFileSrc(shot.path)}
                              alt={shot.trigger ?? "Screenshot evento"}
                              loading="lazy"
                              onError={(e) => {
                                (e.target as HTMLImageElement).style.display = "none";
                              }}
                            />
                            <div className="tl-shot-badge">
                              <span>{shot.trigger?.replaceAll("_", " ") ?? "screenshot"}</span>
                            </div>
                            {hasClick && (
                              <div
                                className="tl-shot-ann-badge"
                                style={{ background: "rgba(16, 185, 129, 0.9)" }}
                                title="Punto di click registrato"
                              >
                                🎯 Click
                              </div>
                            )}
                            {annotationsCount > 0 && (
                              <div
                                className="tl-shot-ann-badge"
                                style={{
                                  left: hasClick ? "65px" : "5px",
                                }}
                                title={`${annotationsCount} annotazioni`}
                              >
                                ✏️ {annotationsCount}
                              </div>
                            )}
                            <div className="tl-shot-hover-action">
                              <span className="tl-shot-hover-btn">
                                <Icon name="edit" size={12} />
                                <span>Modifica</span>
                              </span>
                            </div>
                          </div>
                          <div className="tl-shot-footer">
                            <span className="tl-shot-label" title={shot.trigger ?? "screenshot"}>
                              {shot.trigger?.replaceAll("_", " ") ?? "screenshot"}
                            </span>
                            <span className="tl-shot-time" title={`Istante assoluto: ${shot.timestamp_ms} ms`}>
                              {timeLabel}
                            </span>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
