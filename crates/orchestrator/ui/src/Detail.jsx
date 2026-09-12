import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { api, patch } from "./api.js";
import { useApp, useBusy } from "./context.js";
import { ACTIONS, STATES, ago, isHttp, markdown, usageLine } from "./format.js";
import { LogPane } from "./Log.jsx";

const FIELDS = ["title", "state", "description", "links"];
const same = (a, b) => FIELDS.every(k => a[k] === b[k]);

// Detail pane for one ticket (remounted per ticket via key). The form follows the server while untouched;
// once edited it keeps the user's text and offers a reload when the server version changes.
export function Detail({ ticket: t, usage, run }) {
  const { refresh, select } = useApp();
  const busy = useBusy();
  const server = useMemo(() => ({ title: t.title, state: t.state, description: t.description, links: t.links.join("\n") }),
                         [t.title, t.state, t.description, t.links]);
  const [values, setValues] = useState(server);
  const [loaded, setLoaded] = useState(server); // what the form was last filled with
  const fill = v => { setValues(v); setLoaded(v); };
  useEffect(() => { if (same(values, loaded) || same(values, server)) fill(server); }, [server, values]);
  const set = e => setValues({ ...values, [e.currentTarget.name]: e.currentTarget.value });

  // "Saved" shows next to the button for a moment after a successful save.
  const [saved, setSaved] = useState(false);
  const savedTimer = useRef();
  useEffect(() => () => clearTimeout(savedTimer.current), []);
  const save = busy(async e => {
    e.preventDefault();
    await patch(t.id, { ...values, links: values.links.split("\n").map(l => l.trim()).filter(Boolean) });
    await refresh();
    setSaved(true);
    clearTimeout(savedTimer.current);
    savedTimer.current = setTimeout(() => setSaved(false), 2000);
  });
  const action = state => busy(async () => { await patch(t.id, { state }); await refresh(); });

  return (
    <div class="pane">
      <h2><span>#{t.id} {t.title}</span><button onClick={() => select(null)}>Close</button></h2>
      <div>
      <p>rank: {t.rank} · assignee: {t.assignee ?? "none"}</p>
      <p>created: {t.created_at} · updated: {t.updated_at}</p>
      <p>{usageLine(usage)}</p>
      {t.blocked && <p class="blocked">blocked by {t.relations.filter(r => r.satisfied === false).map(r => `#${r.ticket}`).join(", ")}</p>}
      <p id="links">{t.links.map(l => isHttp(l) ? <a key={l} href={l} target="_blank" rel="noopener" title={l}>{l}</a> : <span key={l}>{l}</span>)}</p>
      <div id="actions">{(ACTIONS[t.state] ?? []).map(([label, state]) => <button key={label} onClick={action(state)}>{label}</button>)}</div>
      <form onSubmit={save}>
        <div class="notice" hidden={same(server, loaded)}>Ticket changed on the server. <button type="button" onClick={() => fill(server)}>Reload</button></div>
        <input name="title" required value={values.title} onInput={set} />
        <select name="state" value={values.state} onChange={set}>{STATES.map(s => <option key={s} value={s}>{s}</option>)}</select>
        <textarea name="description" rows={10} value={values.description} onInput={set} />
        <textarea name="links" rows={3} placeholder="Links, one per line" value={values.links} onInput={set} />
        <div class="row"><span class="saved" hidden={!saved}>Saved</span><button class="primary">Save</button></div>
      </form>
      <Relations ticket={t} />
      <div class="comments">
        <h3>Comments</h3>
        <Thread ticket={t} />
        <CommentForm ticket={t} />
      </div>
      </div>
      <Runs ticket={t} run={run} />
    </div>
  );
}

