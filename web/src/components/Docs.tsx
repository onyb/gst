import { COMMANDS } from "../content/commands";
import { useScrollReveal } from "../hooks/useScrollReveal";
import "./Docs.css";

export function Docs() {
  const ref = useScrollReveal<HTMLDivElement>();

  return (
    <section className="section" id="commands">
      <div className="section-inner reveal" ref={ref}>
        <span className="margin-label">Commands</span>
        <h2>
          One binary, <em>six verbs.</em>
        </h2>
        <div className="docs" role="table" aria-label="Command reference">
          <div className="docs-head" role="row" aria-hidden="true">
            <span>gst(1)</span>
            <span>user commands</span>
          </div>
          {COMMANDS.map((cmd) => (
            <div className="docs-row" role="row" key={cmd.name}>
              <div className="docs-verb" role="cell">
                {cmd.name}
                <span className={`docs-tag is-${cmd.status}`}>{cmd.status}</span>
              </div>
              <div role="cell">
                <p className="docs-desc">{cmd.description}</p>
                <code className="docs-usage">$ {cmd.usage}</code>
              </div>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
