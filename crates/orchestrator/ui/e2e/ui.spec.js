import { test as base, expect } from "@playwright/test";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

const root = path.join(import.meta.dirname, "..", "..", "..", "..");
const TOKEN = "change-me";
const freePort = () => new Promise(resolve => {
  const s = net.createServer().listen(0, () => { const { port } = s.address(); s.close(() => resolve(port)); });
});

// What the fake provider advertises: agents and models the UI can set on tickets. Capacity 0, so nothing is ever started.
const STATUS = { capacity: 0, in_use: 0, workers: [], agents: { "claude-code": { models: ["sonnet", "opus"], default_model: "sonnet" }, codex: { models: ["o3"], default_model: "o3" } } };

// Builds a temp config from factory.example.toml (database defaults to next to it), starts the orchestrator, registers a fake provider that only answers /status as the developer, kills it all after.
const test = base.extend({
  server: [async ({}, use) => {
    const port = await freePort();
    const provider = http.createServer((req, res) => {
      const ok = req.method === "GET" && req.url === "/status";
      res.writeHead(ok ? 200 : 404, { "content-type": "application/json" }).end(JSON.stringify(ok ? STATUS : { error: "no" }));
    });
    await new Promise(r => provider.listen(0, "127.0.0.1", r));
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ui-tests-"));
    const config = path.join(dir, "factory.toml");
    fs.writeFileSync(config, fs.readFileSync(path.join(root, "factory.example.toml"), "utf8").replace('listen = "0.0.0.0:8080"', `listen = "127.0.0.1:${port}"`).replace('# interval = "10s"', 'interval = "1s"'));
    const proc = spawn(path.join(root, "target/debug/orchestrator"), [config], { stdio: ["ignore", "ignore", "inherit"] });
    // Signs `principal` in the way the login endpoint will, straight into the database: a users row (pending unless
    // already there) and a session. Returns the session token.
    const session = principal => {
      const db = new DatabaseSync(path.join(dir, "factory.db"));
      db.exec("PRAGMA busy_timeout = 5000"); // the orchestrator writes too
      const token = `session-${principal}-${Date.now()}`;
      db.prepare("INSERT OR IGNORE INTO users (principal, status, created_at) VALUES (?, 'pending', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))").run(principal);
      db.prepare("INSERT INTO sessions (token_hash, principal, created_at, expires_at) VALUES (?, ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+8 hours'))")
        .run(createHash("sha256").update(token).digest("hex"), principal);
      db.close();
      return token;
    };
    const url = `http://127.0.0.1:${port}`;
    const api = async (p, opts = {}) => {
      const r = await fetch(url + p, { ...opts, headers: { Authorization: "Bearer " + (opts.token ?? TOKEN), "Content-Type": "application/json" }, body: opts.body && JSON.stringify(opts.body) });
      if (!r.ok) throw new Error(`${opts.method ?? "GET"} ${p}: ${r.status}`);
      return r.json();
    };
    for (let i = 0; ; i++) {
      try { await api("/tickets"); break; } catch (e) { if (i > 100) throw new Error("orchestrator did not start: " + e.message); await new Promise(r => setTimeout(r, 100)); }
    }
    // The developer the browser is signed in as, unless a test says otherwise. The fake provider is theirs.
    await api("/users/dev-principal/approve", { method: "POST", body: { name: "Dev" } });
    const dev = session("dev-principal");
    await api("/providers", { method: "POST", token: dev, body: { name: "fake", url: `http://127.0.0.1:${provider.address().port}`, token: "x" } });
    // Past the scheduler pass that fetches what the provider advertises.
    for (let i = 0; ; i++) {
      try { await api("/tickets", { method: "POST", token: dev, body: { title: "probe", agent: "claude-code" } }); break; } catch (e) { if (i > 100) throw new Error("provider status was never fetched: " + e.message); await new Promise(r => setTimeout(r, 100)); }
    }
    await use({ url, api, session, dev });
    proc.kill();
    provider.close();
    fs.rmSync(dir, { recursive: true, force: true });
  }, { scope: "worker" }],
  baseURL: async ({ server }, use) => use(server.url),
  // Signed in as Dev: the session token sits in localStorage as the login flow leaves it. Internet Identity itself is
  // not driven here.
  page: async ({ page, server }, use) => {
    await page.addInitScript(t => localStorage.setItem("factory.token", t), server.dev);
    await use(page);
  },
});
const signedInAs = (page, token) => page.addInitScript(t => localStorage.setItem("factory.token", t), token);

