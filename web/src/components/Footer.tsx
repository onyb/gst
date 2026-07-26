import "./Footer.css";

const REPO_URL = "https://github.com/onyb/gst";

export function Footer() {
  return (
    <footer className="footer">
      <div className="footer-inner">
        <span>gst · MPL-2.0 · work in progress</span>
        <div className="footer-links">
          <a href={REPO_URL}>GitHub</a>
          <a href={`${REPO_URL}/blob/master/LICENSE`}>License</a>
          <a href={`${REPO_URL}/blob/master/spec/README.md`}>Spec</a>
        </div>
      </div>
    </footer>
  );
}
