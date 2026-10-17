// The registry's two HTML pages: the package list with search, and a package page.

import { getPackage, getVersions, newestVersion, searchPackages, type SearchHit } from "./api";
import { escapeHtml as e } from "./http";
import { parseVersion } from "./semver";

export const STYLE = `
:root {
  --bg: #ffffff; --fg: #1d1f23; --muted: #5f6670; --line: #e3e5e8; --card: #f7f8f9;
  --accent: #2459c9; --warn: #a3330f; color-scheme: light dark;
}
@media (prefers-color-scheme: dark) {
  :root { --bg: #121417; --fg: #e6e8eb; --muted: #9aa1ab; --line: #2a2e34; --card: #1a1d21;
    --accent: #7aa5ff; --warn: #ff9a76; }
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg);
  font: 16px/1.55 system-ui, -apple-system, "Segoe UI", sans-serif; }
main { max-width: 760px; margin: 0 auto; padding: 32px 16px 64px; }
header.site { display: flex; align-items: baseline; gap: 12px; margin-bottom: 24px; }
header.site a { color: var(--fg); text-decoration: none; font-weight: 700; font-size: 20px; }
header.site span { color: var(--muted); }
a { color: var(--accent); }
h1 { font-size: 28px; margin: 0 0 4px; overflow-wrap: anywhere; }
h2 { font-size: 18px; margin: 32px 0 8px; }
p.lead { color: var(--muted); margin: 0 0 16px; }
input[type=search] { width: 100%; padding: 10px 12px; font: inherit; color: var(--fg);
  background: var(--card); border: 1px solid var(--line); border-radius: 8px; }
ul.packages { list-style: none; padding: 0; margin: 16px 0 0; }
ul.packages li { padding: 12px 0; border-bottom: 1px solid var(--line); }
ul.packages a { font-weight: 600; text-decoration: none; }
.version { color: var(--muted); font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 14px; margin-left: 8px; }
.desc { color: var(--muted); margin-top: 2px; overflow-wrap: anywhere; }
pre { background: var(--card); border: 1px solid var(--line); border-radius: 8px; padding: 12px 14px;
  overflow-x: auto; font: 14px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; }
table { width: 100%; border-collapse: collapse; font-size: 15px; }
th, td { text-align: left; padding: 8px 8px 8px 0; border-bottom: 1px solid var(--line); vertical-align: top; }
th { color: var(--muted); font-weight: 500; }
td code { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 14px; }
.yanked { color: var(--warn); font-size: 13px; margin-left: 6px; }
dl { display: grid; grid-template-columns: max-content 1fr; gap: 4px 16px; margin: 16px 0 0; }
dt { color: var(--muted); }
dd { margin: 0; overflow-wrap: anywhere; }
.empty { color: var(--muted); padding: 16px 0; }
`;

function page(title: string, body: string): Response {
  const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${e(title)}</title>
<link rel="stylesheet" href="/static/site.css">
</head>
<body>
<main>
<header class="site"><a href="/">Brim packages</a><span>the Brim package registry</span></header>
${body}
</main>
</body>
</html>`;
  return new Response(html, {
    headers: {
      "content-type": "text/html; charset=utf-8",
      "cache-control": "public, max-age=60",
      "content-security-policy": "default-src 'none'; style-src 'self'; script-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'",
      "x-content-type-options": "nosniff",
      "referrer-policy": "same-origin",
    },
  });
}

function hitItem(hit: SearchHit): string {
  const version = hit.version ? `<span class="version">${e(hit.version)}</span>` : "";
  const desc = hit.description ? `<div class="desc">${e(hit.description)}</div>` : "";
  return `<li><a href="/packages/${encodeURIComponent(hit.name)}">${e(hit.name)}</a>${version}${desc}</li>`;
}

// The search box re-renders the list from /api/v1/search as the user types.
export const SEARCH_SCRIPT = `
"use strict";
const input = document.getElementById("q");
const list = document.getElementById("packages");
const esc = (s) => s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
let timer, seq = 0;
function render(packages) {
  if (packages.length === 0) { list.innerHTML = '<li class="empty">No packages found.</li>'; return; }
  list.innerHTML = packages.map((p) =>
    '<li><a href="/packages/' + encodeURIComponent(p.name) + '">' + esc(p.name) + '</a>' +
    (p.version ? '<span class="version">' + esc(p.version) + '</span>' : '') +
    (p.description ? '<div class="desc">' + esc(p.description) + '</div>' : '') + '</li>').join("");
}
async function run() {
  const q = input.value.trim();
  const mine = ++seq;
  const url = new URL(location.href);
  if (q) url.searchParams.set("q", q); else url.searchParams.delete("q");
  history.replaceState(null, "", url);
  try {
    const res = await fetch("/api/v1/search?q=" + encodeURIComponent(q));
    const data = await res.json();
    if (mine === seq) render(data.packages || []);
  } catch {
    if (mine === seq) list.innerHTML = '<li class="empty">Search is not available right now.</li>';
  }
}
input.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(run, 150); });
`;

// GET /
export async function homePage(env: Env, url: URL): Promise<Response> {
  const q = (url.searchParams.get("q") ?? "").trim();
  const hits = await searchPackages(env, q);
  const items = hits.length ? hits.map(hitItem).join("") : `<li class="empty">No packages found.</li>`;
  const body = `
