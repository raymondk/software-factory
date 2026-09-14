import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { api } from "./api.js";
import { Ctx, useApp, useBusy } from "./context.js";
import { STATES } from "./format.js";
import { Board } from "./Board.jsx";
import { Detail } from "./Detail.jsx";
import { Workers } from "./Workers.jsx";
import { Metrics } from "./Metrics.jsx";
import { LogPane } from "./Log.jsx";
import { Close } from "./Close.jsx";

// What is open lives in the URL hash, so reload, back and forward all go through history:
// #/tickets/<id>, #/tickets/<id>/runs/<run> (that run's log), #/workers/<id> (the worker's log).
const idFromHash = () => { const m = /^#\/tickets\/(\d+)(\/runs\/\d+)?$/.exec(location.hash); return m ? Number(m[1]) : null; };
const runFromHash = () => { const m = /^#\/tickets\/\d+\/runs\/(\d+)$/.exec(location.hash); return m ? Number(m[1]) : null; };
const workerFromHash = () => { const m = /^#\/workers\/([\w-]+)$/.exec(location.hash); return m ? m[1] : null; };
const select = id => { location.hash = id == null ? "" : "#/tickets/" + id; };
const clearHash = () => history.replaceState(null, "", location.pathname);

export function App() {
  const [tickets, setTickets] = useState(null); // null until the first load settles
  const [workers, setWorkers] = useState([]);
  const [metrics, setMetrics] = useState(null);
  const [selected, setSelected] = useState(idFromHash);
  const [run, setRun] = useState(runFromHash);
  const [workerLog, setWorkerLog] = useState(workerFromHash);
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
      const id = idFromHash(), worker = workerFromHash();
      if (id == null && worker == null && location.hash) { showError(`Ticket ${location.hash.split("/").pop()} not found`); clearHash(); }
      setSelected(id); setRun(runFromHash()); setWorkerLog(worker);
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

  // The detail dialog follows the open ticket: shown while one is loaded, closed when the hash clears. Escape and a
  // backdrop click close it natively; that close event clears the selection. The close event is asynchronous, so the
  // one from our own close() is flagged and ignored: by the time it fires the hash may already point at a ticket again.
  const dialog = useRef(), closing = useRef(false);
  useLayoutEffect(() => {
    const d = dialog.current;
    if (ticket && !d.open) d.showModal();
    if (!ticket && d.open) { closing.current = true; d.close(); }
  }, [ticket]);
  // A user dismissal drops the selection at once, so a refresh already in flight cannot reopen the dialog.
  const dismissed = () => {
    if (closing.current) { closing.current = false; return; }
    selectedRef.current = null; setTicket(null); select(null);
  };

  // The worker-log dialog follows the hash the same way.
  const workerDialog = useRef(), closingWorker = useRef(false);
  useLayoutEffect(() => {
    const d = workerDialog.current;
    if (workerLog && !d.open) d.showModal();
    if (!workerLog && d.open) { closingWorker.current = true; d.close(); }
  }, [workerLog]);
  const workerDismissed = () => {
    if (closingWorker.current) { closingWorker.current = false; return; }
    setWorkerLog(null); select(null);
  };
  const loggedWorker = workerLog && (workers.find(w => w.id === workerLog) ?? { id: workerLog, status: "?" });

  const usage = ticket && (metrics?.per_ticket.find(x => x.ticket_id === ticket.id) ?? { tokens_in: 0, tokens_out: 0, cost: 0 });
  return (
    <Ctx.Provider value={{ refresh, select, showError }}>
      <h1>Software Factory</h1>
      <div id="error">{error && <><span>{error.message}</span><button onClick={() => setError(null)}>×</button></>}</div>
      <section>
        <CreateDialog />
        <div id="board">{tickets ? <Board tickets={tickets} selected={selected} /> : <span id="loading">Loading…</span>}</div>
      </section>
      <dialog id="detail" ref={dialog} onClose={dismissed} onClick={e => e.target === e.currentTarget && e.currentTarget.close()}>
        {ticket && <Detail key={ticket.id} ticket={ticket} usage={usage} run={run} />}
      </dialog>
      <dialog id="worker-log" ref={workerDialog} onClose={workerDismissed} onClick={e => e.target === e.currentTarget && e.currentTarget.close()}>
        {loggedWorker && <>
          <Close onClick={() => select(null)} />
          <h2><span>{loggedWorker.id} <span class="tag">{loggedWorker.status}</span></span></h2>
          <LogPane path={`/workers/${loggedWorker.id}/logs`} live={loggedWorker.status !== "dead"} agent={loggedWorker.agent} />
        </>}
      </dialog>
      <Metrics metrics={metrics} />
      <Workers workers={workers} />
    </Ctx.Provider>
  );
}

// "New ticket" opens a dialog with the form; a failed request shows its error inside and keeps the input.
function CreateDialog() {
  const { refresh } = useApp();
  const dialog = useRef();
  const [error, setError] = useState(null);
  const busy = useBusy(m => setError(m));
  const submit = busy(async e => {
    e.preventDefault();
    const form = e.currentTarget, f = new FormData(form);
    await api("/tickets", { method: "POST", body: JSON.stringify(Object.fromEntries(f)) });
    form.reset();
    dialog.current.close();
    await refresh();
  });
  return (
    <>
      <div id="toolbar"><button class="primary" onClick={() => { setError(null); dialog.current.showModal(); }}>New ticket</button></div>
      <dialog ref={dialog}>
        <Close onClick={() => dialog.current.close()} />
        <h2><span>New ticket</span></h2>
        <form id="create" onSubmit={submit}>
          {error && <div class="error">{error}</div>}
          <input name="title" placeholder="Title" required />
          <select name="state">{STATES.map(s => <option key={s} value={s}>{s}</option>)}</select>
          <textarea name="description" placeholder="Description" rows={16} />
          <div class="row"><button class="primary">Create ticket</button></div>
        </form>
      </dialog>
    </>
  );
}
