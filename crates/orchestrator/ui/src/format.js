export const STATES = ["todo", "ready", "in_progress", "in_review", "failed", "done"];
// One-click state moves shown in the detail pane per current state.
export const ACTIONS = { todo: [["Mark ready", "ready"]], ready: [["Back to todo", "todo"]],
                         in_review: [["Done", "done"], ["Back to todo", "todo"]], failed: [["Retry", "ready"]] };

export const isHttp = href => /^https?:\/\//.test(href);
// A user's name, or a shortened principal when the user is unknown (pending, revoked, or the list not loaded).
export const userName = (users, principal) => users.find(u => u.principal === principal)?.name ?? principal.replace(/^([^-]+-[^-]+)-.+$/, "$1…");
export const usageLine = u => `tokens in: ${u.tokens_in} · tokens out: ${u.tokens_out} · cost: $${u.cost.toFixed(2)}`;

export const ago = (iso, now = Date.now()) => {
  const s = (now - Date.parse(iso)) / 1000;
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  if (s < 172800) return "yesterday";
  return new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric" });
};

// Minimal markdown: escape first, then fenced code, paragraphs, lists, inline code, http(s) links. `[text](#/...)`
// links into this UI open in the same tab; external ones in a new tab.
const esc = s => s.replace(/[&<>"]/g, ch => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[ch]));
const link = (href, text) => href.startsWith("#") ? `<a href="${href}">${text}</a>` : `<a href="${href}" target="_blank" rel="noopener">${text}</a>`;
const inline = s => {
  const codes = [];
  return s.replace(/`([^`\n]+)`/g, (_, c) => `\0${codes.push(`<code>${c}</code>`) - 1}\0`)
    .replace(/\[([^\]\n]+)\]\(((?:https?:\/\/|#\/)[^\s)]+)\)|https?:\/\/[^\s<]+?(?=[.,;:!?)]*(?:\s|$))/g,
             (m, text, href) => link(href ?? m, text ?? m))
    .replace(/\0(\d+)\0/g, (_, i) => codes[i]);
};
export const markdown = src => esc(src).split(/^```[^\n]*\n([\s\S]*?)\n?^```[ \t]*$/m).map((part, i) =>
  i % 2 ? `<pre><code>${part}</code></pre>` : part.split(/\n[ \t]*\n/).map(b => b.trim()).filter(Boolean).map(block => {
    const lines = block.split("\n");
    if (lines.every(l => /^\s*[-*] /.test(l))) return `<ul>${lines.map(l => `<li>${inline(l.replace(/^\s*[-*] /, ""))}</li>`).join("")}</ul>`;
    return `<p>${inline(block)}</p>`;
  }).join("")).join("");
