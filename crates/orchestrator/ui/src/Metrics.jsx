import { usageLine } from "./format.js";

const Table = ({ label, rows, id }) => (
  <div class="panel">
    <h3>Per {label.toLowerCase()}</h3>
    <table>
      <thead><tr><th>{label}</th><th>Tokens in</th><th>Tokens out</th><th>Cost</th><th>Completed</th><th>Failed</th></tr></thead>
      <tbody id={id}>
        {rows.map(r => (
          <tr key={r.key}>
            <td>{r.key}</td><td>{r.tokens_in}</td><td>{r.tokens_out}</td><td>${r.cost.toFixed(2)}</td><td>{r.tickets_completed}</td><td>{r.tickets_failed}</td>
          </tr>
        ))}
      </tbody>
    </table>
  </div>
);

export function Metrics({ metrics: m }) {
  return (
    <details id="metrics-section" open>
      <summary>Metrics</summary>
      <p id="totals">{m && `${usageLine(m.totals)} · completed: ${m.totals.tickets_completed} · failed: ${m.totals.tickets_failed}`}</p>
      <div class="panels">
        <Table id="metrics" label="Agent" rows={m?.per_agent.map(r => ({ ...r, key: r.agent })) ?? []} />
        {m?.per_model.length > 0 && <Table id="metrics-models" label="Model" rows={m.per_model.map(r => ({ ...r, key: r.model ?? "(none)" }))} />}
      </div>
    </details>
  );
}