<h1>Packages</h1>
<p class="lead">Libraries for the Brim programming language. Add one to the <code>[dependencies]</code> of your <code>.brimproj</code>.</p>
<form action="/" method="get" role="search">
  <input id="q" type="search" name="q" value="${e(q)}" placeholder="Search packages" aria-label="Search packages" autocomplete="off">
</form>
<ul id="packages" class="packages">${items}</ul>
<script src="/static/search.js" defer></script>`;
  return page("Brim packages", body);
}

/** The requirement to suggest for a version: `MAJOR.MINOR`, or the whole pre-release. */
function suggestedRequirement(version: string): string {
  const v = parseVersion(version);
  if (!v) return version;
  return v.pre.length ? `=${version}` : `${v.major}.${v.minor}`;
}

function safeUrl(text: string): string | null {
  try {
    const url = new URL(text);
    return url.protocol === "https:" || url.protocol === "http:" ? url.href : null;
  } catch {
    return null;
  }
}

// GET /packages/{name}
export async function packagePage(env: Env, name: string): Promise<Response | null> {
  const pkg = await getPackage(env, name);
  if (!pkg) return null;
  const versions = await getVersions(env, name);
  const newest = newestVersion(versions);

  const depend = newest
    ? `<h2>Use it</h2>
<p>Add this to the <code>[dependencies]</code> of your <code>.brimproj</code>:</p>
<pre>[dependencies]
${e(pkg.name)} = "${e(suggestedRequirement(newest))}"</pre>`
    : `<h2>Use it</h2><p>Every version of this package is yanked, so new projects cannot depend on it.</p>`;

  const rows = [...versions]
    .reverse()
    .map((v) => {
      const deps = Object.entries(JSON.parse(v.dependencies) as Record<string, string>)
        .map(([d, r]) => `<a href="/packages/${encodeURIComponent(d)}">${e(d)}</a> <code>${e(r)}</code>`)
        .join(", ");
      const yanked = v.yanked ? `<span class="yanked">yanked</span>` : "";
      return `<tr><td><code>${e(v.version)}</code>${yanked}</td><td>${e(v.published_at.slice(0, 10))}</td><td>${deps || "none"}</td></tr>`;
    })
    .join("");

  const repo = pkg.repository ? safeUrl(pkg.repository) : null;
  const facts = [
    ["Owner", e(pkg.owner)],
    pkg.license ? ["License", e(pkg.license)] : null,
    repo ? ["Repository", `<a href="${e(repo)}" rel="nofollow noopener">${e(pkg.repository!)}</a>`] : null,
  ]
    .filter((f): f is string[] => f !== null)
    .map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`)
    .join("");

  const body = `
<h1>${e(pkg.name)}${newest ? `<span class="version">${e(newest)}</span>` : ""}</h1>
${pkg.description ? `<p class="lead">${e(pkg.description)}</p>` : ""}
<dl>${facts}</dl>
${depend}
<h2>Versions</h2>
<table>
<thead><tr><th>Version</th><th>Published</th><th>Dependencies</th></tr></thead>
<tbody>${rows}</tbody>
</table>`;
  return page(`${pkg.name} - Brim packages`, body);
}

export function notFoundPage(): Response {
  const res = page("Not found - Brim packages", `<h1>Not found</h1><p class="lead">There is nothing at this address. <a href="/">See all packages</a>.</p>`);
  return new Response(res.body, { status: 404, headers: res.headers });
}

// GET /static/site.css and /static/search.js
export function staticFile(path: string): Response | null {
  const files: Record<string, [string, string]> = {
    "/static/site.css": [STYLE, "text/css; charset=utf-8"],
    "/static/search.js": [SEARCH_SCRIPT, "text/javascript; charset=utf-8"],
  };
  const file = files[path];
  if (!file) return null;
  return new Response(file[0], {
    headers: { "content-type": file[1], "cache-control": "public, max-age=3600", "x-content-type-options": "nosniff" },
  });
}
