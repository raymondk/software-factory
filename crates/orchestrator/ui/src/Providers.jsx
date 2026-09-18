import { useState } from "preact/hooks";
import { api } from "./api.js";
import { useApp, useBusy } from "./context.js";
import { userName } from "./format.js";

// Every developer's worker providers with their last status. The signed-in developer adds their own (the token is
// sent once and never shown again) and removes them; the admin removes any but adds none, having no tickets of their own.
export function Providers({ providers }) {
  const { refresh, users, me } = useApp();
  const [error, setError] = useState(null);
  const busy = useBusy(m => setError(m));
  const add = busy(async e => {
    e.preventDefault();
    const form = e.currentTarget;
    setError(null);
    await api("/providers", { method: "POST", body: JSON.stringify(Object.fromEntries(new FormData(form))) });
    form.reset();
    await refresh();
  });
  const remove = p => busy(async () => {
    if (!confirm(`Remove provider ${p.name}? Its workers are stopped.`)) return;
    setError(null);
    await api(`/providers/${p.id}`, { method: "DELETE" });
    await refresh();
  });
  const mine = p => p.owner === me.principal;
  return (
    <details id="providers-section" open>
      <summary>Providers</summary>
      <div class="panel">
        {providers.length === 0 ? <p class="empty">No providers yet</p> : (
          <table>
            <thead><tr><th>Name</th><th>Owner</th><th>URL</th><th>Workers</th><th>Agents</th><th></th></tr></thead>
            <tbody id="providers">
              {providers.map(p => (
                <tr key={p.id} class={mine(p) ? "mine" : undefined}>
                  <td>{p.name}</td>
                  <td><span title={p.owner}>{userName(users, p.owner)}</span></td>
                  <td><a href={p.url.replace(/\/+$/, "") + "/status"} target="_blank" rel="noopener">{p.url}</a></td>
                  <td>{p.status ? `${p.status.in_use} / ${p.status.capacity}` : <span class="empty">no status yet</span>}</td>
                  <td>{p.status && Object.entries(p.status.agents).map(([a, info]) => <div key={a}>{a} <span class="empty">{info.models.join(", ")}</span></div>)}</td>
                  <td class="row">{(mine(p) || me.admin) && <button onClick={remove(p)}>Remove</button>}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {error && <div class="error">{error}</div>}
        {!me.admin && (
          <form id="add-provider" class="row" onSubmit={add}>
            <input name="name" placeholder="Name" required />
            <input name="url" type="url" placeholder="http://provider:8081" required />
            <input name="token" type="password" placeholder="Provider token" required autocomplete="off" />
            <button class="primary">Add provider</button>
          </form>
        )}
      </div>
    </details>
  );
}
