import { useEffect, useRef, useState } from "preact/hooks";
import { api } from "./api.js";

// Monospace scrolling pane fed by `path?after=<line id>`. Polls every second while `live`, once more when it stops.
// Sticks to the bottom unless the reader has scrolled up.
export function LogPane({ path, live }) {
  const [lines, setLines] = useState([]);
  const pre = useRef(), after = useRef(0), gen = useRef(0), stick = useRef(true);
  const more = async () => {
    const g = gen.current;
    for (;;) {
      const got = await api(`${path}?after=${after.current}`).catch(() => []);
      if (g !== gen.current || !got.length) return;
      after.current = got.at(-1).id;
      setLines(l => l.concat(got));
      if (got.length < 1000) return;
    }
  };
  useEffect(() => { gen.current++; after.current = 0; stick.current = true; setLines([]); more(); }, [path]);
  useEffect(() => {
    if (!live) { more(); return; }
    const i = setInterval(more, 1000);
    return () => clearInterval(i);
  }, [path, live]);
  useEffect(() => { const p = pre.current; if (stick.current) p.scrollTop = p.scrollHeight; }, [lines]);
  const onScroll = e => { const p = e.currentTarget; stick.current = p.scrollHeight - p.scrollTop - p.clientHeight < 40; };
  return <pre class="log" ref={pre} onScroll={onScroll}>{lines.map(l => <div key={l.id}>{l.line}</div>)}</pre>;
}
