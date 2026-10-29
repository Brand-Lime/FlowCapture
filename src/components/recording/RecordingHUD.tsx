import { useEffect, useState } from "react";
import { AppButton } from "@/components/ui/AppButton";
import { Icon } from "@/components/ui/Icon";
import { api, type Session } from "@/lib/api";
import { recordedSeconds, type PauseClock } from "@/hooks/useRecording";
import { formatDuration } from "@/lib/utils";

type FeedItem = {
  id: string;
  ts: string;
  k: string;
  v: string;
};

type RecordingHUDProps = {
  session: Session;
  loading: boolean;
  onStop: () => void;
  onMarkStep: () => void;
  recordAudio?: boolean;
  selectedMicId?: string;
  audioDevices?: MediaDeviceInfo[];
  onSwitchMicrophone?: (micId: string) => void;
  isMuted?: boolean;
  onToggleMute?: () => void;
  isPaused?: boolean;
  pauseClock?: PauseClock;
  onPause?: () => void;
  onResume?: () => void;
  monitors?: { id: string; name: string; is_primary: boolean }[];
  selectedMonitorId?: string;
  onSwitchMonitor?: (monitorId: string) => void;
  highlightClicks?: boolean;
  selectedMonitorName?: string;
  /** The microphone stopped working during the recording. */
  micProblem?: string | null;
};

const NO_PAUSE_CLOCK: PauseClock = { pausedMs: 0, pausedAt: null };

