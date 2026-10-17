import { HttpError, sha256Hex } from "./http";

/** A new API token: `brim_` and 32 random bytes in unpadded base64url. */
export function newToken(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  let binary = "";
  for (const b of bytes) binary += String.fromCharCode(b);
  const b64 = btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  return `brim_${b64}`;
}

export function bearer(request: Request): string | null {
  const header = request.headers.get("authorization");
  const match = header ? /^Bearer\s+(\S+)\s*$/i.exec(header) : null;
  return match ? match[1]! : null;
}

const unauthorized = (message: string) => new HttpError(401, message, { "www-authenticate": "Bearer" });

/** The user the request's token belongs to; 401 otherwise. */
export async function requireUser(request: Request, env: Env): Promise<string> {
  const token = bearer(request);
  if (!token) throw unauthorized("This needs an API token: send it as `Authorization: Bearer <token>`.");
  const row = await env.DB.prepare("SELECT user FROM tokens WHERE hash = ?")
    .bind(await sha256Hex(token))
    .first<{ user: string }>();
  if (!row) throw unauthorized("The API token is not valid.");
  return row.user;
}

/** Checks the request carries the service's ADMIN_TOKEN; 401 otherwise. */
export async function requireAdmin(request: Request, env: Env): Promise<void> {
  const token = bearer(request);
  if (!env.ADMIN_TOKEN) throw new HttpError(503, "Token creation is not set up on this registry.");
  if (!token) throw unauthorized("This needs the admin token.");
  // Compare digests so the comparison takes the same time whatever the input.
  const enc = new TextEncoder();
  const [a, b] = await Promise.all([
    crypto.subtle.digest("SHA-256", enc.encode(token)),
    crypto.subtle.digest("SHA-256", enc.encode(env.ADMIN_TOKEN)),
  ]);
  if (!crypto.subtle.timingSafeEqual(a, b)) throw unauthorized("The admin token is not valid.");
}
