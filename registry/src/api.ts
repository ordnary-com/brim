import { ArchiveError, checkArchive } from "./archive";
import { newToken, requireAdmin, requireUser } from "./auth";
import { HttpError, json, now, ok, readBody, sha256Hex } from "./http";
import { compareVersionStrings, parseRequirement, parseVersion } from "./semver";

export const MAX_ARCHIVE_BYTES = 10 * 1024 * 1024;
export const MAX_METADATA_BYTES = 1024 * 1024;
const MAX_BODY_BYTES = 4 + MAX_METADATA_BYTES + 4 + MAX_ARCHIVE_BYTES;
export const EDITIONS = ["2026"];

const NAME_RULES =
  "package names start with a lowercase ASCII letter and contain only lowercase letters, digits and `_` (at most 64 characters)";

/** `brim_manifest::valid_import_name`: a valid name without `-`. */
export function validName(name: string): boolean {
  return /^[a-z][a-z0-9_]{0,63}$/.test(name);
}

export const normalizeName = (name: string) => name.replaceAll("_", "");

export const archiveKey = (name: string, version: string) => `${name}/${name}-${version}.tar.gz`;

export interface VersionRow {
  package: string;
  version: string;
  checksum: string;
  edition: string;
  dependencies: string;
  yanked: number;
  published_at: string;
  size: number;
}

export interface PackageRow {
  name: string;
  normalized: string;
  owner: string;
  description: string | null;
  license: string | null;
  repository: string | null;
  created_at: string;
  updated_at: string;
}

export function sortVersions<T extends { version: string }>(rows: T[]): T[] {
  return [...rows].sort((a, b) => compareVersionStrings(a.version, b.version));
}

/**
 * The newest non-yanked version: the newest release, or the newest pre-release
 * when there is no release. Null when every version is yanked.
 */
export function newestVersion(rows: { version: string; yanked: number }[]): string | null {
  const live = sortVersions(rows.filter((r) => !r.yanked));
  const releases = live.filter((r) => !r.version.includes("-"));
  const pick = releases.at(-1) ?? live.at(-1);
  return pick ? pick.version : null;
}

export async function getPackage(env: Env, name: string): Promise<PackageRow | null> {
  return env.DB.prepare("SELECT * FROM packages WHERE name = ?").bind(name).first<PackageRow>();
}

export async function getVersions(env: Env, name: string): Promise<VersionRow[]> {
  const { results } = await env.DB.prepare("SELECT * FROM versions WHERE package = ?").bind(name).all<VersionRow>();
  return sortVersions(results);
}

export function indexEntry(row: VersionRow) {
  return {
    version: row.version,
    checksum: row.checksum,
    edition: row.edition,
    dependencies: JSON.parse(row.dependencies) as Record<string, string>,
    yanked: row.yanked !== 0,
    published_at: row.published_at,
  };
}

// GET /api/v1/index/{name}
export async function index(env: Env, name: string): Promise<Response> {
  const pkg = await getPackage(env, name);
  if (!pkg) throw new HttpError(404, `There is no package \`${name}\` on this registry.`);
  const versions = await getVersions(env, name);
  return json({ name: pkg.name, versions: versions.map(indexEntry) }, 200, {
    "cache-control": "public, max-age=60",
  });
}

// GET /api/v1/download/{name}/{version}
export async function download(env: Env, name: string, version: string): Promise<Response> {
  const notFound = new HttpError(404, `There is no version ${version} of \`${name}\` on this registry.`);
  if (!validName(name) || !parseVersion(version)) throw notFound;
  const object = await env.ARCHIVES.get(archiveKey(name, version));
  if (!object) throw notFound;
  const headers = new Headers();
  headers.set("content-type", "application/gzip");
  headers.set("content-length", String(object.size));
  headers.set("cache-control", "public, max-age=31536000, immutable");
  headers.set("etag", object.httpEtag);
  headers.set("content-disposition", `attachment; filename="${name}-${version}.tar.gz"`);
  return new Response(object.body, { headers });
}

