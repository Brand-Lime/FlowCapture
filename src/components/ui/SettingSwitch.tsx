/** Label + description on the left, a two-option segmented switch on the right. */
export function SettingSwitch({
  label,
  description,
  value,
  onLabel = "Attivo",
  offLabel = "Disattivo",
  onChange,
  narrow = false,
  emoji,
  className,
}: {
  label: string;
  description?: string;
  value: boolean;
  onLabel?: string;
  offLabel?: string;
  onChange: (next: boolean) => void | Promise<void>;
  narrow?: boolean;
  /** Optional emoji shown before the label (recording options on the home page). */
  emoji?: string;
  className?: string;
}) {
  return (
    <div className={`field setting-row${narrow ? " narrow" : ""}${className ? ` ${className}` : ""}`}>
      <div className="setting-label">
        {emoji ? <span className="setting-emoji">{emoji}</span> : null}
        <div>
          <label>{label}</label>
          {description ? <div className="setting-desc">{description}</div> : null}
        </div>
      </div>
      <div className="seg">
        <button type="button" className={value ? "on" : ""} onClick={() => onChange(true)}>
          {onLabel}
        </button>
        <button type="button" className={!value ? "on" : ""} onClick={() => onChange(false)}>
          {offLabel}
        </button>
      </div>
    </div>
  );
}
