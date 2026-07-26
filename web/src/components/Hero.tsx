import { useState } from "react";
import { StampBadge } from "./StampBadge";
import { Terminal, type TerminalLine } from "./Terminal";
import { LedgerSnippet } from "./LedgerSnippet";
import "./Hero.css";

const REPO_URL = "https://github.com/onyb/gst";
const INSTALL_CMD = "brew install onyb/tap/gst";

const DEMO_LINES: TerminalLine[] = [
  { text: "workbook.xlsx — B2B (Business to Business)", variant: "dim" },
  { text: "142 row(s) read", variant: "dim" },
  { text: "" },
  { text: "error row 14 · GSTIN", variant: "error", link: "row-14" },
  {
    text: "      '27ABCDE1Z' is not a valid registration number (check digit failed)",
    link: "row-14",
  },
  { text: "" },
  { text: "1 error(s); 141 envelope(s) would be generated", variant: "summary" },
];

export function Hero() {
  const [copied, setCopied] = useState(false);
  const [activeLink, setActiveLink] = useState<string | null>(null);

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
    <header className="hero">
      <nav className="hero-nav">
        <span className="hero-nav-mark">gst</span>
        <div className="hero-nav-links">
          <a href="#workflow">Workflow</a>
          <a href="#commands">Commands</a>
          <a href={REPO_URL}>GitHub</a>
        </div>
      </nav>
      <div className="hero-inner">
        <div className="hero-left">
          <span className="margin-label">Offline GST return preparation</span>
          <h1 className="hero-headline">
            File GST returns <em>without booting Windows.</em>
          </h1>
          <p className="hero-sub">
            An open-source Rust CLI that checks your Excel workbook and writes
            portal-ready upload JSON — offline, on macOS, Linux, or Windows.
          </p>
          <div className="hero-actions">
            <button
              className="hero-install"
              onClick={copyInstall}
              type="button"
              title="Copy install command"
            >
              <span className="hero-install-prompt" aria-hidden="true">
                $
              </span>
              <code className="hero-install-cmd">{INSTALL_CMD}</code>
              <span className="hero-install-action" aria-live="polite">
                {copied ? "copied" : "copy"}
              </span>
            </button>
            <a className="hero-github" href={REPO_URL}>
              Source on GitHub →
            </a>
          </div>
        </div>
        <div className="hero-figure">
          <LedgerSnippet activeLink={activeLink} onLinkHover={setActiveLink} />
          <Terminal
            label="terminal"
            prompt="gst validate workbook.xlsx --gstin 27AAAAA0000A1Z5 --period 072024"
            lines={DEMO_LINES}
            activeLink={activeLink}
            onLinkHover={setActiveLink}
          />
          <StampBadge />
        </div>
      </div>
    </header>
  );
}
