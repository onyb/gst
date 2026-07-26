import "./StampBadge.css";

export function StampBadge() {
  return (
    <div className="stamp" role="img" aria-label="Offline. Open source. No cloud.">
      <div className="stamp-text">
        <strong>OFFLINE</strong>
        <span className="stamp-rule" aria-hidden="true" />
        <span>OPEN SOURCE</span>
        <span>NO CLOUD</span>
      </div>
    </div>
  );
}
