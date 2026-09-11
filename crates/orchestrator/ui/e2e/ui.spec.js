import { test as base, expect } from "@playwright/test";
import { spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";

const root = path.join(import.meta.dirname, "..", "..", "..", "..");
const TOKEN = "change-me";
const freePort = () => new Promise(resolve => {
  const s = net.createServer().listen(0, () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

// Builds a temp config from factory.example.toml (database defaults to next to it, provider pointed at a dead port), starts the orchestrator, kills it after.
const test = base.extend({
  server: [async ({}, use) => {
    const port = await freePort();
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ui-tests-"));
    const config = path.join(dir, "factory.toml");
    fs.writeFileSync(config, fs.readFileSync(path.join(root, "factory.example.toml"), "utf8").replace('listen = "0.0.0.0:8080"', `listen = "127.0.0.1:${port}"`).replace("http://localhost:8081", "http://127.0.0.1:1"));
    const proc = spawn(path.join(root, "target/debug/orchestrator"), [config], { stdio: ["ignore", "ignore", "inherit"] });
    const url = `http://127.0.0.1:${port}`;
    const api = async (p, opts = {}) => {
      const r = await fetch(url + p, { ...opts, headers: { Authorization: "Bearer " + TOKEN, "Content-Type": "application/json" }, body: opts.body && JSON.stringify(opts.body) });
      if (!r.ok) throw new Error(`${opts.method ?? "GET"} ${p}: ${r.status}`);
      return r.json();
    };
    for (let i = 0; ; i++) {
      try { await api("/tickets"); break; } catch (e) { if (i > 100) throw new Error("orchestrator did not start: " + e.message); await new Promise(r => setTimeout(r, 100)); }
    }
    await use({ url, api });
    proc.kill();
    fs.rmSync(dir, { recursive: true, force: true });
  }, { scope: "worker" }],
  baseURL: async ({ server }, use) => use(server.url),
});

const card = (page, id) => page.locator(`.card[data-id="${id}"]`);
const create = (server, title) => server.api("/tickets", { method: "POST", body: { title } });
const ids = async (server, ...want) => (await server.api("/tickets")).map(t => t.id).filter(id => want.includes(id));

test("creates a ticket from the dialog", async ({ page }) => {
  await page.goto("/");
  await page.click("button:text-is('New ticket')");
  await expect(page.locator("#create")).toBeVisible();
  await page.fill("#create input[name=title]", "Created in browser");
  await page.click("#create button:text-is('Create ticket')");
  await expect(page.locator("#create")).toBeHidden();
  await expect(page.locator(".column.todo .card", { hasText: "Created in browser" })).toBeVisible();
  await expect(page).toHaveURL(/#\/tickets\/\d+$/);
  await expect(page.locator("#detail")).toBeVisible();
  await page.keyboard.press("Escape"); // closes the ticket that just opened
  await page.click("button:text-is('New ticket')");
  await page.keyboard.press("Escape");
  await expect(page.locator("dialog[open]")).toHaveCount(0);
});

test("opens a ticket by card click and by URL", async ({ page, server }) => {
  const t = await create(server, "Open me");
  await page.goto("/");
  await card(page, t.id).click();
  await expect(page.locator("#detail h2")).toContainText(`#${t.id} Open me`);
  await expect(page).toHaveURL(new RegExp(`#/tickets/${t.id}$`));
  await expect(card(page, t.id)).toHaveClass(/selected/);
  await page.goto(`/#/tickets/${t.id}`);
  await expect(page.locator("#detail h2")).toContainText("Open me");
  await page.click("#detail h2 button");
  await expect(page.locator("#detail")).toBeHidden();
  await expect(page).toHaveURL(/\/#?$/);
  await page.goBack();
  await expect(page.locator("#detail h2")).toContainText("Open me");
  await page.keyboard.press("Escape");
  await expect(page.locator("#detail")).toBeHidden();
});

test("edits title and state from the form and the Mark ready action", async ({ page, server }) => {
  const t = await create(server, "Edit me");
  await page.goto(`/#/tickets/${t.id}`);
  await page.fill("#detail input[name=title]", "Edited");
  await page.selectOption("#detail select[name=state]", "in_review");
  await page.click("#detail button:text-is('Save')");
  await expect(page.locator(".column.in_review .card", { hasText: "Edited" })).toBeVisible();
  await page.click("#actions button:text-is('Back to todo')");
  await expect(page.locator(".column.todo .card", { hasText: "Edited" })).toBeVisible();
  await page.click("#actions button:text-is('Mark ready')");
  await expect(page.locator(".column.ready .card", { hasText: "Edited" })).toBeVisible();
  expect(await server.api(`/tickets/${t.id}`)).toMatchObject({ title: "Edited", state: "ready" });
});

test("adds and resolves a comment", async ({ page, server }) => {
  const t = await create(server, "Comment me");
  await page.goto(`/#/tickets/${t.id}`);
  await page.fill("textarea[name=body]", "Please **check** this");
  await page.click("button:text-is('Comment')");
  const comment = page.locator(".comment.human");
  await expect(comment).toContainText("Please **check** this");
  await expect(card(page, t.id).locator(".badge")).toHaveText("1");
  await comment.locator("button:text-is('Resolve')").click();
  await expect(card(page, t.id).locator(".badge")).toHaveCount(0);
  await expect(page.locator("button:text-is('Show 1 resolved')")).toBeVisible();
  expect((await server.api(`/tickets/${t.id}`)).comments[0].resolved).toBe(true);
});

test("reorders with Alt+ArrowDown and by dragging", async ({ page, server }) => {
  const a = await create(server, "First"), b = await create(server, "Second");
  await page.goto("/");
  await card(page, a.id).focus();
  await page.keyboard.press("Alt+ArrowDown");
  await expect.poll(() => ids(server, a.id, b.id)).toEqual([b.id, a.id]);
  await expect(page.locator(".column.todo .card").filter({ hasText: /First|Second/ }).first()).toHaveText(/Second/);
  await card(page, a.id).dragTo(card(page, b.id), { targetPosition: { x: 10, y: 2 } });
  await expect.poll(() => ids(server, a.id, b.id)).toEqual([a.id, b.id]);
});

test("shows a worker in the Workers panel", async ({ page, server }) => {
  const w = await server.api("/workers", { method: "POST", body: { worker_type: "default" } });
  await page.goto("/");
  const row = page.locator("#workers tr", { hasText: w.id });
  await expect(row).toContainText("default");
  await expect(row).toContainText("starting");
});

test("shows a failed create inside the dialog and keeps the input", async ({ page }) => {
  await page.route("**/tickets", (route, req) =>
    req.method() === "POST" ? route.fulfill({ status: 400, contentType: "application/json", body: '{"error":"forced failure"}' }) : route.continue());
  await page.goto("/");
  await page.click("button:text-is('New ticket')");
  await page.fill("#create input[name=title]", "Will fail");
  await page.click("#create button:text-is('Create ticket')");
  await expect(page.locator("dialog .error")).toContainText("forced failure");
  await expect(page.locator("#create input[name=title]")).toHaveValue("Will fail");
  await expect(page.locator("#error")).toBeEmpty();
});

test("shows an inline error when a refresh fails", async ({ page }) => {
  await page.goto("/");
  await page.route("**/workers", route => route.fulfill({ status: 500, contentType: "application/json", body: '{"error":"forced failure"}' }));
  await expect(page.locator("#error")).toContainText("forced failure");
  await page.click("#error button");
  await expect(page.locator("#error")).toBeEmpty();
});
