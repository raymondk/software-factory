import { useApp } from "./context.js";

export function Workers({ workers }) {
  const { select } = useApp();
  return (
    <details id="workers-section" open>
      <summary>Workers</summary>
      <table>
        <thead><tr><th>Id</th><th>Type</th><th>Status</th><th>Last heartbeat</th><th>Ticket</th><th>Log</th></tr></thead>
        <tbody id="workers">
          {workers.map(w => (
            <tr key={w.id} data-ticket={w.ticket ?? undefined} onClick={w.ticket ? () => select(w.ticket) : undefined}>
              <td>{w.id}</td><td>{w.worker_type}</td><td>{w.status}</td><td>{w.last_heartbeat ?? ""}</td><td>{w.ticket ? "#" + w.ticket : ""}</td>
              <td><a href={`#/workers/${w.id}`} onClick={e => e.stopPropagation()}>Log</a></td>
            </tr>
          ))}
        </tbody>
      </table>
    </details>
  );
}
