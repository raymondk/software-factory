import { useState } from "preact/hooks";
import { api, patch } from "./api.js";
import { useApp } from "./context.js";
import { STATES, isHttp } from "./format.js";

const upper = (e, c) => { const r = c.getBoundingClientRect(); return e.clientY < r.top + r.height / 2; };

export function Board({ tickets, selected }) {
  const { refresh, select, showError } = useApp();
  const [drag, setDrag] = useState(null); // { ticket, tickets }: the board shows the snapshot taken at drag start until the drop
  const [over, setOver] = useState(null); // marked drop target: { card, cls } or { column }
  const shown = drag?.tickets ?? tickets;

  // Changes state if needed, then rank; the board re-renders either way, so a failed call snaps it back.
  const move = async (t, state, rel) => {
    try {
      if (state !== t.state) await patch(t.id, { state });
      if (rel) await api(`/tickets/${t.id}/move`, { method: "POST", body: JSON.stringify(rel) });
    } catch (e) { showError(e.message); }
    await refresh();
  };
  const drop = (e, state, rel) => { e.preventDefault(); const t = drag?.ticket; setDrag(null); setOver(null); if (t) move(t, state, rel); };
  const markCard = (id, cls) => setOver(o => o?.card === id && o.cls === cls ? o : { card: id, cls });

  const card = (t, prev, next) => (
    <div key={t.id} data-id={t.id} draggable tabIndex={0}
         class={"card" + (t.id === selected ? " selected" : "") + (drag?.ticket.id === t.id ? " dragging" : "") + (over?.card === t.id ? " " + over.cls : "")}
         onClick={() => select(t.id === selected ? null : t.id)}
         onDragStart={e => { setDrag({ ticket: t, tickets }); e.dataTransfer.setData("text/plain", t.id); }}
         onDragEnd={() => { setDrag(null); setOver(null); }}
         onDragOver={e => { e.preventDefault(); e.stopPropagation(); markCard(t.id, upper(e, e.currentTarget) ? "drop-before" : "drop-after"); }}
         onDrop={e => { e.stopPropagation(); if (drag?.ticket.id !== t.id) drop(e, t.state, upper(e, e.currentTarget) ? { before: t.id } : { after: t.id }); }}
         onKeyDown={e => {
           if (e.key === "Enter") return select(t.id);
           const rel = e.altKey && (e.key === "ArrowUp" ? prev && { before: prev.id } : e.key === "ArrowDown" && next && { after: next.id });
           if (rel) { e.preventDefault(); move(t, t.state, rel); }
         }}>
      <div>#{t.id} {t.title}</div>
      <div class="meta">
        {t.assignee && <span>{t.assignee}</span>}
        {t.links.filter(isHttp).map(href => <a key={href} href={href} target="_blank" rel="noopener" title={href} onClick={e => e.stopPropagation()}>↗</a>)}
        {t.unresolved_comments ? <span class="badge" title="unresolved comments">{t.unresolved_comments}</span> : null}
      </div>
    </div>
  );

  return STATES.map(s => {
    const cards = shown.filter(t => t.state === s);
    return (
      <section key={s} class={"column " + s + (over?.column === s ? " drop" : "")}
               onDragOver={e => { e.preventDefault(); setOver(o => o?.column === s ? o : { column: s }); }}
               onDragLeave={e => e.currentTarget.contains(e.relatedTarget) || setOver(null)}
               onDrop={e => { const last = cards.filter(t => t.id !== drag?.ticket.id).at(-1); drop(e, s, last && { after: last.id }); }}>
        <h3>{s.replace("_", " ")} ({cards.length})</h3>
        {cards.map((t, i) => card(t, cards[i - 1], cards[i + 1]))}
      </section>
    );
  });
}
