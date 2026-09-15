import { cleanup, fireEvent, render } from "@testing-library/preact";
import { afterEach, expect, test, vi } from "vitest";
import { AgentModel } from "./AgentModel.jsx";

const agents = { "claude-code": ["sonnet", "opus"], codex: ["o3"] };
const texts = sel => [...sel.querySelectorAll("option")].map(o => o.value);
afterEach(cleanup);

test("models follow the agent; any agent offers every model", () => {
  const { container, rerender } = render(<AgentModel agents={agents} agent="" model="" onChange={() => {}} />);
  expect(texts(container.querySelector("select[name=model]"))).toEqual(["", "sonnet", "opus", "o3"]);
  rerender(<AgentModel agents={agents} agent="codex" model="" onChange={() => {}} />);
  expect(texts(container.querySelector("select[name=model]"))).toEqual(["", "o3"]);
});

test("picking an agent that cannot run the model clears it; an unadvertised value stays selectable", () => {
  const onChange = vi.fn();
  const { container } = render(<AgentModel agents={agents} agent="" model="opus" onChange={onChange} />);
  fireEvent.change(container.querySelector("select[name=agent]"), { target: { value: "codex" } });
  expect(onChange).toHaveBeenCalledWith({ agent: "codex", model: "" });
  cleanup();
  const { container: c } = render(<AgentModel agents={agents} agent="gone" model="old" onChange={onChange} />);
  expect(c.querySelector("select[name=agent]").value).toBe("gone");
  expect(c.querySelector("select[name=model]").value).toBe("old");
});
