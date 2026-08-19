#!/usr/bin/env node
/**
 * Renders the Jeden documentation pages from web/docs/pages.mjs through the
 * canonical DocumentationLayout in @wisent-ai/components, wraps the result in
 * the jeden.wisent.com site chrome, and writes the static web/docs/*.html
 * files plus web/wisent-components.css (the package stylesheet).
 *
 * Run from anywhere: `npm run build:docs` (repo root) or
 * `node web/scripts/build-docs.mjs`.
 */
import { copyFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createElement as h } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { DocumentationLayout } from "@wisent-ai/components";
import { homeHref, nav, pages, product, sourceHref, sourceLabel } from "../docs/pages.mjs";

const webRoot = fileURLToPath(new URL("..", import.meta.url));
const docsDir = path.join(webRoot, "docs");

/* ------------------------------------------------------------------------
 * Limited inline markup -> React nodes.
 *
 * DocPage prose is plain text to the component, so the data module carries
 * the pages' inline semantics as literal <code>, <strong>, <em>, and
 * <a href="…"> tags. This parses exactly those four tags (one level of
 * nesting, e.g. <strong><code>…</code></strong>) into elements; everything
 * else stays text and is escaped by React.
 * ---------------------------------------------------------------------- */
const INLINE = /<(code|strong|em)>(.*?)<\/\1>|<a href="([^"]*)">(.*?)<\/a>/;

