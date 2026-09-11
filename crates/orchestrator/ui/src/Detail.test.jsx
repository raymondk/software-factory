import { fireEvent, render, screen } from "@testing-library/preact";
import { expect, test, vi } from "vitest";
import { Ctx } from "./context.js";
import { Detail } from "./Detail.jsx";

const ticket = (over = {}) => ({ id: 1, title: "T", state: "todo", description: "", links: [], rank: 1, assignee: null,
                                 created_at: "", updated_at: "", comments: [], ...over });
const usage = { tokens_in: 0, tokens_out: 0, cost: 0 };
const ctx = { refresh: vi.fn(), select: vi.fn(), showError: vi.fn() };
const mount = t => render(<Ctx.Provider value={ctx}><Detail ticket={t} usage={usage} /></Ctx.Provider>);

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
  const comments = [{ id: 1, author: "human", body: "open", resolved: false, created_at: new Date().toISOString() },
                    { id: 2, author: "w1", body: "closed", resolved: true, created_at: new Date().toISOString() }];
  const { container } = mount(ticket({ comments }));
  expect(container.querySelector(".comment.resolved").hidden).toBe(true);
  fireEvent.click(screen.getByText("Show 1 resolved"));
  expect(container.querySelector(".comment.resolved").hidden).toBe(false);
  expect(screen.getByText("Hide 1 resolved")).toBeTruthy();
});
