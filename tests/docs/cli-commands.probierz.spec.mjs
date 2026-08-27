import assert from "node:assert/strict";
import test from "node:test";

export const PRODUCTION_ORIGIN = "https://jeden.wisent.com";

export const EXPECTED_CLI_ROUTES = [
  ["/docs/cli/run", 'jeden run "task" [--json] [--model-only] [--cwd path] [--model name] [--max-tokens n] [--allow-write] [--allow-command] [--yolo|--auto-approve] [--max-steps n]'],
  ["/docs/cli/pursue", 'jeden pursue "rough objective" [--json] [--cwd path] [--model name] [--allow-write] [--allow-command] [--yolo|--auto-approve] [--max-steps n]'],
  ["/docs/cli/rpc", "jeden rpc"],
  ["/docs/cli/headless", "jeden headless <addr> <server-cert.pem> <server-key.pem> <client-ca.pem> <identity-map.json> [revoked-serials.txt]"],
  ["/docs/cli/acp", "jeden acp"],
  ["/docs/cli/collab-relay", "jeden collab-relay [addr]"],
  ["/docs/cli/sessions", "jeden sessions [limit]"],
  ["/docs/cli/show", "jeden show <session-id-or-path>"],
  ["/docs/cli/export", "jeden export <session-id-or-path> [output] [--html|--markdown]"],
  ["/docs/cli/artifacts", "jeden artifacts <session-id-or-path>"],
  ["/docs/cli/artifact", "jeden artifact <session-id-or-path> <name> [output]"],
  ["/docs/cli/tools", "jeden tools [--json] [--cwd path]"],
  ["/docs/cli/search-sessions", 'jeden search-sessions "query" [limit]'],
  ["/docs/cli/resume", 'jeden resume <session-id-or-path> ["task"] [--allow-write] [--allow-command] [--yolo|--auto-approve]'],
  ["/docs/cli/recall_conversation", "jeden recall_conversation <session-id-or-path>"],
  ["/docs/cli/update", "JEDEN_UPDATE_MANIFEST=<https-or-local-dsse-manifest> jeden update"],
  ["/docs/cli/config", "jeden config [list|path|get <key>|set <key> <value>|reset <key>] [--json] [--cwd path]"],
  ["/docs/cli/doctor", "jeden doctor [--json] [--cwd path]"],
  ["/docs/cli/conformance", "jeden conformance [--json] [--cwd path]"],
  ["/docs/cli/probierz", "jeden probierz [args...]"],
  ["/docs/cli/capabilities", "jeden capabilities [--json] [--cwd path]"],
  ["/docs/cli/completions", "jeden completions <bash|zsh|fish>"],
  ["/docs/cli/worktree", "jeden worktree [list|clear] [--dry-run] [--json] [--cwd path]"],
  ["/docs/cli/token", "jeden token [--list] [--reveal] [--json]"],
  ["/docs/cli/stats", "jeden stats [--json|--summary|--serve [--port N]]"],
  ["/docs/cli/gallery", "jeden gallery [--theme NAME|--all] [--color]"],
  ["/docs/cli/roadmap", "jeden roadmap <list|show|add|drop|start|implemented|block|pass|status|depends|undepends|graph|acceptance|check|work> [args] [--json] [--cwd path]"],
  ["/docs/cli/config/list", "jeden config list [--json] [--cwd path]"],
  ["/docs/cli/config/path", "jeden config path"],
  ["/docs/cli/config/get", "jeden config get <key> [--json] [--cwd path]"],
  ["/docs/cli/config/set", "jeden config set <key> <value> [--json]"],
  ["/docs/cli/config/reset", "jeden config reset <key> [--json]"],
  ["/docs/cli/completions/bash", "jeden completions bash"],
  ["/docs/cli/completions/zsh", "jeden completions zsh"],
  ["/docs/cli/completions/fish", "jeden completions fish"],
  ["/docs/cli/worktree/list", "jeden worktree list [--json] [--cwd path]"],
  ["/docs/cli/worktree/clear", "jeden worktree clear [--dry-run] [--json] [--cwd path]"],
  ["/docs/cli/roadmap/list", "jeden roadmap list [--status STATUS] [--area AREA] [--priority PRIORITY] [--json]"],
  ["/docs/cli/roadmap/show", "jeden roadmap show <id> [--json]"],
  ["/docs/cli/roadmap/add", "jeden roadmap add <title> | --title <title> [--area AREA] [--priority P0|P1|P2|P3] [--summary TEXT] [--acceptance TEXT] [--depends-on ID] [--capability ID] [--external-prerequisite TEXT] [--status STATUS] [--revision N] [--json]"],
  ["/docs/cli/roadmap/drop", "jeden roadmap drop <id> [reason|--reason TEXT] [--revision N] [--json]"],
  ["/docs/cli/roadmap/start", "jeden roadmap start <id> [reason|--reason TEXT] [--revision N] [--json]"],
  ["/docs/cli/roadmap/implemented", "jeden roadmap implemented <id> [reason|--reason TEXT] [--revision N] [--json]"],
  ["/docs/cli/roadmap/pass", "jeden roadmap pass <id> [reason|--reason TEXT] [--evidence URI] [--revision N] [--json]"],
  ["/docs/cli/roadmap/block", "jeden roadmap block <id> <reason> [--external-prerequisite TEXT] [--evidence URI] [--revision N] [--json]"],
  ["/docs/cli/roadmap/status", "jeden roadmap status <id> <status> [reason|--reason TEXT] [--external-prerequisite TEXT] [--evidence URI] [--revision N] [--json]"],
  ["/docs/cli/roadmap/depends", "jeden roadmap depends <id> <dependency-id> [--revision N] [--json]"],
  ["/docs/cli/roadmap/undepends", "jeden roadmap undepends <id> <dependency-id> [--revision N] [--json]"],
  ["/docs/cli/roadmap/graph", "jeden roadmap graph [--json]"],
  ["/docs/cli/roadmap/acceptance", "jeden roadmap acceptance <list|add|evidence> <item-id> ... [--revision N] [--json]"],
  ["/docs/cli/roadmap/acceptance/list", "jeden roadmap acceptance list <item-id> [--json]"],
  ["/docs/cli/roadmap/acceptance/add", "jeden roadmap acceptance add <item-id> <criterion> [--id ID] [--revision N] [--json]"],
  ["/docs/cli/roadmap/acceptance/evidence", "jeden roadmap acceptance evidence <item-id> <acceptance-id> <artifact-uri> [--revision N] [--json]"],
  ["/docs/cli/roadmap/check", "jeden roadmap check [--json]"],
  ["/docs/cli/roadmap/work", "jeden roadmap work <item-id> [--json]"],
].map(([path, invocation]) => ({ path, invocation }));

