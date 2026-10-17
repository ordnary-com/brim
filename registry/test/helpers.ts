import { expect } from "vitest";
import { exports } from "cloudflare:workers";

export const ADMIN_TOKEN = "test-admin-token";
const BASE = "https://registry.test";

export function call(path: string, init: RequestInit = {}): Promise<Response> {
  return exports.default.fetch(new Request(`${BASE}${path}`, init));
}

export interface TarEntry {
  path: string;
  /** Tar type flag; `0` (regular file) by default. */
  type?: string;
  data?: string | Uint8Array;
}

function writeString(block: Uint8Array, offset: number, length: number, text: string): void {
  const bytes = new TextEncoder().encode(text);
  if (bytes.length > length) throw new Error(`tar field too long: ${text}`);
  block.set(bytes, offset);
}

function writeOctal(block: Uint8Array, offset: number, length: number, value: number): void {
  writeString(block, offset, length, value.toString(8).padStart(length - 1, "0") + "\0");
}

/** A tiny ustar writer for test fixtures. */
export function tar(entries: TarEntry[]): Uint8Array {
  const blocks: Uint8Array[] = [];
  for (const entry of entries) {
    const data = typeof entry.data === "string" ? new TextEncoder().encode(entry.data) : (entry.data ?? new Uint8Array());
    const header = new Uint8Array(512);
    let name = entry.path;
    let prefix = "";
    if (name.length > 100) {
      const cut = name.lastIndexOf("/", 155);
      prefix = name.slice(0, cut);
      name = name.slice(cut + 1);
    }
    const type = entry.type ?? "0";
    writeString(header, 0, 100, name);
    writeOctal(header, 100, 8, type === "5" ? 0o755 : 0o644);
    writeOctal(header, 108, 8, 0);
    writeOctal(header, 116, 8, 0);
    writeOctal(header, 124, 12, data.length);
    writeOctal(header, 136, 12, 0);
    header.fill(0x20, 148, 156);
    writeString(header, 156, 1, type);
    writeString(header, 257, 6, "ustar\0");
    writeString(header, 263, 2, "00");
    writeString(header, 345, 155, prefix);
    let sum = 0;
    for (const b of header) sum += b;
    writeString(header, 148, 8, sum.toString(8).padStart(6, "0") + "\0 ");
    blocks.push(header);
    if (data.length) {
      const padded = new Uint8Array(Math.ceil(data.length / 512) * 512);
      padded.set(data);
      blocks.push(padded);
    }
  }
  blocks.push(new Uint8Array(1024));
  return concat(blocks);
}

export function concat(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}

export async function gzip(bytes: Uint8Array): Promise<Uint8Array> {
  const stream = new Blob([bytes]).stream().pipeThrough(new CompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

export async function sha256(bytes: Uint8Array): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return [...digest].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** A valid archive for `name`@`version`. */
export function packageArchive(
  name: string,
  version: string,
  extra: TarEntry[] = [],
  dependencies: Record<string, string> = {},
): Promise<Uint8Array> {
  const root = `${name}-${version}`;
  const deps = Object.entries(dependencies);
  const depText = deps.length ? `\n[dependencies]\n${deps.map(([d, r]) => `${d} = "${r}"\n`).join("")}` : "";
  return gzip(
    tar([
      { path: `${root}/`, type: "5" },
      { path: `${root}/brim.toml`, data: `[package]\nname = "${name}"\nversion = "${version}"\nedition = "2026"\n${depText}` },
      { path: `${root}/src/`, type: "5" },
      { path: `${root}/src/lib.brim`, data: "pub fn hello() -> Int { 42 }\n" },
      ...extra,
    ]),
  );
}

/** Frames metadata and archive as a publish body. */
export function frame(metadata: unknown, archive: Uint8Array): Uint8Array {
  const meta = new TextEncoder().encode(typeof metadata === "string" ? metadata : JSON.stringify(metadata));
  const head = new Uint8Array(4);
  new DataView(head.buffer).setUint32(0, meta.length, true);
  const mid = new Uint8Array(4);
  new DataView(mid.buffer).setUint32(0, archive.length, true);
  return concat([head, meta, mid, archive]);
}

/** A token for `user`, also when the user exists already. */
export async function createToken(user: string): Promise<string> {
  const post = (existing: boolean) =>
    call("/api/v1/admin/tokens", {
      method: "POST",
      headers: { authorization: `Bearer ${ADMIN_TOKEN}`, "content-type": "application/json" },
      body: JSON.stringify({ user, existing }),
    });
  let res = await post(false);
  if (res.status === 409) res = await post(true);
  if (res.status !== 200) throw new Error(`token creation failed: ${res.status} ${await res.text()}`);
  return ((await res.json()) as { token: string }).token;
}

export interface PublishOptions {
  dependencies?: Record<string, string>;
  description?: string | null;
  archive?: Uint8Array;
  metadata?: Record<string, unknown>;
}

export async function publish(token: string, name: string, version: string, opts: PublishOptions = {}): Promise<Response> {
  const archive = opts.archive ?? (await packageArchive(name, version, [], opts.dependencies ?? {}));
  const metadata = {
    name,
    version,
    edition: "2026",
    checksum: await sha256(archive),
    dependencies: opts.dependencies ?? {},
    description: opts.description ?? `The ${name} package`,
    license: "Apache-2.0",
    repository: `https://example.com/${name}`,
    ...opts.metadata,
  };
  return call("/api/v1/publish", {
    method: "PUT",
    headers: { authorization: `Bearer ${token}` },
    body: frame(metadata, archive),
  });
}

export async function errorOf(res: Response): Promise<string> {
  expect(res.headers.get("content-type")).toBe("application/json");
  return ((await res.json()) as { error: string }).error;
}

let counter = 0;
/** A package name not used by any other test. */
export function uniqueName(base: string): string {
  counter += 1;
  return `${base}_${Date.now().toString(36)}${counter}`;
}
