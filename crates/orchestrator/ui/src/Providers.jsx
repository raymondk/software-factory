import { useApp } from "./context.js";
import { ago, userName } from "./format.js";

// Reachable or not, and when it last answered. The last status is kept while a provider is away, so this tells them apart.
export const Health = ({ p }) => {
  if (!p.last_seen && !p.last_error) return <span class="empty">not checked yet</span>;
  const seen = p.last_seen ? <span class="empty" title={p.last_seen}>answered {ago(p.last_seen)}</span> : <span class="empty">never answered</span>;
  return p.reachable
    ? <><span class="health up">reachable</span> {seen}</>
    : <><span class="health down" title={p.last_error}>unreachable</span> {seen}</>;
};

// The agents a provider advertises, each with its models, the default marked.
export const Agents = ({ status }) => status && Object.entries(status.agents).map(([a, info]) => (
  <div key={a}>{a} <span class="empty">{info.models.map((m, i) => <span key={m}>{i > 0 && ", "}{m === info.default_model ? <b title="default model">{m}</b> : m}</span>)}</span></div>
));

// Every developer's providers with health, workers and what they advertise, refreshed with the board's poll. Read-only:
// adding, editing and removing happen under the cog.
export function Providers({ providers }) {
  const { users, me } = useApp();
  return (
    <details id="providers-section" open>
      <summary>Providers</summary>
      <div class="panel">
        {providers.length === 0 ? <p class="empty">No providers yet</p> : (
          <table>
            <thead><tr><th>Name</th><th>Owner</th><th>URL</th><th>Health</th><th>Workers</th><th>Agents</th></tr></thead>
            <tbody id="providers">
              {providers.map(p => (
                <tr key={p.id} class={p.owner === me.principal ? "mine" : undefined}>
                  <td>{p.name}</td>
                  <td><span title={p.owner}>{userName(users, p.owner)}</span></td>
                  <td><a href={p.url.replace(/\/+$/, "") + "/status"} target="_blank" rel="noopener">{p.url}</a></td>
                  <td><Health p={p} /></td>
                  <td>{p.status ? `${p.status.in_use} / ${p.status.capacity}` : <span class="empty">no status yet</span>}</td>
                  <td><Agents status={p.status} /></td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </details>
  );
}