function rich(text) {
  // Fresh stateful regex per call: rich() recurses, and a shared global
  // regex's lastIndex would be clobbered by the inner call.
  const inline = new RegExp(INLINE.source, "g");
  const nodes = [];
  let last = 0;
  let key = 0;
  for (let m; (m = inline.exec(text)); ) {
    if (m.index > last) nodes.push(text.slice(last, m.index));
    if (m[1]) nodes.push(h(m[1], { key: `i${key++}` }, rich(m[2])));
    else nodes.push(h("a", { key: `i${key++}`, href: m[3] }, rich(m[4])));
    last = m.index + m[0].length;
  }
  if (nodes.length === 0) return text;
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

function richSection(section) {
  return {
    ...section,
    paragraphs: section.paragraphs?.map(rich),
    bullets: section.bullets?.map(rich),
    steps: section.steps?.map(rich),
    callout: section.callout ? { ...section.callout, text: rich(section.callout.text) } : undefined,
  };
}

/* ------------------------------------------------------------------------
 * jeden.wisent.com site chrome. The component owns only the docs body
 * (sidebar + reading column); header and footer stay with the site.
 * ---------------------------------------------------------------------- */
const BRAND_PATH =
  "M32 13.665v3.229c-.1.506-.18 1.017-.302 1.519-1.372 5.667-4.884 9.514-10.539 11.472-1.118.387-2.326.527-3.492.781h-3.112c-.142-.032-.283-.074-.427-.094C7.612 29.664 3.193 26.207.903 20.253.44 19.048.293 17.73 0 16.463v-2.367c.103-.595.178-1.198.316-1.785C1.739 6.214 5.601 2.319 11.744.518 12.584.271 13.469.169 14.333 0h3.334c.124.031.246.075.373.09 6.709.808 12.213 5.667 13.653 12.057.114.504.206 1.012.307 1.518ZM3.707 8.538c-.076.091-.131.141-.167.202-.844 1.452-1.366 3.01-1.592 4.658-.02.146.15.388.3.475.965.562 1.932 1.127 2.936 1.621.467.229.593.496.596.982.036 5.005 2.274 8.863 6.521 11.672.372.246.702.289 1.129.137 3.093-1.104 5.506-3.013 7.314-5.674.635-.933.517-.774 1.508-.556 1.64.36 3.279.723 4.912 1.105.323.075.527.025.674-.269.585-1.171 1.186-2.336 1.759-3.512.12-.246.151-.535.233-.835-9.237-2.129-17.956-5.347-26.123-10.006Z";
const BRAND_PATH_FULL = `${BRAND_PATH}m26.412 8.26c.789-6.027-3.017-12.398-9.859-14.473-6.04-1.831-12.377.443-15.526 4.749 7.928 4.519 16.408 7.663 25.385 9.724Z`;

const SITE_HEADER = `    <header class="site-header" data-header>
      <a class="brand" href="/" aria-label="Jeden home">
        <svg class="brand-mark" viewBox="0 0 32 31" aria-hidden="true">
          <path d="${BRAND_PATH_FULL}" />
        </svg>
        <span>JEDEN</span>
      </a>
      <nav class="nav" aria-label="Primary navigation" data-nav>
        <a href="/#principles">Principles</a>
        <a href="/#capabilities">Capabilities</a>
        <a href="/#security">Security</a>
        <a href="/docs">Docs</a>
      </nav>
      <a class="header-cta" href="mailto:contact@wisent.ai?subject=Jeden%20private%20preview">Request access <span aria-hidden="true">↗</span></a>
      <button class="menu-button" type="button" aria-label="Open menu" aria-expanded="false" data-menu>
        <span></span><span></span>
      </button>
    </header>`;

const SITE_FOOTER = `    <footer>
      <a class="brand footer-brand" href="/">
        <svg class="brand-mark" viewBox="0 0 32 31" aria-hidden="true"><path d="${BRAND_PATH}"/></svg>
        <span>JEDEN</span>
      </a>
      <div class="footer-meta"><span>A WISENT SYSTEM</span><span>© <span data-year></span> WISENT AI, INC.</span></div>
      <div class="footer-links"><a href="https://www.wisent.ai" target="_blank" rel="noreferrer">Wisent ↗</a><a href="mailto:contact@wisent.ai">Contact ↗</a></div>
    </footer>`;

function shell(page, body) {
  const { meta } = page;
  return `<!doctype html>
<!-- Generated by web/scripts/build-docs.mjs from web/docs/pages.mjs — do not edit by hand. -->
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="theme-color" content="#f2f1ed" />
    <meta name="description" content="${meta.description}" />
    <meta property="og:type" content="website" />
    <meta property="og:url" content="${meta.canonical}" />
    <meta property="og:title" content="${meta.ogTitle}" />
    <meta property="og:description" content="${meta.ogDescription}" />
    <meta property="og:image" content="https://jeden.wisent.com/og-image.png" />
    <meta name="twitter:card" content="summary_large_image" />
    <link rel="canonical" href="${meta.canonical}" />
    <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
    <link rel="preconnect" href="https://fonts.googleapis.com" />
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
    <link href="https://fonts.googleapis.com/css2?family=DM+Mono:wght@300;400;500&family=Manrope:wght@400;500;600;700&display=swap" rel="stylesheet" />
    <link rel="stylesheet" href="/styles.css" />
    <link rel="stylesheet" href="/wisent-components.css" />
    <script src="/script.js" defer></script>
    <title>${meta.htmlTitle}</title>
  </head>
  <body>
    <div class="grain" aria-hidden="true"></div>
${SITE_HEADER}

    <div class="docs-page">
      ${body}
    </div>

${SITE_FOOTER}
  </body>
</html>
`;
}

/* ------------------------------------------------------------------------ */
const componentStyles = fileURLToPath(await import.meta.resolve("@wisent-ai/components/styles.css"));
await copyFile(componentStyles, path.join(webRoot, "wisent-components.css"));

for (const [i, page] of pages.entries()) {
  const body = renderToStaticMarkup(
    h(DocumentationLayout, {
      product,
      homeHref,
      sourceHref,
      sourceLabel,
      nav,
      currentHref: page.href,
      previous: i > 0 ? { label: nav[i - 1].label, href: nav[i - 1].href } : undefined,
      next: i < pages.length - 1 ? { label: nav[i + 1].label, href: nav[i + 1].href } : undefined,
      page: {
        slug: page.slug,
        eyebrow: page.eyebrow,
        title: rich(page.title),
        description: rich(page.description),
        sections: page.sections.map(richSection),
      },
    }),
  );
  const outFile = path.join(docsDir, page.file);
  await writeFile(outFile, shell(page, body));
  console.log(`wrote docs/${page.file}`);
}
console.log("wrote wisent-components.css");