function decodeVisibleText(html) {
  return html
    .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, " ")
    .replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/&quot;/g, '"')
    .replace(/&#x27;|&#39;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ")
    .trim();
}

function canonicalHref(html) {
  return html.match(/<link\s+rel="canonical"\s+href="([^"]+)"\s*\/?>/i)?.[1] ?? null;
}

test("production publishes the exact Jeden CLI route contract", { timeout: 120_000 }, async () => {
  const seen = new Set();
  for (const entry of EXPECTED_CLI_ROUTES) {
    assert(!seen.has(entry.path), `duplicate expected route: ${entry.path}`);
    seen.add(entry.path);

    const canonical = new URL(entry.path, PRODUCTION_ORIGIN).href;
    const response = await fetch(canonical, { redirect: "follow" });
    assert.equal(response.status, 200, `${entry.path} returned ${response.status}`);
    assert.equal(response.url, canonical, `${entry.path} redirected away from its canonical URL`);

    const html = await response.text();
    assert.equal(canonicalHref(html), canonical, `${entry.path} has the wrong canonical link`);
    assert(
      decodeVisibleText(html).includes(entry.invocation),
      `${entry.path} does not contain its exact invocation: ${entry.invocation}`,
    );
  }
});

test("the production CLI index links exactly the expected command tree", { timeout: 30_000 }, async () => {
  const indexUrl = `${PRODUCTION_ORIGIN}/docs/cli`;
  const response = await fetch(indexUrl, { redirect: "follow" });
  assert.equal(response.status, 200, `/docs/cli returned ${response.status}`);
  assert.equal(response.url, indexUrl, "/docs/cli redirected away from its canonical URL");

  const html = await response.text();
  assert.equal(canonicalHref(html), indexUrl, "/docs/cli has the wrong canonical link");
  const linkedRoutes = [...new Set(
    [...html.matchAll(/href="(\/docs\/cli\/[^"?#]+)"/g)].map((match) => match[1]),
  )].sort();
  const expectedRoutes = EXPECTED_CLI_ROUTES.map(({ path }) => path).sort();
  assert.deepEqual(linkedRoutes, expectedRoutes);
});
