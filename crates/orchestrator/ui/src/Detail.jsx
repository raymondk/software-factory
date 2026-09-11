import { useEffect, useMemo, useState } from "preact/hooks";
import { api, patch } from "./api.js";
import { useApp, useBusy } from "./context.js";
import { ACTIONS, STATES, ago, isHttp, markdown, usageLine } from "./format.js";

const FIELDS = ["title", "state", "description", "links"];
const same = (a, b) => FIELDS.every(k => a[k] === b[k]);

// Detail pane for one ticket (remounted per ticket via key). The form follows the server while untouched;
// once edited it keeps the user's text and offers a reload when the server version changes.
export function Detail({ ticket: t, usage }) {
  const { refresh, select } = useApp();
  const busy = useBusy();
  const server = useMemo(() => ({ title: t.title, state: t.state, description: t.description, links: t.links.join("\n") }),
                         [t.title, t.state, t.description, t.links]);
  const [values, setValues] = useState(server);
  const [loaded, setLoaded] = useState(server); // what the form was last filled with
  const fill = v => { setValues(v); setLoaded(v); };
  useEffect(() => { if (same(values, loaded) || same(values, server)) fill(server); }, [server, values]);
  const set = e => setValues({ ...values, [e.currentTarget.name]: e.currentTarget.value });

  const save = busy(async e => {
    e.preventDefault();
    await patch(t.id, { ...values, links: values.links.split("\n").map(l => l.trim()).filter(Boolean) });
    await refresh();
  });
  const action = state => busy(async () => { await patch(t.id, { state }); await refresh(); });

  return (
    <div class="pane">
      <h2><span>#{t.id} {t.title}</span><button onClick={() => select(null)}>Close</button></h2>
      <div>
      <p>rank: {t.rank} · assignee: {t.assignee ?? "none"}</p>
      <p>created: {t.created_at} · updated: {t.updated_at}</p>
      <p>{usageLine(usage)}</p>
      <p id="links">{t.links.map(l => isHttp(l) ? <a key={l} href={l} target="_blank" rel="noopener" title={l}>{l}</a> : <span key={l}>{l}</span>)}</p>
      <div id="actions">{(ACTIONS[t.state] ?? []).map(([label, state]) => <button key={label} onClick={action(state)}>{label}</button>)}</div>
      <form onSubmit={save}>
        <div class="notice" hidden={same(server, loaded)}>Ticket changed on the server. <button type="button" onClick={() => fill(server)}>Reload</button></div>
        <input name="title" required value={values.title} onInput={set} />
        <select name="state" value={values.state} onChange={set}>{STATES.map(s => <option key={s} value={s}>{s}</option>)}</select>
        <textarea name="description" rows={10} value={values.description} onInput={set} />
        <textarea name="links" rows={3} placeholder="Links, one per line" value={values.links} onInput={set} />
        <button>Save</button>
      </form>
      </div>
      <div class="comments">
        <h3>Comments</h3>
        <Thread ticket={t} />
        <CommentForm ticket={t} />
      </div>
    </div>
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
      <button>Comment</button>
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
