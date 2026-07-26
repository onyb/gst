import type { ReactNode } from "react";
import { useScrollReveal } from "../hooks/useScrollReveal";
import "./Workflow.css";

interface Step {
  title: string;
  body: ReactNode;
}

const STEPS: Step[] = [
  {
    title: "Fill",
    body: "The same Excel template the official tool uses. No new format.",
  },
  {
    title: "Validate",
    body: (
      <>
        <code>gst validate workbook.xlsx</code> — every error points at its
        sheet, row, and column.
      </>
    ),
  },
  {
    title: "Upload",
    body: (
      <>
        <code>gst upload workbook.xlsx</code> — writes the portal file:
        <span className="workflow-output">
          returns_072024_R1_27AAAAA0000A1Z5_offline.json
        </span>
      </>
    ),
  },
  {
    title: "File",
    body: "Import it on gst.gov.in and file with DSC or EVC, as usual.",
  },
];

export function Workflow() {
  const ref = useScrollReveal<HTMLDivElement>();

  return (
    <section className="section" id="workflow">
      <div className="section-inner reveal" ref={ref}>
        <span className="margin-label">Workflow</span>
        <h2>
          Fill. Validate. Upload. <em>File.</em>
        </h2>
        <ol className="workflow">
          {STEPS.map((step, i) => (
            <li className="workflow-step" key={step.title}>
              <span className="workflow-num" aria-hidden="true">
                {i + 1}
              </span>
              <div className="workflow-title">{step.title}</div>
              <p className="workflow-body">{step.body}</p>
            </li>
          ))}
        </ol>
      </div>
    </section>
  );
}