// Dependencies, dependents and related tickets; each opens that ticket. A `blocks` entry is the other ticket's
// `depends_on`, so it is removed from that side.
function Relations({ ticket: t }) {
  const { refresh } = useApp();
  const busy = useBusy();
  const [kind, setKind] = useState("depends_on");
  const [other, setOther] = useState("");
  const add = busy(async e => {
    e.preventDefault();
    await api(`/tickets/${t.id}/relations`, { method: "POST", body: JSON.stringify({ type: kind, ticket: Number(other) }) });
    setOther("");
    await refresh();
  });
  const remove = r => busy(async () => {
    const [id, type, ticket] = r.type === "blocks" ? [r.ticket, "depends_on", t.id] : [t.id, r.type, r.ticket];
    await api(`/tickets/${id}/relations/${type}/${ticket}`, { method: "DELETE" });
    await refresh();
  });
  const group = (label, type) => {
    const rs = t.relations.filter(r => r.type === type);
    return rs.length > 0 && <>
      <h4>{label}</h4>
      {rs.map(r => (
        <div key={type + r.ticket} class={"relation" + (r.satisfied === false ? " blocking" : "")}>
          <a href={`#/tickets/${r.ticket}`}>#{r.ticket} {r.title}</a><span class="tag">{r.state.replace("_", " ")}</span>
          <button onClick={remove(r)} title="Remove relation">×</button>
        </div>
      ))}
    </>;
  };
  return (
    <section id="relations">
      <h3>Relations</h3>
      {group("Depends on", "depends_on")}{group("Blocks", "blocks")}{group("Related to", "related_to")}
      <form onSubmit={add}>
        <select value={kind} onChange={e => setKind(e.currentTarget.value)}>
          <option value="depends_on">depends on</option><option value="related_to">related to</option>
        </select>
        <input name="ticket" type="number" min="1" placeholder="Ticket #" required value={other} onInput={e => setOther(e.currentTarget.value)} />
        <button>Add</button>
      </form>
    </section>
  );
}

// Runs newest first; the one in the URL is highlighted and shows its log, following live while the run is open.
// Closing the log goes back to the ticket's own URL.
function Runs({ ticket: t, run }) {
  const { select } = useApp();
  const open = t.runs.find(r => r.id === run);
  return (
    <section id="runs">
      <h3>Runs</h3>
      {t.runs.length === 0 && <p class="empty">No runs yet</p>}
      {t.runs.map(r => (
        <div key={r.id} class={"run" + (r.id === run ? " selected" : "")}>
          <a href={`#/tickets/${t.id}/runs/${r.id}`}>run {r.id}</a><span>{r.worker_id}</span>
          <span title={r.started_at}>{ago(r.started_at)}</span><span class="tag">{r.ended_at ? "ended" : "running"}</span>
        </div>
      ))}
      {open && <LogPane path={`/runs/${open.id}/logs`} live={!open.ended_at} agent={open.agent}
                        title={`run ${open.id} · ${open.worker_id}`} onClose={() => select(t.id)} />}
    </section>
  );
}

function CommentForm({ ticket: t }) {
  const { refresh } = useApp();
  const busy = useBusy();
  const [body, setBody] = useState("");
  const submit = busy(async e => {
    e.preventDefault();
    await api(`/tickets/${t.id}/comments`, { method: "POST", body: JSON.stringify({ body }) });
    setBody("");
    await refresh();
  });
  return (
    <form onSubmit={submit}>
      <textarea name="body" rows={3} placeholder="Comment" required value={body} onInput={e => setBody(e.currentTarget.value)} />
      <div class="row"><button class="primary">Comment</button></div>
    </form>
  );
}

function Thread({ ticket: t }) {
  const [showResolved, setShowResolved] = useState(false);
  const resolved = t.comments.filter(c => c.resolved).length;
  return (
    <section>
      {t.comments.map(c => <Comment key={c.id} ticket={t} comment={c} hidden={c.resolved && !showResolved} />)}
      {resolved > 0 && <button onClick={() => setShowResolved(!showResolved)}>{showResolved ? "Hide" : "Show"} {resolved} resolved</button>}
    </section>
  );
}

function Comment({ ticket: t, comment: c, hidden }) {
  const { refresh } = useApp();
  const busy = useBusy();
  const [collapsed, setCollapsed] = useState(true);
  const long = c.body.length > 600 || c.body.split("\n").length > 12;
  const kind = c.author === "human" ? "human" : "worker";
  const resolve = busy(async () => { await api(`/tickets/${t.id}/comments/${c.id}/resolve`, { method: "POST" }); await refresh(); });
  return (
    <div class={`comment ${kind}` + (c.resolved ? " resolved" : "")} hidden={hidden}>
      <small>
        {kind === "worker" ? c.author + " " : ""}<span class="tag">{kind}</span> · <span title={c.created_at}>{ago(c.created_at)}</span>{" "}
        {!c.resolved && <button onClick={resolve}>Resolve</button>}
      </small>
      <div class={"body" + (long && collapsed ? " collapsed" : "")} dangerouslySetInnerHTML={{ __html: markdown(c.body) }} />
      {long && <button onClick={() => setCollapsed(!collapsed)}>{collapsed ? "Show more" : "Show less"}</button>}
    </div>
  );
}
