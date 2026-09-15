// Agent and model selects fed by what the providers advertise (GET /agents). An empty agent means any, an empty model
// the provider's default. The models offered are the chosen agent's, or every agent's when none is chosen; picking an
// agent that cannot run the chosen model clears the model. A value no provider advertises any more (or before the list
// has loaded) stays selectable, so the form never changes it silently.
export function AgentModel({ agents, agent, model, onChange }) {
  const models = agent ? agents[agent] ?? [] : [...new Set(Object.values(agents).flat())];
  const options = (list, current) => (current && !list.includes(current) ? [current, ...list] : list);
  const change = e => {
    const { name, value } = e.currentTarget;
    const next = { agent, model, [name]: value };
    if (name === "agent" && value && !(agents[value] ?? []).includes(next.model)) next.model = "";
    onChange(next);
  };
  return (
    <div class="pair">
      <select name="agent" value={agent} onChange={change}>
        <option value="">Agent (any)</option>
        {options(Object.keys(agents), agent).map(a => <option key={a} value={a}>{a}</option>)}
      </select>
      <select name="model" value={model} onChange={change}>
        <option value="">Model (default)</option>
        {options(models, model).map(m => <option key={m} value={m}>{m}</option>)}
      </select>
    </div>
  );
}