const card = (page, id) => page.locator(`.card[data-id="${id}"]`);
const create = (server, title) => server.api("/tickets", { method: "POST", body: { title } });
const ids = async (server, ...want) => (await server.api("/tickets")).map(t => t.id).filter(id => want.includes(id));

test("creates a ticket from the dialog", async ({ page, server }) => {
  await page.goto("/");
  await page.click("button:text-is('New ticket')");
  await expect(page.locator("#create")).toBeVisible();
  await page.fill("#create input[name=title]", "Created in browser");
  await page.selectOption("#create select[name=state]", "ready");
  await page.selectOption("#create select[name=agent]", "codex");
  await expect(page.locator("#create select[name=model] option")).toHaveText(["Model (default)", "o3"]);
  await page.click("#create button:text-is('Create ticket')");
  await expect(page.locator("#create")).toBeHidden();
  await expect(page.locator(".column.ready .card", { hasText: "Created in browser" })).toBeVisible();
  const created = (await server.api("/tickets")).find(t => t.title === "Created in browser");
  expect([created.agent, created.model]).toEqual(["codex", null]);
  await expect(page).toHaveURL(/\/$/);
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
  await page.click("#detail header button");
  await expect(page.locator("#detail")).toBeHidden();
  await expect(page).toHaveURL(/\/#?$/);
  await page.goBack();
  await expect(page.locator("#detail h2")).toContainText("Open me");
  await page.keyboard.press("Escape");
  await expect(page.locator("#detail")).toBeHidden();
});

test("edits title and state from the form and the Mark ready action", async ({ page, server }) => {
  // The developer's own ticket: agent and model are validated against their providers.
  const t = await server.api("/tickets", { method: "POST", token: server.dev, body: { title: "Edit me" } });
  await page.goto(`/#/tickets/${t.id}`);
  await page.fill("#detail input[name=title]", "Edited");
  await page.selectOption("#detail select[name=state]", "in_review");
  await page.selectOption("#detail select[name=agent]", "claude-code");
  await page.selectOption("#detail select[name=model]", "opus");
  await page.click("#detail button:text-is('Save')");
  await expect(page.locator("#detail .saved")).toHaveText("Saved");
  await expect(page.locator(".column.in_review .card", { hasText: "Edited" })).toBeVisible();
  expect(await server.api(`/tickets/${t.id}`)).toMatchObject({ agent: "claude-code", model: "opus" });
  await page.selectOption("#detail select[name=model]", "");
  await page.click("#detail button:text-is('Save')");
  await expect.poll(async () => (await server.api(`/tickets/${t.id}`)).model).toBe(null);
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

test("relates tickets and marks the blocked one", async ({ page, server }) => {
  const dep = await create(server, "Dependency");
  const t = await server.api("/tickets", { method: "POST", body: { title: "Needs it", state: "ready" } });
  await page.goto(`/#/tickets/${t.id}`);
  await page.selectOption("#relations select", "depends_on");
  await page.fill("#relations input[name=ticket]", String(dep.id));
  await page.click("#relations button:text-is('Add')");
  const rel = page.locator("#relations .relation", { hasText: `#${dep.id} Dependency` });
  await expect(rel).toHaveClass(/blocking/);
  await expect(page.locator("#detail .blocked")).toHaveText(`blocked by #${dep.id}`);
  await expect(card(page, t.id).locator(".blocked")).toBeVisible();
  await rel.locator("a").click();
  await expect(page.locator("#detail h2")).toContainText("Dependency");
  await expect(page.locator("#relations h4")).toHaveText("Blocks");
  await page.locator("#relations .relation button").click();
  await expect(page.locator("#relations .relation")).toHaveCount(0);
  expect((await server.api(`/tickets/${t.id}`)).relations).toEqual([]);
  await expect(card(page, t.id).locator(".blocked")).toHaveCount(0);
});

test("shows a run's log live and a worker's whole log", async ({ page, server }) => {
  await server.api("/tickets", { method: "POST", token: server.dev, body: { title: "Logged", state: "ready" } });
  const w = await server.api("/workers", { method: "POST", body: { agent: "claude-code" } }); // on the developer's provider
  const asWorker = async (p, body) => {
    const r = await fetch(server.url + p, { method: "POST", headers: { Authorization: "Bearer " + w.token, "Content-Type": "application/json" }, body: JSON.stringify(body) });
    return r.status === 204 ? null : r.json();
  };
  await asWorker(`/workers/${w.id}/logs`, { run: null, lines: ["worker starting"] });
  const job = await asWorker(`/workers/${w.id}/poll`, {}); // the lowest-ranked ready ticket, not necessarily `t`
  const id = job.ticket.id;
  await asWorker(`/workers/${w.id}/logs`, { run: job.run, lines: ["first line"] });
  await page.goto(`/#/tickets/${id}`);
  await page.click(`#runs a:text-is("run ${job.run}")`);
  await expect(page).toHaveURL(new RegExp(`#/tickets/${id}/runs/${job.run}$`));
  const log = page.locator("#detail .log");
  await expect(log).toContainText("first line");
  const ev = JSON.stringify({ type: "assistant", message: { content: [{ type: "tool_use", id: "t1", name: "Bash", input: { command: "ls" } }] } });
  await asWorker(`/workers/${w.id}/logs`, { run: job.run, lines: ["second line", ev] });
  await expect(log).toContainText("second line");
  await expect(log).not.toContainText("worker starting");
  // The worker's agent is claude-code, so the stream-json line renders as a tool block until toggled to raw.
  await expect(log.locator(".ev.tool summary")).toHaveText("Bash");
  await page.click("#detail button.log-view:text-is(\"Raw\")");
  await expect(log.locator(".ev")).toHaveCount(0);
  await expect(log).toContainText(ev);
  await page.goto("/");
  await page.click(`#workers tr:has-text("${w.id}") a:text-is("Log")`);
  await expect(page).toHaveURL(new RegExp(`#/workers/${w.id}$`));
  await expect(page.locator("#worker-log .log")).toContainText("worker starting");
  await expect(page.locator("#worker-log .log")).toContainText("second line");
  // The worker's stream is pretty too: its own lines raw, the agent's event as a block.
  await expect(page.locator("#worker-log .ev.tool summary")).toHaveText("Bash");
  await expect(page.locator("#worker-log button.log-view")).toHaveText("Raw");
  await page.keyboard.press("Escape");
  await expect(page.locator("#worker-log")).toBeHidden();
  await expect(page).toHaveURL(/\/#?$/);
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

test("opens the configuration read-only from the cog, without the token", async ({ page }) => {
  await page.goto("/");
  const dialog = page.locator("#config");
  await expect(dialog).toBeHidden();
  await page.click("#settings");
  await expect(dialog).toBeVisible();
  await expect(dialog.locator(".panel h3")).toHaveText(["Project", "Orchestrator", "Scheduler", "Agents", "Prompts"]);
  await expect(dialog).toContainText("{{ticket.id}}");
  await expect(dialog).not.toContainText(TOKEN);
  await expect(dialog.locator("input, textarea, select, form")).toHaveCount(0);
  const repo = dialog.locator("a", { hasText: "repo-a" });
  await expect(repo).toHaveAttribute("href", "https://github.com/org/repo-a.git");
  await expect(repo).toHaveAttribute("target", "_blank");
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
});

test("lists every developer's providers with status; the developer adds and removes their own", async ({ page, server }) => {
  await page.goto("/");
  const rows = page.locator("#providers tr");
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toContainText("fake");
  await expect(rows.first()).toContainText("Dev");
  await expect(rows.first()).toContainText("0 / 0");
  await expect(rows.first()).toContainText("claude-code");
  await expect(rows.first()).toContainText("sonnet, opus");
  await expect(rows.first().locator("a")).toHaveAttribute("href", /\/status$/);
  await page.fill("#add-provider input[name=name]", "spare");
  await page.fill("#add-provider input[name=url]", "http://127.0.0.1:1");
  await page.fill("#add-provider input[name=token]", "s3cret");
  await page.click("#add-provider button:text-is('Add provider')");
  const spare = page.locator("#providers tr", { hasText: "spare" });
  await expect(spare).toContainText("no status yet");
  expect(await page.locator("#providers-section").textContent()).not.toContain("s3cret");
  const listed = await server.api("/providers");
  expect(listed.map(p => [p.name, p.owner])).toEqual([["fake", "dev-principal"], ["spare", "dev-principal"]]);
  // A duplicate name is refused inline.
  await page.fill("#add-provider input[name=name]", "spare");
  await page.fill("#add-provider input[name=url]", "http://127.0.0.1:2");
  await page.fill("#add-provider input[name=token]", "x");
  await page.click("#add-provider button:text-is('Add provider')");
  await expect(page.locator("#providers-section .error")).toContainText("already have a provider");
  page.once("dialog", d => d.accept());
  await spare.locator("button:text-is('Remove')").click();
  await expect(rows).toHaveCount(1);
});

test("shows a worker in the Workers panel", async ({ page, server }) => {
  const w = await server.api("/workers", { method: "POST", body: { agent: "claude-code" } });
  await page.goto("/");
  const row = page.locator("#workers tr", { hasText: w.id });
  await expect(row).toContainText("claude-code");
  await expect(row).toContainText(String(w.provider));
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

test("shows the owner by name, filters by owner, and takes it over from the detail", async ({ page, server }) => {
  await server.api("/users/alice-principal/approve", { method: "POST", body: { name: "Alice" } });
  await server.api("/users/bob-principal/approve", { method: "POST", body: { name: "Bob" } });
  const alice = server.session("alice-principal");
  const owned = await server.api("/tickets", { method: "POST", body: { title: "Alice owns this" }, token: alice });
  const unowned = await create(server, "Nobody owns this");
  expect(owned.owner).toBe("alice-principal");
  await page.goto("/");
  await expect(card(page, owned.id).locator(".owner")).toHaveText("Alice");
  await expect(card(page, unowned.id).locator(".owner")).toHaveCount(0);
  await page.selectOption("#owner-filter", "alice-principal");
  await expect(card(page, owned.id)).toBeVisible();
  await expect(card(page, unowned.id)).toHaveCount(0);
  await page.selectOption("#owner-filter", "none");
  await expect(card(page, owned.id)).toHaveCount(0);
  await expect(card(page, unowned.id)).toBeVisible();
  await page.selectOption("#owner-filter", "");
  await card(page, owned.id).click();
  await expect(page.locator("#detail .pane p").first()).toContainText("owner: Alice");
  await expect(page.locator("#detail select[name=owner]")).toHaveCount(0);
  // Spec 3.4: a developer may only make themselves the owner, and the owner may release the ticket.
  await page.click("#actions button:text-is('Make me owner')");
  await expect(page.locator("#detail .pane p").first()).toContainText("owner: Dev");
  await expect(card(page, owned.id).locator(".owner")).toHaveText("Dev");
  expect((await server.api(`/tickets/${owned.id}`)).owner).toBe("dev-principal");
  await page.click("#actions button:text-is('Release ownership')");
  await expect(page.locator("#detail .pane p").first()).toContainText("owner: none");
  // Without an owner there is nothing to pick agent and model from; a held ticket offers no ownership action.
  await expect(page.locator("#detail select[name=agent]")).toBeDisabled();
  await expect(page.locator("#detail .hint")).toContainText("no owner");
  await server.api(`/tickets/${owned.id}`, { method: "PATCH", body: { assignee: "w-0a1b2c3d" } });
  await expect(page.locator("#actions button:text-is('Make me owner')")).toHaveCount(0);
});

test("without a session the sign-in screen shows and nothing else loads", async ({ page }) => {
  await page.addInitScript(() => localStorage.removeItem("factory.token"));
  const calls = [];
  await page.route("**/tickets", route => { calls.push(route.request().url()); route.continue(); });
  await page.goto("/");
  await expect(page.locator("button:text-is('Sign in with Internet Identity')")).toBeVisible();
  await expect(page.locator("#board")).toHaveCount(0);
  expect(await page.content()).not.toContain(TOKEN);
  expect(calls).toEqual([]);
});

test("an expired session drops back to the sign-in screen", async ({ page, server }) => {
  await signedInAs(page, "no-such-session");
  await page.goto("/");
  await expect(page.locator("button:text-is('Sign in with Internet Identity')")).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem("factory.token"))).toBe(null);
  // A revoked session mid-use does the same.
  await signedInAs(page, server.dev);
  await page.goto("/");
  await expect(page.locator("#board")).toBeVisible();
  await page.route("**/tickets", route => route.fulfill({ status: 401, contentType: "application/json", body: '{"error":"unauthorized"}' }));
  await expect(page.locator("button:text-is('Sign in with Internet Identity')")).toBeVisible();
});

test("a pending user waits with their principal until the admin approves", async ({ page, server }) => {
  await signedInAs(page, server.session("newbie-principal"));
  await page.goto("/");
  const waiting = page.locator("main.login");
  await expect(waiting).toContainText("Waiting for approval");
  await expect(waiting.locator("pre")).toHaveText('factory user approve newbie-principal --name "…"');
  await expect(page.locator("#board")).toHaveCount(0);
  await server.api("/users/newbie-principal/approve", { method: "POST", body: { name: "Newbie" } });
  await expect(page.locator("#board")).toBeVisible({ timeout: 10000 });
  await expect(page.locator("#user")).toContainText("Newbie");
});

test("the header names the user; tokens are created, shown once and revoked; log out ends the session", async ({ page, server }) => {
  await page.goto("/");
  await expect(page.locator("#user")).toContainText("Dev");
  await page.click("#user button:text-is('Tokens')");
  const dialog = page.locator("#tokens");
  await expect(dialog).toContainText("No tokens yet");
  await page.fill("#tokens input[name=name]", "laptop");
  await page.click("#tokens button:text-is('Create token')");
  const once = dialog.locator(".token-once pre");
  await expect(once).toContainText("FACTORY_TOKEN=");
  const token = (await once.textContent()).replace("FACTORY_TOKEN=", "").trim();
  expect((await server.api("/me", { token })).name).toBe("Dev");
  await expect(dialog.locator("tbody tr")).toHaveText([/laptop/]);
  expect(await dialog.locator("tbody").textContent()).not.toContain(token);
  await dialog.locator("button:text-is('Revoke')").click();
  await expect(dialog).toContainText("No tokens yet");
  await expect(server.api("/me", { token })).rejects.toThrow("401");
  await page.keyboard.press("Escape");
  const session = await page.evaluate(() => localStorage.getItem("factory.token"));
  await page.click("#user button:text-is('Log out')");
  await expect(page.locator("button:text-is('Sign in with Internet Identity')")).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem("factory.token"))).toBe(null);
  await expect(server.api("/me", { token: session })).rejects.toThrow("401");
});
