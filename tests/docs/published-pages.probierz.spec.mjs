import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { componentStylesPath, pages, renderPage, renderSitemap } from "../../web/docs/pages.mjs";

// jeden.wisent.com is rendered by the Vercel build from the documentation
// sources pushed to main; no rendered file is committed. Until 2026-09-23 a
// person rendered and committed the HTML by hand, and the task contract's
// time-to-completion change reached web/docs/content while production kept
// serving the previous pages. Production therefore has to serve, byte for
// byte, what the sources of the revision under test render.

const origin = new URL(pages[0].meta.canonical).origin;
const revision = execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();

// Characters quoted on either side of the first difference: enough to find
// the sentence in web/docs, short enough to read in a failure line.
const CONTEXT = 80;

async function served(url) {
  const response = await fetch(url, { redirect: "follow" });
  assert.equal(response.status, 200, `${url} returned ${response.status}`);
  assert.equal(response.url, url, `${url} redirected to ${response.url}`);
  return response.text();
}

function firstDifference(production, rendered) {
  let at = 0;
  while (at < production.length && at < rendered.length && production[at] === rendered[at]) at += 1;
  const from = Math.max(0, at - CONTEXT);
  return `from character ${at} production serves ${JSON.stringify(production.slice(from, at + CONTEXT))} where the sources render ${JSON.stringify(rendered.slice(from, at + CONTEXT))}`;
}

function assertSame(url, production, rendered) {
  assert(
    production === rendered,
    `${url} is not what web/docs renders at ${revision}; ${firstDifference(production, rendered)}`,
  );
}

test("production serves every documentation page as the sources of this revision render it", async () => {
  for (const [index, page] of pages.entries()) {
    assertSame(page.meta.canonical, await served(page.meta.canonical), renderPage(index));
  }
});

test("production serves the sitemap and the component stylesheet the build writes", async () => {
  const sitemap = `${origin}/sitemap.xml`;
  assertSame(sitemap, await served(sitemap), renderSitemap());
  const styles = `${origin}/wisent-components.css`;
  assertSame(styles, await served(styles), await readFile(componentStylesPath(), "utf8"));
});
