import { usageLine } from "./format.js";

export function Metrics({ metrics: m }) {
  return (
    <details id="metrics-section" open>
      <summary>Metrics</summary>
      <p id="totals">{m && `${usageLine(m.totals)} · completed: ${m.totals.tickets_completed} · failed: ${m.totals.tickets_failed}`}</p>
      <table>
        <thead><tr><th>Agent</th><th>Tokens in</th><th>Tokens out</th><th>Cost</th><th>Completed</th><th>Failed</th></tr></thead>
        <tbody id="metrics">
          {m?.per_agent.map(w => (
            <tr key={w.agent}>
              <td>{w.agent}</td><td>{w.tokens_in}</td><td>{w.tokens_out}</td><td>${w.cost.toFixed(2)}</td><td>{w.tickets_completed}</td><td>{w.tickets_failed}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </details>
  );
}
