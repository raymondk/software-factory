import { useState } from "preact/hooks";
import { useApp } from "./context.js";

export function Workers({ workers }) {
  const { select } = useApp();
  const [showDead, setShowDead] = useState(false);
  const dead = workers.filter(w => w.status === "dead").length;
  return (
    <details id="workers-section" open>
      <summary>Workers</summary>
      <table>
        <thead><tr><th>Id</th><th>Agent</th><th>Status</th><th>Last heartbeat</th><th>Ticket</th><th>Log</th></tr></thead>
        <tbody id="workers">
          {workers.filter(w => showDead || w.status !== "dead").map(w => (
            <tr key={w.id} data-ticket={w.ticket ?? undefined} onClick={w.ticket ? () => select(w.ticket) : undefined}>
              <td>{w.id}</td><td>{w.agent}</td><td>{w.status}</td><td>{w.last_heartbeat ?? ""}</td><td>{w.ticket ? "#" + w.ticket : ""}</td>
              <td><a href={`#/workers/${w.id}`} onClick={e => e.stopPropagation()}>Log</a></td>
            </tr>
          ))}
        </tbody>
      </table>
      {dead > 0 && <button onClick={() => setShowDead(!showDead)}>{showDead ? "Hide" : "Show"} {dead} dead</button>}
    </details>
  );
}
