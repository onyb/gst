import { useState } from "react";
import { useScrollReveal } from "../hooks/useScrollReveal";
import "./Closing.css";

const REPO_URL = "https://github.com/onyb/gst";
const INSTALL_CMD = "brew install onyb/tap/gst";

const FACTS = [
  "Clean-room — built from public templates and schemas, no GSTN code",
  "Spec-driven — every rule is readable JSON in spec/",
  "Offline — zero network calls, ever",
];

export function Closing() {
  const ref = useScrollReveal<HTMLDivElement>();
  const [copied, setCopied] = useState(false);

  async function copyInstall() {
    try {
      await navigator.clipboard.writeText(INSTALL_CMD);
      setCopied(true);
      setTimeout(() => setCopied(false), 1800);
    } catch {
      // clipboard API unavailable — the command is still selectable as text
    }
  }

  return (
    <section className="section" id="install">
      <div className="section-inner reveal" ref={ref}>
        <span className="margin-label">Trust, then verify</span>
        <h2>
          This touches your taxes. <em>Read it first.</em>
        </h2>
        <ul className="closing-facts">
          {FACTS.map((fact) => (
            <li key={fact}>{fact}</li>
          ))}
        </ul>
        <div className="closing-actions">
          <button className="closing-install" onClick={copyInstall} type="button">
            <span aria-hidden="true">$</span>
            <code>{INSTALL_CMD}</code>
            <span className="closing-copy" aria-live="polite">
              {copied ? "copied" : "copy"}
            </span>
          </button>
          <div className="closing-links">
            <a href={REPO_URL}>Source →</a>
            <a href={`${REPO_URL}/tree/master/spec`}>The spec →</a>
          </div>
        </div>
      </div>
    </section>
  );
}
