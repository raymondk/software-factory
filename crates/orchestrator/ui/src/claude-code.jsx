import { markdown } from "./format.js";

// Renders one line of Claude Code's `--output-format stream-json`. Returns null for a line that is not one of its
// events, so the pane shows it raw: the worker's own stderr, a truncated line.
export function claudeCode(line) {
  let ev;
  try { ev = JSON.parse(line); } catch { return null; }
  if (!ev || typeof ev !== "object") return null;
  switch (ev.type) {
    case "system": return <System ev={ev} />;
    case "assistant": return <Assistant ev={ev} />;
    case "user": return <ToolResults ev={ev} />;
    case "result": return <Result ev={ev} />;
    default: return null;
  }
}

const json = v => typeof v === "string" ? v : JSON.stringify(v, null, 2);
const LONG = 600;
// A block that collapses long content, or all content when `collapsed`.
const Block = ({ kind, title, text, collapsed = text.length > LONG || text.split("\n").length > 12 }) => (
  <details class={"ev " + kind} open={!collapsed}>
    <summary>{title}</summary>
    <pre>{text}</pre>
  </details>
);

function System({ ev }) {
  const { type, subtype, ...rest } = ev;
  return <Block kind="system" title={`system ${subtype ?? ""}`.trim()} text={json(rest)} collapsed />;
}

// Each content block of an assistant message: text as markdown, thinking dimmed, a tool call with its input.
function Assistant({ ev }) {
  const content = ev.message?.content;
  if (!Array.isArray(content)) return <Block kind="system" title="assistant" text={json(ev)} />;
  return content.map((c, i) => {
    if (c.type === "text") return <div key={i} class="ev text" dangerouslySetInnerHTML={{ __html: markdown(c.text) }} />;
    if (c.type === "thinking") return <Block key={i} kind="thinking" title="thinking" text={c.thinking} />;
    if (c.type === "tool_use") return <Block key={i} kind="tool" title={c.name} text={toolInput(c.input)} />;
    return <Block key={i} kind="system" title={c.type} text={json(c)} />;
  });
}

// The input of a tool call: a one-field input shows just its value, others their JSON.
const toolInput = input => {
  if (!input || typeof input !== "object") return json(input);
  const keys = Object.keys(input);
  return keys.length === 1 ? json(input[keys[0]]) : json(input);
};

function ToolResults({ ev }) {
  const content = ev.message?.content;
  if (!Array.isArray(content)) return <Block kind="system" title="user" text={json(ev)} />;
  return content.map((c, i) => c.type === "tool_result"
    ? <Block key={i} kind={c.is_error ? "result error" : "result"} title={c.is_error ? "tool result (error)" : "tool result"} text={resultText(c.content)} />
    : <Block key={i} kind="system" title={c.type} text={json(c)} />);
}

const resultText = content => Array.isArray(content) ? content.map(p => p.type === "text" ? p.text : json(p)).join("\n") : json(content ?? "");

function Result({ ev }) {
  const secs = ev.duration_ms != null ? `${(ev.duration_ms / 1000).toFixed(1)}s` : null;
  const cost = ev.total_cost_usd != null ? `$${ev.total_cost_usd.toFixed(4)}` : null;
  const parts = [ev.subtype, ev.num_turns != null && `${ev.num_turns} turns`, secs, cost].filter(Boolean).join(" · ");
  return (
    <div class={"ev final" + (ev.is_error ? " error" : "")}>
      <span class="tag">result</span> {parts}
      {ev.result && <div dangerouslySetInnerHTML={{ __html: markdown(String(ev.result)) }} />}
    </div>
  );
}
