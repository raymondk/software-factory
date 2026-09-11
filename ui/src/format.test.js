import { describe, expect, test } from "vitest";
import { ago, markdown } from "./format.js";

describe("markdown", () => {
  test("escapes html and wraps paragraphs", () => {
    expect(markdown("a <b> & \"c\"\n\nd")).toBe("<p>a &lt;b&gt; &amp; &quot;c&quot;</p><p>d</p>");
  });
  test("renders lists, inline code and links", () => {
    expect(markdown("- one `x`\n- [two](https://t.io/a)")).toBe('<ul><li>one <code>x</code></li><li><a href="https://t.io/a" target="_blank" rel="noopener">two</a></li></ul>');
    expect(markdown("see https://t.io/a.")).toBe('<p>see <a href="https://t.io/a" target="_blank" rel="noopener">https://t.io/a</a>.</p>');
  });
  test("keeps fenced code verbatim", () => {
    expect(markdown("```js\n- not a list\n\nx\n```")).toBe("<pre><code>- not a list\n\nx</code></pre>");
  });
});

test("ago", () => {
  const now = Date.parse("2026-01-10T12:00:00Z");
  const at = s => new Date(now - s * 1000).toISOString();
  expect(ago(at(5), now)).toBe("just now");
  expect(ago(at(120), now)).toBe("2 min ago");
  expect(ago(at(7200), now)).toBe("2 h ago");
  expect(ago(at(100000), now)).toBe("yesterday");
  expect(ago(at(300000), now)).toMatch(/Jan 7/);
});
