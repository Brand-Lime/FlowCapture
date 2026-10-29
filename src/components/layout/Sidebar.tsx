import { NavLink, useLocation, useNavigate } from "react-router-dom";
import { Icon } from "@/components/ui/Icon";
import { Logo } from "@/components/ui/Logo";
import { useSessionsContext } from "@/context/SessionsContext";
import { statusDotClass, statusLabel } from "@/lib/icons";
import { formatDuration } from "@/lib/utils";

interface SidebarProps {
  onClose?: () => void;
}

export function Sidebar({ onClose }: SidebarProps) {
  const navigate = useNavigate();
  const location = useLocation();
  const { sessions } = useSessionsContext();

  return (
    <aside className="side">
      <div className="logo" style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <div style={{ display: "flex", alignItems: "center", gap: "12px" }}>
          <Logo />
          <div>
            <div className="lt">FlowCapture</div>
            <div className="ls">Guide passo-passo</div>
          </div>
        </div>
        {onClose && (
          <button
            type="button"
            className="btn-collapse-sidebar"
            onClick={onClose}
            title="Chiudi barra laterale"
          >
            ◀
          </button>
        )}
      </div>

      <NavLink
        to="/"
        end
        className={({ isActive }) => `nav-i${isActive ? " active" : ""}`}
      >
        <Icon name="home" /> Home
      </NavLink>
      <NavLink
        to="/settings"
        className={({ isActive }) => `nav-i${isActive ? " active" : ""}`}
      >
        <Icon name="settings" /> Impostazioni
      </NavLink>

      <div className="side-sessions">
        <div className="sh">
          <span>Sessioni</span>
          <span>{sessions.length}</span>
        </div>
        <div className="side-sessions-list">
          {sessions.map((session) => (
            <div
              key={session.id}
              className={`ss-item${location.pathname === `/sessions/${session.id}` ? " active" : ""}`}
              role="button"
              tabIndex={0}
              onClick={() => navigate(`/sessions/${session.id}`)}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  navigate(`/sessions/${session.id}`);
                }
              }}
            >
              <div className="st">{session.title}</div>
              <div className="sm">
                <span className={`dotstat ${statusDotClass(session.status)}`} />
                {statusLabel(session.status)} · {formatDuration(session.duration)}
              </div>
            </div>
          ))}
        </div>
      </div>

      <div className="side-foot">
        <div className="ft">
          <Icon name="shield" size={15} />
          <span>
            <b style={{ color: "var(--text-2)" }}>Dati in sede.</b> Sessioni, screenshot ed
            esportazioni restano su questo computer e nella cartella sessioni.
          </span>
        </div>
      </div>
    </aside>
  );
}
