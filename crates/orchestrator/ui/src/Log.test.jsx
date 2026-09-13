import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/preact";
import { afterEach, expect, test, vi } from "vitest";
import { claudeCode } from "./claude-code.jsx";
import { LogPane } from "./Log.jsx";

vi.mock("./api.js", () => ({ api: vi.fn() }));
import { api } from "./api.js";

afterEach(cleanup);

const evs = [
  JSON.stringify({ type: "system", subtype: "init", model: "m", tools: ["Bash"] }),
  JSON.stringify({ type: "assistant", message: { content: [{ type: "thinking", thinking: "hmm" }, { type: "text", text: "Hello **there**" },
                                                           { type: "tool_use", id: "t1", name: "Bash", input: { command: "ls -la" } }] } }),
  JSON.stringify({ type: "user", message: { content: [{ type: "tool_result", tool_use_id: "t1", content: "a.txt\nb.txt", is_error: false }] } }),
  JSON.stringify({ type: "user", message: { content: [{ type: "tool_result", tool_use_id: "t2", content: [{ type: "text", text: "boom" }], is_error: true }] } }),
  JSON.stringify({ type: "result", subtype: "success", is_error: false, duration_ms: 5065, num_turns: 3, total_cost_usd: 0.2187, result: "done" }),
];

test("renders every Claude Code event and leaves other lines raw", () => {
  const { container } = render(<div>{evs.map(claudeCode)}</div>);
  expect(container.querySelector(".ev.system summary").textContent).toBe("system init");
  expect(container.querySelector(".ev.system").open).toBe(false);
  expect(container.querySelector(".ev.thinking pre").textContent).toBe("hmm");
  expect(container.querySelector(".ev.text").innerHTML).toBe("<p>Hello **there**</p>");
  expect(container.querySelector(".ev.tool summary").textContent).toBe("Bash");
  expect(container.querySelector(".ev.tool pre").textContent).toBe("ls -la");
  expect(container.querySelector(".ev.tool").open).toBe(true);
  const results = container.querySelectorAll(".ev.result");
  expect(results[0].querySelector("pre").textContent).toBe("a.txt\nb.txt");
  expect(results[1].classList.contains("error")).toBe(true);
  expect(results[1].querySelector("summary").textContent).toBe("tool result (error)");
  expect(container.querySelector(".ev.final").textContent).toBe("result success · 3 turns · 5.1s · $0.2187done");
  expect(claudeCode("worker w-1: polling")).toBe(null);
  expect(claudeCode('{"type":"unknown"}')).toBe(null);
  expect(claudeCode("[1,2]")).toBe(null);
});

test("a known agent opens pretty with a toggle to raw; an unknown one is raw only", async () => {
  const lines = [{ id: 1, line: "worker starting" }, { id: 2, line: evs[4] }];
  api.mockImplementation(async p => lines.filter(l => l.id > Number(p.split("after=")[1])));
  const { container, unmount } = render(<LogPane path="/runs/1/logs" live={false} agent="claude-code" />);
  await waitFor(() => expect(container.querySelector(".ev.final")).toBeTruthy());
  expect(container.querySelector(".log.pretty .raw").textContent).toBe("worker starting");
  fireEvent.click(screen.getByText("Raw"));
  expect(container.querySelector(".ev.final")).toBe(null);
  expect(container.querySelectorAll(".raw")[1].textContent).toBe(evs[4]);
  expect(screen.getByText("Pretty")).toBeTruthy();
  unmount();
  const other = render(<LogPane path="/runs/2/logs" live={false} agent="other" />);
  await waitFor(() => expect(other.container.querySelectorAll(".raw").length).toBe(2));
  expect(other.container.querySelector(".log-view")).toBe(null);
  expect(other.container.querySelector(".log.pretty")).toBe(null);
});
