import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { api } from "./api.js";
import { Ctx, useApp, useBusy } from "./context.js";
import { Board } from "./Board.jsx";
import { Detail } from "./Detail.jsx";
import { Workers } from "./Workers.jsx";
import { Metrics } from "./Metrics.jsx";

// The open ticket lives in the URL hash (#/tickets/<id>), so reload, back and forward all go through history.
const idFromHash = () => { const m = /^#\/tickets\/(\d+)$/.exec(location.hash); return m ? Number(m[1]) : null; };
const select = id => { location.hash = id == null ? "" : "#/tickets/" + id; };
const clearHash = () => history.replaceState(null, "", location.pathname);

export function App() {
  const [tickets, setTickets] = useState(null); // null until the first load settles
  const [workers, setWorkers] = useState([]);
  const [metrics, setMetrics] = useState(null);
  const [selected, setSelected] = useState(idFromHash);
  const [ticket, setTicket] = useState(null); // the open ticket, in full
  const [error, setError] = useState(null); // { message, fromPoll }
  const selectedRef = useRef(selected);
  selectedRef.current = selected;

  const showError = useCallback((message, fromPoll) => setError({ message, fromPoll: !!fromPoll }), []);

  // Reloads everything; a poll error is shown once (replaced, not stacked) and cleared by the next successful poll.
  const refresh = useCallback(async fromPoll => {
    const id = selectedRef.current;
    try {
      const [ts, ws, m, t] = await Promise.all([api("/tickets"), api("/workers"), api("/metrics"),
        id == null ? null : api("/tickets/" + id).catch(e => { if (e.message !== "not found") throw e; })]);
      setTickets(ts); setWorkers(ws); setMetrics(m);
      if (id != null && !t) { showError(`Ticket ${id} not found`); clearHash(); setSelected(null); }
      if (selectedRef.current === id) setTicket(t ?? null);
      if (fromPoll) setError(e => e?.fromPoll ? null : e);
    } catch (e) { showError(e.message, fromPoll); }
    finally { setTickets(t => t ?? []); }
  }, [showError]);

  useEffect(() => {
    const route = () => {
      const id = idFromHash();
      if (id == null && location.hash) { showError(`Ticket ${location.hash.split("/").pop()} not found`); clearHash(); }
      setSelected(id);
    };
    route();
    addEventListener("hashchange", route);
    return () => removeEventListener("hashchange", route);
  }, [showError]);

  useEffect(() => { if (selected == null) setTicket(null); refresh(); }, [selected, refresh]);

  // Polls while the tab is visible and refreshes as soon as it becomes visible again.
  useEffect(() => {
    const tick = () => { if (!document.hidden) refresh(true); };
    const i = setInterval(tick, 3000);
    document.addEventListener("visibilitychange", tick);
    return () => { clearInterval(i); document.removeEventListener("visibilitychange", tick); };
  }, [refresh]);

  const usage = ticket && (metrics?.per_ticket.find(x => x.ticket_id === ticket.id) ?? { tokens_in: 0, tokens_out: 0, cost: 0 });
  return (
    <Ctx.Provider value={{ refresh, select, showError }}>
      <h1>Software Factory</h1>
      <div id="error">{error && <><span>{error.message}</span><button onClick={() => setError(null)}>×</button></>}</div>
      <section>
        <CreateForm />
        <div id="board">{tickets ? <Board tickets={tickets} selected={selected} /> : <span id="loading">Loading…</span>}</div>
      </section>
      <section id="detail">{ticket && <Detail key={ticket.id} ticket={ticket} usage={usage} />}</section>
      <Workers workers={workers} />
      <Metrics metrics={metrics} />
    </Ctx.Provider>
  );
}

function CreateForm() {
  const busy = useBusy();
  const { refresh } = useApp();
  const submit = busy(async e => {
    e.preventDefault();
    const form = e.currentTarget, f = new FormData(form);
    const t = await api("/tickets", { method: "POST", body: JSON.stringify({ title: f.get("title"), description: f.get("description") }) });
    form.reset();
    await refresh();
    select(t.id);
  });
  return (
    <form id="create" onSubmit={submit}>
      <input name="title" placeholder="Title" required />
      <textarea name="description" placeholder="Description" rows={1} />
      <button>Create ticket</button>
    </form>
  );
}