interface Metadata {
  name: string;
  version: string;
  edition: string;
  checksum: string;
  dependencies: Record<string, string>;
  description: string | null;
  license: string | null;
  repository: string | null;
}

function optionalString(meta: Record<string, unknown>, key: string): string | null {
  const value = meta[key];
  if (value === undefined || value === null) return null;
  if (typeof value !== "string") throw new HttpError(400, `The metadata's \`${key}\` must be a string or null.`);
  return value;
}

function parseMetadata(bytes: Uint8Array): Metadata {
  let meta: unknown;
  try {
    meta = JSON.parse(new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(bytes));
  } catch {
    throw new HttpError(400, "The publish metadata is not valid UTF-8 JSON.");
  }
  if (typeof meta !== "object" || meta === null || Array.isArray(meta)) {
    throw new HttpError(400, "The publish metadata must be a JSON object.");
  }
  const m = meta as Record<string, unknown>;
  for (const key of ["name", "version", "edition", "checksum"]) {
    if (typeof m[key] !== "string") throw new HttpError(400, `The metadata needs \`${key}\` as a string.`);
  }
  const deps = m.dependencies ?? {};
  if (typeof deps !== "object" || deps === null || Array.isArray(deps)) {
    throw new HttpError(400, "The metadata's `dependencies` must be an object of names to version requirements.");
  }
  for (const [dep, req] of Object.entries(deps)) {
    if (typeof req !== "string") {
      throw new HttpError(400, `The requirement for dependency \`${dep}\` must be a string.`);
    }
  }
  return {
    name: m.name as string,
    version: m.version as string,
    edition: m.edition as string,
    checksum: m.checksum as string,
    dependencies: deps as Record<string, string>,
    description: optionalString(m, "description"),
    license: optionalString(m, "license"),
    repository: optionalString(m, "repository"),
  };
}

/** Splits the publish body into metadata and archive. */
function unframe(body: Uint8Array): { metadata: Uint8Array; archive: Uint8Array } {
  const bad = (why: string) => new HttpError(400, `The publish request is malformed: ${why}.`);
  const view = new DataView(body.buffer, body.byteOffset, body.byteLength);
  if (body.byteLength < 4) throw bad("it is too short for the metadata length");
  const metaLen = view.getUint32(0, true);
  if (metaLen > MAX_METADATA_BYTES) throw new HttpError(413, "The publish metadata is larger than 1 MiB.");
  if (body.byteLength < 4 + metaLen + 4) throw bad("it ends inside the metadata or the archive length");
  const metadata = body.subarray(4, 4 + metaLen);
  const archiveLen = view.getUint32(4 + metaLen, true);
  if (archiveLen > MAX_ARCHIVE_BYTES) throw new HttpError(413, "The archive is larger than 10 MiB.");
  const start = 8 + metaLen;
  if (body.byteLength < start + archiveLen) throw bad("it ends inside the archive");
  if (body.byteLength > start + archiveLen) throw bad("there are bytes after the archive");
  return { metadata, archive: body.subarray(start, start + archiveLen) };
}

