import { useScrollReveal } from "../hooks/useScrollReveal";
import "./Compare.css";

const ROWS = [
  { label: "Runs on", official: "Windows only", gst: "macOS · Linux · Windows" },
  { label: "Source", official: "Closed", gst: "Open — MPL-2.0" },
  { label: "Interface", official: "Desktop GUI", gst: "One CLI binary" },
  { label: "Network", official: "Local Node server", gst: "None — files in, files out" },
];

export function Compare() {
  const ref = useScrollReveal<HTMLDivElement>();

  return (
    <section className="section" id="why">
      <div className="section-inner reveal" ref={ref}>
        <span className="margin-label">Why this exists</span>
        <h2>
          The official offline tool wasn't built <em>for you to use.</em>
        </h2>
        <table className="compare">
          <thead>
            <tr>
              <th />
              <th scope="col">GSTN's offline tool</th>
              <th scope="col" className="compare-gst">
                gst
              </th>
            </tr>
          </thead>
          <tbody>
            {ROWS.map((row) => (
              <tr key={row.label}>
                <th scope="row">{row.label}</th>
                <td>{row.official}</td>
                <td className="compare-gst">{row.gst}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
