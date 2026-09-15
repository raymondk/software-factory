import { cleanup, fireEvent, render, screen } from "@testing-library/preact";
import { afterEach, expect, test, vi } from "vitest";
import { Ctx } from "./context.js";
import { Detail } from "./Detail.jsx";

const ticket = (over = {}) => ({ id: 1, title: "T", state: "todo", description: "", links: [], rank: 1, assignee: null,
                                 created_at: "", updated_at: "", comments: [], relations: [], blocked: false, runs: [], ...over });
const usage = { tokens_in: 0, tokens_out: 0, cost: 0 };
const ctx = { refresh: vi.fn(), select: vi.fn(), showError: vi.fn() };
const mount = t => render(<Ctx.Provider value={ctx}><Detail ticket={t} usage={usage} /></Ctx.Provider>);
afterEach(cleanup);

test("an untouched form follows the server", () => {
  const { rerender, container } = mount(ticket());
  rerender(<Ctx.Provider value={ctx}><Detail ticket={ticket({ title: "T2" })} usage={usage} /></Ctx.Provider>);
  expect(container.querySelector("input[name=title]").value).toBe("T2");
  expect(container.querySelector(".notice").hidden).toBe(true);
});

test("an edited form keeps the user's text and offers a reload", () => {
  const { rerender, container } = mount(ticket());
  const title = container.querySelector("input[name=title]");
  fireEvent.input(title, { target: { value: "mine" } });
  rerender(<Ctx.Provider value={ctx}><Detail ticket={ticket({ title: "theirs" })} usage={usage} /></Ctx.Provider>);
  expect(title.value).toBe("mine");
  const notice = container.querySelector(".notice");
  expect(notice.hidden).toBe(false);
  fireEvent.click(notice.querySelector("button"));
  expect(title.value).toBe("theirs");
  expect(notice.hidden).toBe(true);
});

test("resolved comments are hidden behind a toggle", () => {
  const comments = [{ id: 1, author: "Alice", body: "open", resolved: false, created_at: new Date().toISOString() },
                    { id: 2, author: "w-0a1b2c3d", body: "closed", resolved: true, created_at: new Date().toISOString() }];
  const { container } = mount(ticket({ comments }));
  expect([...container.querySelectorAll(".comment")].map(c => c.className.includes("worker"))).toEqual([false, true]);
  expect(container.querySelector(".comment.resolved").hidden).toBe(true);
  fireEvent.click(screen.getByText("Show 1 resolved"));
  expect(container.querySelector(".comment.resolved").hidden).toBe(false);
  expect(screen.getByText("Hide 1 resolved")).toBeTruthy();
});

test("relations are grouped and a blocked ticket names its blockers", () => {
  const relations = [{ type: "depends_on", ticket: 2, title: "dep", state: "todo", satisfied: false },
                     { type: "depends_on", ticket: 3, title: "done dep", state: "done", satisfied: true },
                     { type: "blocks", ticket: 4, title: "later", state: "ready" },
                     { type: "related_to", ticket: 5, title: "kin", state: "todo" }];
  const { container } = mount(ticket({ state: "ready", relations, blocked: true }));
  expect(container.querySelector(".blocked").textContent).toBe("blocked by #2");
  expect([...container.querySelectorAll("#relations h4")].map(h => h.textContent)).toEqual(["Depends on", "Blocks", "Related to"]);
  expect([...container.querySelectorAll(".relation.blocking a")].map(a => a.textContent)).toEqual(["#2 dep"]);
  expect(container.querySelector(".relation a[href='#/tickets/5']").textContent).toBe("#5 kin");
});

test("runs are listed newest first and the selected one shows its log", () => {
  const runs = [{ id: 2, worker_id: "w-2", started_at: new Date().toISOString(), ended_at: null },
                { id: 1, worker_id: "w-1", started_at: new Date().toISOString(), ended_at: new Date().toISOString() }];
  const { container } = render(<Ctx.Provider value={ctx}><Detail ticket={ticket({ runs })} usage={usage} run={1} /></Ctx.Provider>);
  expect([...container.querySelectorAll("#runs .run a")].map(a => a.textContent)).toEqual(["run 2", "run 1"]);
  expect([...container.querySelectorAll("#runs .run .tag")].map(a => a.textContent)).toEqual(["running", "ended"]);
  const selected = [...container.querySelectorAll(".run")].filter(d => d.classList.contains("selected"));
  expect(selected.map(d => d.querySelector("a").getAttribute("href"))).toEqual(["#/tickets/1/runs/1"]);
  expect(container.querySelector(".log")).toBeTruthy();
  cleanup(); // two mounted dialogs would both carry id="runs"
  const empty = render(<Ctx.Provider value={ctx}><Detail ticket={ticket()} usage={usage} run={null} /></Ctx.Provider>).container;
  expect(empty.querySelector("section#runs .empty").textContent).toBe("No runs yet");
  expect(empty.querySelector(".log")).toBe(null);
});