// PUT /api/v1/publish
export async function publish(request: Request, env: Env): Promise<Response> {
  const user = await requireUser(request, env);
  const body = await readBody(request, MAX_BODY_BYTES, "The archive is larger than 10 MiB.");
  const framed = unframe(body);
  const meta = parseMetadata(framed.metadata);
  const { name, version, edition } = meta;

  if (!validName(name)) throw new HttpError(400, `\`${name}\` is not a valid registry package name: ${NAME_RULES}.`);
  if (version.includes("+")) throw new HttpError(400, `Version ${version} has build metadata (\`+...\`), which the registry does not allow.`);
  if (!parseVersion(version)) {
    throw new HttpError(400, `\`${version}\` is not a valid version; use MAJOR.MINOR.PATCH with an optional -pre.release part.`);
  }
  if (!EDITIONS.includes(edition)) {
    throw new HttpError(400, `Edition \`${edition}\` is not supported; use ${EDITIONS.map((e) => `\`${e}\``).join(" or ")}.`);
  }

  if (!/^[0-9a-f]{64}$/.test(meta.checksum)) {
    throw new HttpError(400, "The metadata's `checksum` must be a SHA-256 in lowercase hex.");
  }
  const actual = await sha256Hex(framed.archive);
  if (actual !== meta.checksum) {
    throw new HttpError(400, `The archive's SHA-256 is ${actual}, not the ${meta.checksum} in the metadata.`);
  }

  try {
    await checkArchive(framed.archive, name, version);
  } catch (e) {
    if (e instanceof ArchiveError) throw new HttpError(400, e.message);
    throw e;
  }

  const depNames = Object.keys(meta.dependencies);
  for (const dep of depNames) {
    const req = meta.dependencies[dep]!;
    if (!parseRequirement(req)) {
      throw new HttpError(400, `The requirement \`${req}\` for dependency \`${dep}\` is not well-formed.`);
    }
  }
  if (depNames.length) {
    const { results } = await env.DB.prepare("SELECT name FROM packages WHERE name IN (SELECT value FROM json_each(?))")
      .bind(JSON.stringify(depNames))
      .all<{ name: string }>();
    const known = new Set(results.map((r) => r.name));
    const missing = depNames.filter((d) => !known.has(d));
    if (missing.length) {
      const list = missing.map((d) => `\`${d}\``).join(", ");
      throw new HttpError(400, `${missing.length === 1 ? "Dependency" : "Dependencies"} ${list} ${missing.length === 1 ? "is" : "are"} not on the registry.`);
    }
  }

  const existing = await getPackage(env, name);
  const time = now();
  const statements: D1PreparedStatement[] = [];
  if (existing) {
    if (existing.owner !== user) {
      throw new HttpError(403, `The package \`${name}\` belongs to another user.`);
    }
    const versions = await getVersions(env, name);
    if (versions.some((v) => v.version === version)) {
      throw new HttpError(409, `Version ${version} of \`${name}\` already exists; published versions never change.`);
    }
    // Keep the package's description from its newest version.
    const newest = versions.every((v) => compareVersionStrings(v.version, version) < 0);
    statements.push(
      newest
        ? env.DB.prepare(
            "UPDATE packages SET description = ?, license = ?, repository = ?, updated_at = ? WHERE name = ?",
          ).bind(meta.description, meta.license, meta.repository, time, name)
        : env.DB.prepare("UPDATE packages SET updated_at = ? WHERE name = ?").bind(time, name),
    );
  } else {
    const clash = await env.DB.prepare("SELECT name FROM packages WHERE normalized = ?")
      .bind(normalizeName(name))
      .first<{ name: string }>();
    if (clash) {
      throw new HttpError(400, `The name \`${name}\` is too close to the existing package \`${clash.name}\`; names may not differ only in \`_\`.`);
    }
    statements.push(
      env.DB.prepare(
        "INSERT INTO packages (name, normalized, owner, description, license, repository, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
      ).bind(name, normalizeName(name), user, meta.description, meta.license, meta.repository, time, time),
    );
  }
  statements.push(
    env.DB.prepare(
      "INSERT INTO versions (package, version, checksum, edition, dependencies, yanked, published_at, size) VALUES (?, ?, ?, ?, ?, 0, ?, ?)",
    ).bind(name, version, meta.checksum, edition, JSON.stringify(meta.dependencies), time, framed.archive.byteLength),
  );

  // The index row goes in first: its primary key makes a concurrent publish of
  // the same version fail here, so only one request ever writes the archive.
  try {
    await env.DB.batch(statements);
  } catch (e) {
    if (String(e).includes("UNIQUE constraint failed")) {
      throw new HttpError(409, `Version ${version} of \`${name}\` (or a package with a too similar name) was published at the same time.`);
    }
    throw e;
  }

  try {
    await env.ARCHIVES.put(archiveKey(name, version), framed.archive, {
      httpMetadata: { contentType: "application/gzip" },
      sha256: meta.checksum,
    });
  } catch (e) {
    // Undo the index rows so the version can be published again.
    const undo = [env.DB.prepare("DELETE FROM versions WHERE package = ? AND version = ?").bind(name, version)];
    if (!existing) undo.push(env.DB.prepare("DELETE FROM packages WHERE name = ?").bind(name));
    await env.DB.batch(undo);
    throw e;
  }

  console.log(JSON.stringify({ event: "publish", package: name, version, user, size: framed.archive.byteLength }));
  return ok();
}

