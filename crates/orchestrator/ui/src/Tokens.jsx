import { useRef, useState } from "preact/hooks";
import { api } from "./api.js";
import { useApp, useBusy } from "./context.js";
import { Header } from "./Header.jsx";
import { ago } from "./format.js";

// Personal tokens for the CLI: listed without their secret; a new one is shown once, right after creation.
export function TokensDialog() {
  const { showError } = useApp();
  const busy = useBusy(m => setError(m));
  const dialog = useRef();
  const [tokens, setTokens] = useState(null);
  const [created, setCreated] = useState(null); // the NewToken just minted, shown until the dialog closes
  const [name, setName] = useState("");
  const [error, setError] = useState(null);
  const load = async () => { try { setTokens(await api("/tokens")); } catch (e) { showError(e.message); } };
  const open = () => { setCreated(null); setError(null); dialog.current.showModal(); load(); };
  const create = busy(async e => {
    e.preventDefault();
    setCreated(await api("/tokens", { method: "POST", body: JSON.stringify({ name }) }));
    setName("");
    await load();
  });
  const revoke = id => busy(async () => { await api(`/tokens/${id}`, { method: "DELETE" }); await load(); });
  return (<>
    <button onClick={open}>Tokens</button>
    <dialog id="tokens" ref={dialog} onClick={e => e.target === e.currentTarget && e.currentTarget.close()}>
      <Header kind="Account" title="Personal tokens" onClose={() => dialog.current.close()} />
      <div class="scroll" tabindex={-1} autofocus>
        <p>Tokens authenticate the <code>factory</code> CLI as you. Each is shown once, when created.</p>
        {created && <div class="notice token-once">
          <div>Token <b>{created.name}</b>, copy it now:</div>
          <pre>FACTORY_TOKEN={created.token}</pre>
        </div>}
        {error && <div class="error">{error}</div>}
        <form onSubmit={create} class="row">
          <input name="name" placeholder="Name, e.g. laptop" required value={name} onInput={e => setName(e.currentTarget.value)} />
          <button class="primary">Create token</button>
        </form>
        {tokens === null ? <span class="empty">Loading…</span> : tokens.length === 0 ? <p class="empty">No tokens yet</p> : (
          <table><thead><tr><th>Name</th><th>Created</th><th></th></tr></thead>
            <tbody>{tokens.map(t => (
              <tr key={t.id}><td>{t.name}</td><td title={t.created_at}>{ago(t.created_at)}</td>
                <td class="row"><button onClick={revoke(t.id)}>Revoke</button></td></tr>
            ))}</tbody>
          </table>
        )}
      </div>
    </dialog>
  </>);
}
