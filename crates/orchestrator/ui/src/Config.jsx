import { useRef, useState } from "preact/hooks";
import { api } from "./api.js";
import { useApp } from "./context.js";
import { Header } from "./Header.jsx";

const Rows = ({ rows }) => (
  <dl>{rows.map(([k, v]) => <div key={k}><dt>{k}</dt><dd>{v}</dd></div>)}</dl>
);
const Link = ({ href }) => <a href={href} target="_blank" rel="noopener">{href}</a>;
const entries = (obj, f) => Object.entries(obj).map(([k, v]) => [k, f(v)]);

const Panes = ({ config: c }) => (
  <div class="panels config">
    <div class="panel"><h3>Project</h3><Rows rows={[["name", c.project.name], ["repos", c.project.repos.map(r => <Link key={r} href={r} />)]]} /></div>
    <div class="panel"><h3>Orchestrator</h3><Rows rows={[["listen", c.orchestrator.listen], ["public_url", c.orchestrator.public_url], ["database", c.orchestrator.database], ["heartbeat_timeout", c.orchestrator.heartbeat_timeout]]} /></div>
    <div class="panel"><h3>Scheduler</h3><Rows rows={[["max_workers", c.scheduler.max_workers], ["interval", c.scheduler.interval]]} /></div>
    <div class="panel"><h3>Providers</h3><Rows rows={entries(c.providers, p => <Link href={p.url.replace(/\/+$/, "") + "/status"} />)} /></div>
    <div class="panel"><h3>Agents</h3><Rows rows={entries(c.agents, a => `run_timeout ${a.run_timeout}`)} /></div>
    <div class="panel wide"><h3>Prompts</h3><Rows rows={entries(c.prompts, p => <pre>{p}</pre>)} /></div>
  </div>
);

// The cog at the top right opens the running configuration (GET /config; the token never leaves the orchestrator)
// read-only in a modal, fetched each time it opens.
export function ConfigDialog() {
  const { showError } = useApp();
  const dialog = useRef();
  const [config, setConfig] = useState(null);
  const open = async () => {
    dialog.current.showModal();
    try { setConfig(await api("/config")); } catch (e) { showError(e.message); }
  };
  return (<>
    <button id="settings" title="Configuration" aria-label="Configuration" onClick={open}>
      <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
      </svg>
    </button>
    <dialog id="config" ref={dialog} onClick={e => e.target === e.currentTarget && e.currentTarget.close()}>
      <Header kind="Orchestrator" title="Configuration" onClose={() => dialog.current.close()} />
      <div class="scroll" tabindex={-1} autofocus>{config ? <Panes config={config} /> : <span class="empty">Loading…</span>}</div>
    </dialog>
  </>);
}
