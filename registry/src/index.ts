// The Brim package registry. The protocol is described in docs/registry.md.

import { createToken, download, index, me, publish, revokeOwnToken, revokeUserTokens, search, setYanked } from "./api";
import { HttpError, errorResponse } from "./http";
import { homePage, notFoundPage, packagePage, staticFile } from "./pages";

type Handler = (request: Request, env: Env, url: URL, params: string[]) => Promise<Response>;

interface Route {
  method: string;
  pattern: RegExp;
  handler: Handler;
}

const SEGMENT = "([^/]+)";

const routes: Route[] = [
  { method: "GET", pattern: new RegExp(`^/api/v1/index/${SEGMENT}$`), handler: (_r, env, _u, [name]) => index(env, name!) },
  {
    method: "GET",
    pattern: new RegExp(`^/api/v1/download/${SEGMENT}/${SEGMENT}$`),
    handler: (_r, env, _u, [name, version]) => download(env, name!, version!),
  },
  { method: "PUT", pattern: /^\/api\/v1\/publish$/, handler: (r, env) => publish(r, env) },
  {
    method: "DELETE",
    pattern: new RegExp(`^/api/v1/packages/${SEGMENT}/${SEGMENT}/yank$`),
    handler: (r, env, _u, [name, version]) => setYanked(r, env, name!, version!, true),
  },
  {
    method: "PUT",
    pattern: new RegExp(`^/api/v1/packages/${SEGMENT}/${SEGMENT}/unyank$`),
    handler: (r, env, _u, [name, version]) => setYanked(r, env, name!, version!, false),
  },
  { method: "GET", pattern: /^\/api\/v1\/me$/, handler: (r, env) => me(r, env) },
  { method: "GET", pattern: /^\/api\/v1\/search$/, handler: (_r, env, url) => search(env, url) },
  { method: "POST", pattern: /^\/api\/v1\/admin\/tokens$/, handler: (r, env) => createToken(r, env) },
  { method: "DELETE", pattern: /^\/api\/v1\/admin\/tokens$/, handler: (r, env) => revokeUserTokens(r, env) },
  { method: "DELETE", pattern: /^\/api\/v1\/me\/token$/, handler: (r, env) => revokeOwnToken(r, env) },
  { method: "GET", pattern: /^\/$/, handler: (_r, env, url) => homePage(env, url) },
  {
    method: "GET",
    pattern: new RegExp(`^/packages/${SEGMENT}$`),
    handler: async (_r, env, _u, [name]) => (await packagePage(env, name!)) ?? notFoundPage(),
  },
];

async function route(request: Request, env: Env): Promise<Response> {
  const url = new URL(request.url);
  const method = request.method === "HEAD" ? "GET" : request.method;
  const isApi = url.pathname.startsWith("/api/");

  if (method === "GET") {
    const file = staticFile(url.pathname);
    if (file) return file;
  }

  const allowed: string[] = [];
  for (const r of routes) {
    const match = r.pattern.exec(url.pathname);
    if (!match) continue;
    if (r.method !== method) {
      allowed.push(r.method);
      continue;
    }
    let params: string[];
    try {
      params = match.slice(1).map((p) => decodeURIComponent(p));
    } catch {
      throw new HttpError(400, "The address is not valid.");
    }
    return r.handler(request, env, url, params);
  }
  if (allowed.length) {
    throw new HttpError(405, `Use ${allowed.join(" or ")} for this address.`, { allow: allowed.join(", ") });
  }
  if (isApi) throw new HttpError(404, "There is no such API endpoint.");
  return notFoundPage();
}

export default {
  async fetch(request, env): Promise<Response> {
    try {
      return await route(request, env);
    } catch (e) {
      if (e instanceof HttpError) return errorResponse(e.status, e.message, e.headers);
      console.error(
        JSON.stringify({
          event: "unhandled_error",
          method: request.method,
          path: new URL(request.url).pathname,
          error: e instanceof Error ? (e.stack ?? e.message) : String(e),
        }),
      );
      return errorResponse(500, "The registry hit an internal error. Try again later.");
    }
  },
} satisfies ExportedHandler<Env>;
