export interface CommandEntry {
  name: string;
  usage: string;
  description: string;
  status: "ready" | "planned";
}

// Mirrors the Command enum in crates/gst-cli/src/main.rs — keep in sync.
export const COMMANDS: CommandEntry[] = [
  {
    name: "validate",
    usage: "gst validate workbook.xlsx --gstin <GSTIN> --period <MMYYYY>",
    description: "Report every problem with its sheet, row and column.",
    status: "ready",
  },
  {
    name: "summary",
    usage: "gst summary workbook.xlsx --gstin <GSTIN> --period <MMYYYY>",
    description: "Print section totals before you upload.",
    status: "ready",
  },
  {
    name: "upload",
    usage: "gst upload workbook.xlsx --gstin <GSTIN> --period <MMYYYY>",
    description: "Write the complete portal upload file from one workbook.",
    status: "ready",
  },
  {
    name: "generate",
    usage: "gst generate workbook.xlsx --gstin <GSTIN> --period <MMYYYY>",
    description: "Emit a single section's payload on its own.",
    status: "ready",
  },
  {
    name: "errors",
    usage: "gst errors portal-errors.csv workbook.xlsx",
    description: "Map the portal's error file back to your rows.",
    status: "planned",
  },
  {
    name: "diff",
    usage: "gst diff left.json right.json",
    description: "Semantically compare two portal JSON files.",
    status: "planned",
  },
];
