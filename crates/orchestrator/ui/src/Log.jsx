import { useEffect, useRef, useState } from "preact/hooks";
import { api } from "./api.js";
import { claudeCode } from "./claude-code.jsx";

// Pretty renderers by the agent that produced the run's output; a line the renderer returns null for is shown raw.
const RENDERERS = { "claude-code": claudeCode };

// Monospace scrolling pane fed by `path?after=<line id>`. Polls every second while `live`, once more when it stops.
// Sticks to the bottom unless the reader has scrolled up. With an `agent` the UI knows, it opens in the pretty view
// with a toggle to raw. A header row shows `title`, the toggle, a fold toggle (`onFold`, with `folded`) and a Close
// button (`onClose`) when any is given. Folding hides the pane but keeps it fetching, so the live tail is intact.
export function LogPane({ path, live, agent, title, folded, onFold, onClose }) {
  const render = RENDERERS[agent];
  const [pretty, setPretty] = useState(true);
  const [lines, setLines] = useState([]);
  const pre = useRef(), after = useRef(0), gen = useRef(0), stick = useRef(true), busy = useRef(false), again = useRef(false), latest = useRef();
  // One fetch at a time: a call while one is in flight runs (against the current path) once it finishes, so two callers
  // never append the same page.
  const more = async () => {
    if (busy.current) { again.current = true; return; }
    busy.current = true;
    const g = gen.current;
    try {
      for (;;) {
        const got = await api(`${path}?after=${after.current}`).catch(() => []);
        if (g !== gen.current || !got.length) return;
        after.current = got.at(-1).id;
        setLines(l => l.concat(got));
        if (got.length < 1000) return;
      }
    } finally {
      busy.current = false;
      if (again.current) { again.current = false; latest.current(); }
    }
  };
  latest.current = more;
  useEffect(() => { gen.current++; after.current = 0; stick.current = true; setLines([]); more(); }, [path]);
  useEffect(() => {
    if (!live) { more(); return; }
    const i = setInterval(more, 1000);
    return () => clearInterval(i);
  }, [path, live]);
  useEffect(() => { const p = pre.current; if (stick.current && !folded) p.scrollTop = p.scrollHeight; }, [lines, folded]);
  const onScroll = e => { const p = e.currentTarget; stick.current = p.scrollHeight - p.scrollTop - p.clientHeight < 40; };
  const show = render && pretty ? l => render(l.line) ?? <div class="raw">{l.line}</div> : l => <div class="raw">{l.line}</div>;
  return (
    <>
      {(render || title || onFold || onClose) && (
        <div class="log-head">
          {title && <span>{title}</span>}
          {render && <button class="log-view" onClick={() => setPretty(!pretty)}>{pretty ? "Raw" : "Pretty"}</button>}
          {onFold && <button class="log-view" onClick={onFold}>{folded ? "Expand" : "Collapse"}</button>}
          {onClose && <button class="log-view" onClick={onClose}>Close</button>}
        </div>
      )}
      <pre class={"log" + (render && pretty ? " pretty" : "")} ref={pre} onScroll={onScroll} hidden={folded}>{lines.map(l => <div key={l.id} class="line">{show(l)}</div>)}</pre>
    </>
  );
}