// DELETE /api/v1/packages/{name}/{version}/yank and PUT .../unyank
export async function setYanked(request: Request, env: Env, name: string, version: string, yanked: boolean): Promise<Response> {
  const user = await requireUser(request, env);
  const pkg = await getPackage(env, name);
  if (!pkg) throw new HttpError(404, `There is no package \`${name}\` on this registry.`);
  if (pkg.owner !== user) throw new HttpError(403, `The package \`${name}\` belongs to another user.`);
  const result = await env.DB.prepare("UPDATE versions SET yanked = ? WHERE package = ? AND version = ?")
    .bind(yanked ? 1 : 0, name, version)
    .run();
  if (result.meta.changes === 0) throw new HttpError(404, `There is no version ${version} of \`${name}\` on this registry.`);
  console.log(JSON.stringify({ event: yanked ? "yank" : "unyank", package: name, version, user }));
  return ok();
}

// GET /api/v1/me
export async function me(request: Request, env: Env): Promise<Response> {
  return json({ user: await requireUser(request, env) });
}

export interface SearchHit {
  name: string;
  version: string | null;
  description: string | null;
}

export async function searchPackages(env: Env, q: string): Promise<SearchHit[]> {
  const pattern = `%${q.replace(/[\\%_]/g, (c) => `\\${c}`)}%`;
  const { results: packages } = await env.DB.prepare(
    `SELECT name, description FROM packages
     WHERE ? = '' OR name LIKE ? ESCAPE '\\' OR description LIKE ? ESCAPE '\\'
     ORDER BY updated_at DESC, name ASC LIMIT 50`,
  )
    .bind(q, pattern, pattern)
    .all<{ name: string; description: string | null }>();
  if (packages.length === 0) return [];
  const { results: versions } = await env.DB.prepare(
    "SELECT package, version, yanked FROM versions WHERE package IN (SELECT value FROM json_each(?))",
  )
    .bind(JSON.stringify(packages.map((p) => p.name)))
    .all<{ package: string; version: string; yanked: number }>();
  const byPackage = new Map<string, { version: string; yanked: number }[]>();
  for (const v of versions) {
    const list = byPackage.get(v.package) ?? [];
    list.push(v);
    byPackage.set(v.package, list);
  }
  return packages.map((p) => ({
    name: p.name,
    version: newestVersion(byPackage.get(p.name) ?? []),
    description: p.description,
  }));
}

// GET /api/v1/search?q=
export async function search(env: Env, url: URL): Promise<Response> {
  const q = (url.searchParams.get("q") ?? "").trim();
  return json({ packages: await searchPackages(env, q) }, 200, { "cache-control": "public, max-age=60" });
}

// POST /api/v1/admin/tokens
export async function createToken(request: Request, env: Env): Promise<Response> {
  await requireAdmin(request, env);
  const body = await readBody(request, 64 * 1024, "The request body is too large.");
  let user: unknown;
  try {
    user = (JSON.parse(new TextDecoder().decode(body)) as { user?: unknown }).user;
  } catch {
    throw new HttpError(400, 'The body must be JSON like {"user": "<name>"}.');
  }
  if (typeof user !== "string" || !/^[a-z][a-z0-9_-]{0,63}$/.test(user)) {
    throw new HttpError(400, "User names start with a lowercase ASCII letter and contain only lowercase letters, digits, `_` and `-` (at most 64 characters).");
  }
  const token = newToken();
  const time = now();
  await env.DB.batch([
    env.DB.prepare("INSERT OR IGNORE INTO users (name, created_at) VALUES (?, ?)").bind(user, time),
    env.DB.prepare("INSERT INTO tokens (hash, user, created_at) VALUES (?, ?, ?)").bind(await sha256Hex(token), user, time),
  ]);
  console.log(JSON.stringify({ event: "token_created", user }));
  return json({ token });
}
