import { useState } from "preact/hooks";
import { api } from "./api.js";
import { useApp, useBusy } from "./context.js";
import { userName } from "./format.js";
import { Health } from "./Providers.jsx";

const Eye = ({ off }) => (
  <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d="M1 12s4-7 11-7 11 7 11 7-4 7-11 7S1 12 1 12z" /><circle cx="12" cy="12" r="3" />{off && <path d="M3 3l18 18" />}
  </svg>
);

// The token behind an eye: fetched from the owner-only endpoint when revealed, forgotten when hidden.
function Token({ p }) {
  const { showError } = useApp();
  const [token, setToken] = useState(null);
  const toggle = async () => {
    if (token) return setToken(null);
    try { setToken((await api(`/providers/${p.id}/token`)).token); } catch (e) { showError(e.message); }
  };
  return (
    <span class="token">
      <code>{token ?? "••••••••"}</code>
      <button class="icon" aria-label={token ? "Hide token" : "Reveal token"} title={token ? "Hide token" : "Reveal token"} onClick={toggle}><Eye off={!!token} /></button>
    </span>
  );
}

// Removes a provider, after confirming; for its owner and the admin.
function RemoveButton({ p, onError }) {
  const { refresh } = useApp();
  const busy = useBusy(onError);
  const remove = busy(async () => {
    if (!confirm(`Remove provider ${p.name}? Its workers are stopped.`)) return;
    onError(null);
    await api(`/providers/${p.id}`, { method: "DELETE" });
    await refresh();
  });
  return <button onClick={remove}>Remove</button>;
}

// One of the developer's providers: shown, or being edited (url and token; an empty token keeps the current one).
function Mine({ p, onError }) {
  const { refresh } = useApp();
  const [editing, setEditing] = useState(false);
  const busy = useBusy(onError);
  const save = busy(async e => {
    e.preventDefault();
    const f = new FormData(e.currentTarget);
    onError(null);
    await api(`/providers/${p.id}`, { method: "PATCH", body: JSON.stringify({ url: f.get("url"), ...(f.get("token") ? { token: f.get("token") } : {}) }) });
    setEditing(false);
    await refresh();
  });
  if (editing) return (
    <tr>
      <td>{p.name}</td>
      <td colspan={3}>
        <form class="row edit-provider" onSubmit={save}>
          <input name="url" type="url" defaultValue={p.url} required />
          <input name="token" type="password" placeholder="New token (unchanged if empty)" autocomplete="off" />
          <button class="primary">Save</button>
          <button type="button" onClick={() => setEditing(false)}>Cancel</button>
        </form>
      </td>
    </tr>
  );
  return (
    <tr>
      <td>{p.name}</td>
      <td>{p.url}</td>
      <td><Token p={p} /></td>
      <td class="row"><Health p={p} /><button onClick={() => { onError(null); setEditing(true); }}>Edit</button><RemoveButton p={p} onError={onError} /></td>
    </tr>
  );
}

// Under the cog. For a developer, their own providers: add, edit url and token, reveal the token, remove. For the
// admin, who has none, every provider, to remove.
export function MyProviders({ providers }) {
  const { refresh, me, users } = useApp();
  const [error, setError] = useState(null);
  const busy = useBusy(m => setError(m));
  if (me.admin) return (
    <div id="my-providers" class="panel">
      <h3>Providers</h3>
      {providers.length === 0 ? <p class="empty">No providers yet</p> : (
        <table>
          <thead><tr><th>Name</th><th>Owner</th><th>URL</th><th></th></tr></thead>
          <tbody>{providers.map(p => (
            <tr key={p.id}><td>{p.name}</td><td><span title={p.owner}>{userName(users, p.owner)}</span></td><td>{p.url}</td>
              <td class="row"><RemoveButton p={p} onError={setError} /></td></tr>
          ))}</tbody>
        </table>
      )}
      {error && <div class="error">{error}</div>}
    </div>
  );
  const add = busy(async e => {
    e.preventDefault();
    const form = e.currentTarget;
    setError(null);
    await api("/providers", { method: "POST", body: JSON.stringify(Object.fromEntries(new FormData(form))) });
    form.reset();
    await refresh();
  });
  const mine = providers.filter(p => p.owner === me.principal);
  return (
    <div id="my-providers" class="panel">
      <h3>My providers</h3>
      {mine.length === 0 ? <p class="empty">No providers yet</p> : (
        <table>
          <thead><tr><th>Name</th><th>URL</th><th>Token</th><th></th></tr></thead>
          <tbody>{mine.map(p => <Mine key={p.id} p={p} onError={setError} />)}</tbody>
        </table>
      )}
      {error && <div class="error">{error}</div>}
      <form id="add-provider" class="row" onSubmit={add}>
        <input name="name" placeholder="Name" required />
        <input name="url" type="url" placeholder="http://provider:8081" required />
        <input name="token" type="password" placeholder="Provider token" required autocomplete="off" />
        <button class="primary">Add provider</button>
      </form>
    </div>
  );
}
