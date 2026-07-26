import { useTypedText, useStaggerReveal } from "../hooks/useTypedText";
import "./Terminal.css";

export type TerminalLineVariant =
  | "prompt"
  | "output"
  | "dim"
  | "error"
  | "warn"
  | "success"
  | "summary";

export interface TerminalLine {
  text: string;
  variant?: TerminalLineVariant;
  /** Ties this line to an element elsewhere on the page (cross-highlight). */
  link?: string;
}

interface TerminalProps {
  label: string;
  prompt: string;
  lines: TerminalLine[];
  activeLink?: string | null;
  onLinkHover?: (link: string | null) => void;
}

export function Terminal({
  label,
  prompt,
  lines,
  activeLink,
  onLinkHover,
}: TerminalProps) {
  const { text: typedPrompt, done: promptDone } = useTypedText(prompt);
  const visibleCount = useStaggerReveal(lines.length, promptDone);

  return (
    <div className="terminal" role="group" aria-label={`${label}: ${prompt}`}>
      <div className="terminal-chrome">
        <span aria-hidden="true" />
        <span aria-hidden="true" />
        <span aria-hidden="true" />
        <span className="terminal-chrome-label">{label}</span>
      </div>
      <div className="terminal-body">
        <div className="terminal-line terminal-prompt">
          {typedPrompt}
          {!promptDone && <span className="terminal-cursor" aria-hidden="true" />}
        </div>
        {lines.slice(0, visibleCount).map((line, i) => (
          <div
            key={i}
            className={[
              "terminal-line",
              line.variant ? `is-${line.variant}` : "",
              line.link ? "is-linked" : "",
              line.link && activeLink === line.link ? "is-active" : "",
            ]
              .filter(Boolean)
              .join(" ")}
            onMouseEnter={line.link ? () => onLinkHover?.(line.link!) : undefined}
            onMouseLeave={line.link ? () => onLinkHover?.(null) : undefined}
          >
            {line.text || " "}
          </div>
        ))}
      </div>
    </div>
  );
}
