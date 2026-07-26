import "./LedgerSnippet.css";

const ROWS = [
  { row: 13, gstin: "29AABCU9603R1ZX", invoice: "INV/2024/0139", value: "48,200.00", link: null },
  { row: 14, gstin: "27ABCDE1Z", invoice: "INV/2024/0140", value: "12,750.00", link: "row-14" },
  { row: 15, gstin: "07AAACT2727Q1ZW", invoice: "INV/2024/0141", value: "9,430.50", link: null },
];

interface LedgerSnippetProps {
  activeLink?: string | null;
  onLinkHover?: (link: string | null) => void;
}

export function LedgerSnippet({ activeLink, onLinkHover }: LedgerSnippetProps) {
  return (
    <div className="ledger">
      <div className="ledger-caption">workbook.xlsx — sheet b2b</div>
      <table>
        <thead>
          <tr>
            <th>Row</th>
            <th>GSTIN</th>
            <th>Invoice no.</th>
            <th>Invoice value</th>
          </tr>
        </thead>
        <tbody>
          {ROWS.map((r) => (
            <tr
              key={r.row}
              className={[
                r.link ? "is-flagged-row" : "",
                r.link && activeLink === r.link ? "is-active" : "",
              ]
                .filter(Boolean)
                .join(" ") || undefined}
              onMouseEnter={r.link ? () => onLinkHover?.(r.link) : undefined}
              onMouseLeave={r.link ? () => onLinkHover?.(null) : undefined}
            >
              <td>{r.row}</td>
              <td className={r.link ? "is-flagged" : undefined}>{r.gstin}</td>
              <td>{r.invoice}</td>
              <td>{r.value}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
