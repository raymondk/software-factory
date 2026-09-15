// Read-only view of the running configuration (GET /config; the token never leaves the orchestrator).
const Rows = ({ rows }) => (
  <dl>{rows.map(([k, v]) => <div key={k}><dt>{k}</dt><dd>{v}</dd></div>)}</dl>
);

export function Config({ config: c }) {
  if (!c) return null;
  const entries = (obj, f) => Object.entries(obj).map(([k, v]) => [k, f(v)]);
  return (
    <details id="config-section" open>
      <summary>Configuration</summary>
      <div class="panels config">
        <div class="panel"><h3>Project</h3><Rows rows={[["name", c.project.name], ["repos", c.project.repos.join("\n")]]} /></div>
        <div class="panel"><h3>Orchestrator</h3><Rows rows={[["listen", c.orchestrator.listen], ["public_url", c.orchestrator.public_url], ["database", c.orchestrator.database], ["heartbeat_timeout", c.orchestrator.heartbeat_timeout]]} /></div>
        <div class="panel"><h3>Scheduler</h3><Rows rows={[["max_workers", c.scheduler.max_workers], ["interval", c.scheduler.interval]]} /></div>
        <div class="panel"><h3>Providers</h3><Rows rows={entries(c.providers, p => p.url)} /></div>
        <div class="panel"><h3>Agents</h3><Rows rows={entries(c.agents, a => `run_timeout ${a.run_timeout}`)} /></div>
        <div class="panel wide"><h3>Prompts</h3><Rows rows={entries(c.prompts, p => <pre>{p}</pre>)} /></div>
      </div>
    </details>
  );
}
