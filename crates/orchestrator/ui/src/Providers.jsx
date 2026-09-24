import { useApp, useBusy } from "./context.js";
import { api } from "./api.js";
import { ago, userName } from "./format.js";

// Reachable or not, and when it last answered. The last status is kept while a provider is away, so this tells them apart.
export const Health = ({ p }) => {
  if (!p.last_seen && !p.last_error) return <span class="empty">not checked yet</span>;
  const seen = p.last_seen ? <span class="empty" title={p.last_seen}>answered {ago(p.last_seen)}</span> : <span class="empty">never answered</span>;
  return p.reachable
    ? <><span class="health up">reachable</span> {seen}</>
    : <><span class="health down" title={p.last_error}>unreachable</span> {seen}</>;
};

// Removes a provider, after confirming; for its owner and the admin, wherever it is listed.
export const RemoveButton = ({ p }) => {
  const { refresh, me } = useApp();
  const busy = useBusy();
  const remove = busy(async () => {
    if (!confirm(`Remove provider ${p.name}? Its workers are stopped.`)) return;
    await api(`/providers/${p.id}`, { method: "DELETE" });
    await refresh();
  });
  return (p.owner === me.principal || me.admin) ? <button onClick={remove}>Remove</button> : null;
};

// Every developer's providers with health, workers and what they advertise, refreshed with the board's poll. Read-only:
// a developer adds and edits their own under the cog.
export function Providers({ providers }) {
  const { users, me } = useApp();
  return (
    <details id="providers-section" open>
      <summary>Providers</summary>
      <div class="panel">
        {providers.length === 0 ? <p class="empty">No providers yet</p> : (
          <table>
            <thead><tr><th>Name</th><th>Owner</th><th>URL</th><th>Health</th><th>Workers</th><th>Agents</th><th></th></tr></thead>
            <tbody id="providers">
              {providers.map(p => (
                <tr key={p.id} class={p.owner === me.principal ? "mine" : undefined}>
                  <td>{p.name}</td>
                  <td><span title={p.owner}>{userName(users, p.owner)}</span></td>
                  <td><a href={p.url.replace(/\/+$/, "") + "/status"} target="_blank" rel="noopener">{p.url}</a></td>
                  <td><Health p={p} /></td>
                  <td>{p.status ? `${p.status.in_use} / ${p.status.capacity}` : <span class="empty">no status yet</span>}</td>
                  <td>{p.status && Object.entries(p.status.agents).map(([a, info]) => <div key={a}>{a} <span class="empty">{info.models.join(", ")}</span></div>)}</td>
                  <td class="row"><RemoveButton p={p} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </details>
  );
}