export function RecordingHUD({
  session,
  loading,
  onStop,
  onMarkStep,
  recordAudio = false,
  selectedMicId,
  audioDevices = [],
  onSwitchMicrophone,
  isMuted = false,
  onToggleMute,
  isPaused = false,
  pauseClock = NO_PAUSE_CLOCK,
  onPause,
  onResume,
  monitors = [],
  selectedMonitorId,
  onSwitchMonitor,
  highlightClicks = true,
  selectedMonitorName,
  micProblem,
}: RecordingHUDProps) {
  const [elapsed, setElapsed] = useState(0);
  const [eventsCount, setEventsCount] = useState(0);
  const [shotsCount, setShotsCount] = useState(0);
  const [feed, setFeed] = useState<FeedItem[]>([]);

  useEffect(() => {
    function tick() {
      setElapsed(recordedSeconds(session.started_at, pauseClock));
    }

    tick();
    const timer = window.setInterval(tick, 1000);
    return () => window.clearInterval(timer);
  }, [session.started_at, pauseClock]);

  useEffect(() => {
    async function pollStats() {
      try {
        const [shots, evts] = await Promise.all([
          api.listScreenshots(session.id),
          api.listEvents(session.id),
        ]);
        setShotsCount(shots.length);
        setEventsCount(evts.length);

        const recentFeed: FeedItem[] = evts.slice(-5).reverse().map((e) => {
          let val = e.app_name || "System";
          try {
            const parsed = JSON.parse(e.payload);
            if (parsed.combo) {
              val = `${val} · ⌨️ ${parsed.combo}`;
            } else if (parsed.double_click) {
              val = `${val} · ⚡ Double Click (${parsed.x}, ${parsed.y})`;
            } else if (parsed.button) {
              val = `${val} · ${parsed.button} (${parsed.x}, ${parsed.y})`;
            } else if (parsed.key) {
              val = `${val} · ${parsed.key}`;
            } else if (parsed.title) {
              val = `${val} · "${parsed.title}"`;
            }
          } catch {
            // fallback
          }

          return {
            id: e.id,
            ts: new Date(e.timestamp_ms).toLocaleTimeString("en-US", { hour12: false }),
            k: e.event_type.replaceAll("_", " "),
            v: val,
          };
        });

        if (recentFeed.length > 0) {
          setFeed(recentFeed);
        }
      } catch {
        // quiet fallback
      }
    }

    void pollStats();
    const statsTimer = window.setInterval(() => {
      void pollStats();
    }, 1500);

    return () => {
      window.clearInterval(statsTimer);
    };
  }, [session.id]);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (
        (event.metaKey || event.ctrlKey) &&
        event.shiftKey &&
        event.key.toLowerCase() === "r"
      ) {
        event.preventDefault();
        if (!loading) onStop();
      }
      if (
        (event.metaKey || event.ctrlKey) &&
        event.shiftKey &&
        event.key.toLowerCase() === "m"
      ) {
        event.preventDefault();
        onMarkStep();
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [loading, onMarkStep, onStop]);

  return (
    <>
      <div className="scrim" />
      <div className="hud">
        <div className="hud-top">
          <div className="hud-rec">
            <span className="pulse" />
            <div>
              <div className="hr-t">Recording session</div>
              <div className="hr-s" style={{ display: "flex", gap: "8px", alignItems: "center" }}>
                <span>Capturing screen, input & windows</span>
                {recordAudio ? (
                  <span
                    style={{
                      background: "rgba(34,197,94,0.2)",
                      color: "#4ade80",
                      padding: "1px 6px",
                      borderRadius: "4px",
                      fontSize: "11px",
                      fontWeight: 600,
                    }}
                  >
                    🎙️ {audioDevices.find((d) => d.deviceId === selectedMicId)?.label || "Mic on"}
                  </span>
                ) : null}
                {highlightClicks ? (
                  <span
                    style={{
                      background: "rgba(245,158,11,0.2)",
                      color: "#fbbf24",
                      padding: "1px 6px",
                      borderRadius: "4px",
                      fontSize: "11px",
                      fontWeight: 600,
                    }}
                  >
                    🎯 Click highlight
                  </span>
                ) : null}
              </div>
            </div>
          </div>
          <div className="hud-timer">{formatDuration(elapsed)}</div>
          <div className="hud-stats">
            <div className="hud-stat">
              <div className="hv">{eventsCount.toLocaleString()}</div>
              <div className="hl">events</div>
            </div>
            <div className="hud-stat">
              <div className="hv">{shotsCount}</div>
              <div className="hl">screenshots</div>
            </div>
          </div>
        </div>
        {micProblem ? <div className="hud-warning">🎙️ {micProblem}</div> : null}
        {recordAudio && isMuted && !micProblem ? (
          <div className="hud-warning">🔇 Il microfono è in muto: la tua voce non viene registrata.</div>
        ) : null}
        <div className="hud-feed">
          {feed.map((item) => (
            <div className="fl" key={item.id}>
              <span className="ft">{item.ts}</span>
              <span className="fk">{item.k}</span>
              <span className="fv">{item.v}</span>
            </div>
          ))}
          {feed.length === 0 ? (
            <div className="fl">
              <span className="ft">--:--:--</span>
              <span className="fk">listening…</span>
              <span className="fv">waiting for activity</span>
            </div>
          ) : null}
        </div>
        <div className="hud-foot" style={{ flexWrap: "wrap", gap: "8px" }}>
          {/* Monitor Live Switch */}
          {monitors.length > 1 ? (
            <div style={{ display: "flex", alignItems: "center", gap: "5px" }}>
              <Icon name="monitor" size={15} />
              <select
                className="select"
                value={selectedMonitorId}
                onChange={(e) => onSwitchMonitor?.(e.target.value)}
                style={{ fontSize: "12px", padding: "3px 6px", height: "auto" }}
                title="Cambia schermo in tempo reale"
              >
                {monitors.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name} {m.is_primary ? "(Principale)" : ""}
                  </option>
                ))}
              </select>
            </div>
          ) : (
            <span className="src">
              <Icon name="monitor" size={15} /> {selectedMonitorName || "Primary Display"}
            </span>
          )}

          {/* Microphone Live Switch & Mute */}
          {recordAudio ? (
            <div style={{ display: "flex", alignItems: "center", gap: "5px" }}>
              <button
                type="button"
                onClick={onToggleMute}
                title={isMuted ? "Riattiva Microfono" : "Muta Microfono"}
                style={{
                  background: isMuted ? "#ef4444" : "rgba(255,255,255,0.1)",
                  color: isMuted ? "#fff" : "currentColor",
                  border: "none",
                  borderRadius: "4px",
                  padding: "4px 8px",
                  cursor: "pointer",
                  display: "flex",
                  alignItems: "center",
                  gap: "4px",
                  fontSize: "12px",
                }}
              >
                <Icon name={isMuted ? "micOff" : "mic"} size={14} />
                <span>{isMuted ? "Muto" : "Attivo"}</span>
              </button>

              {audioDevices.length > 1 ? (
                <select
                  className="select"
                  value={selectedMicId}
                  onChange={(e) => onSwitchMicrophone?.(e.target.value)}
                  style={{ fontSize: "12px", padding: "3px 6px", height: "auto", maxWidth: "140px" }}
                  title="Cambia microfono in tempo reale"
                >
                  {audioDevices.map((d) => (
                    <option key={d.deviceId} value={d.deviceId}>
                      {d.label || "Microfono"}
                    </option>
                  ))}
                </select>
              ) : null}
            </div>
          ) : null}

          {/* Action buttons */}
          <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: "8px" }}>
            <AppButton
              size="sm"
              icon={isPaused ? "play" : "pause"}
              kind="ghost"
              disabled={loading}
              onClick={isPaused ? onResume : onPause}
              style={{
                background: isPaused ? "rgba(234, 179, 8, 0.2)" : "rgba(255, 255, 255, 0.08)",
                color: isPaused ? "#facc15" : "inherit",
              }}
            >
              {isPaused ? "Riprendi" : "Pausa"}
            </AppButton>

            <AppButton
              size="sm"
              icon="camera"
              disabled={loading || isPaused}
              onClick={() => onMarkStep()}
            >
              Mark Step (⌘⇧M)
            </AppButton>

            <AppButton
              kind="ghost"
              icon="stop"
              disabled={loading}
              onClick={() => onStop()}
              style={{ background: "#ff5f57", color: "#3a0b09", border: "none" }}
            >
              Stop & Process
            </AppButton>
          </div>
        </div>
      </div>
    </>
  );
}
