// Agent and model selects fed by what the providers advertise (GET /agents). An empty agent means any, an empty model
// the provider's default. The models offered are the chosen agent's, or every agent's when none is chosen; picking an
// agent that cannot run the chosen model clears the model. A value no provider advertises any more (or before the list
// has loaded) stays selectable, so the form never changes it silently. `disabled` for a ticket without an owner: agent
// and model are validated against the owner's providers, so there is nothing to pick from.
export function AgentModel({ agents, agent, model, onChange, disabled = false }) {
  const models = agent ? agents[agent] ?? [] : [...new Set(Object.values(agents).flat())];
  const options = (list, current) => (current && !list.includes(current) ? [current, ...list] : list);
  const change = e => {
    const { name, value } = e.currentTarget;
    const next = { agent, model, [name]: value };
    if (name === "agent" && value && !(agents[value] ?? []).includes(next.model)) next.model = "";
    onChange(next);
  };
  return (<>
    <div class="pair">
      <select name="agent" value={agent} onChange={change} disabled={disabled}>
        <option value="">Agent (any)</option>
        {options(Object.keys(agents), agent).map(a => <option key={a} value={a}>{a}</option>)}
      </select>
      <select name="model" value={model} onChange={change} disabled={disabled}>
        <option value="">Model (default)</option>
        {options(models, model).map(m => <option key={m} value={m}>{m}</option>)}
      </select>
    </div>
    {disabled && <small class="hint">Agent and model come from the owner's providers; this ticket has no owner.</small>}
  </>);
}
