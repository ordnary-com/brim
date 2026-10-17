import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import {
  ADMIN_TOKEN,
  call,
  createToken,
  errorOf,
  frame,
  gzip,
  packageArchive,
  publish,
  sha256,
  tar,
  uniqueName,
} from "./helpers";

interface IndexBody {
  name: string;
  versions: { version: string; checksum: string; edition: string; dependencies: Record<string, string>; yanked: boolean; published_at: string }[];
}

describe("admin tokens and /me", () => {
  it("creates a token that /me recognizes", async () => {
    const token = await createToken("alice");
    expect(token).toMatch(/^brim_[A-Za-z0-9_-]{43}$/);

    const res = await call("/api/v1/me", { headers: { authorization: `Bearer ${token}` } });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ user: "alice" });

    // Only the hash is stored.
    const row = await env.DB.prepare("SELECT hash FROM tokens WHERE hash = ?").bind(await sha256(new TextEncoder().encode(token))).first();
    expect(row).not.toBeNull();
    const plain = await env.DB.prepare("SELECT hash FROM tokens WHERE hash = ?").bind(token).first();
    expect(plain).toBeNull();
  });

  it("gives every call a new token", async () => {
    const a = await createToken("bob");
    const b = await createToken("bob");
    expect(a).not.toBe(b);
  });

  it("refuses a wrong admin token", async () => {
    const res = await call("/api/v1/admin/tokens", {
      method: "POST",
      headers: { authorization: "Bearer nope" },
      body: JSON.stringify({ user: "mallory" }),
    });
    expect(res.status).toBe(401);
    expect(await errorOf(res)).toMatch(/admin token/);
  });

  it("refuses a bad user name", async () => {
    const res = await call("/api/v1/admin/tokens", {
      method: "POST",
      headers: { authorization: `Bearer ${ADMIN_TOKEN}` },
      body: JSON.stringify({ user: "Not A Name" }),
    });
    expect(res.status).toBe(400);
  });

  it("answers 401 without or with a bad token", async () => {
    const none = await call("/api/v1/me");
    expect(none.status).toBe(401);
    expect(await errorOf(none)).toMatch(/API token/);
    const bad = await call("/api/v1/me", { headers: { authorization: "Bearer brim_nothing" } });
    expect(bad.status).toBe(401);
  });
});

describe("publish", () => {
  it("publishes a version, then serves its index and archive", async () => {
    const token = await createToken("carol");
    const name = uniqueName("http_router");
    const archive = await packageArchive(name, "1.2.0");
    const res = await publish(token, name, "1.2.0", { archive });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });

    const index = await call(`/api/v1/index/${name}`);
    expect(index.status).toBe(200);
    expect(index.headers.get("content-type")).toBe("application/json");
    expect(index.headers.get("cache-control")).toBe("public, max-age=60");
    const body = (await index.json()) as IndexBody;
    expect(body.name).toBe(name);
    expect(body.versions).toHaveLength(1);
    expect(body.versions[0]).toMatchObject({
      version: "1.2.0",
      checksum: await sha256(archive),
      edition: "2026",
      dependencies: {},
      yanked: false,
    });
    expect(body.versions[0]!.published_at).toMatch(/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/);

    const dl = await call(`/api/v1/download/${name}/1.2.0`);
    expect(dl.status).toBe(200);
    expect(dl.headers.get("content-type")).toBe("application/gzip");
    expect(dl.headers.get("cache-control")).toContain("immutable");
    expect(new Uint8Array(await dl.arrayBuffer())).toEqual(archive);

    const stored = await env.ARCHIVES.head(`${name}/${name}-1.2.0.tar.gz`);
    expect(stored?.size).toBe(archive.length);
  });

  it("needs a token", async () => {
    const archive = await packageArchive("anon", "1.0.0");
    const res = await call("/api/v1/publish", { method: "PUT", body: frame({}, archive) });
    expect(res.status).toBe(401);
  });

  it("checks the checksum", async () => {
    const token = await createToken("dave");
    const name = uniqueName("sums");
    const res = await publish(token, name, "1.0.0", { metadata: { checksum: "0".repeat(64) } });
    expect(res.status).toBe(400);
    expect(await errorOf(res)).toMatch(/SHA-256/);
    expect((await call(`/api/v1/index/${name}`)).status).toBe(404);
  });

  it("checks the framing", async () => {
    const token = await createToken("dave");
    const put = (body: Uint8Array) =>
      call("/api/v1/publish", { method: "PUT", headers: { authorization: `Bearer ${token}` }, body });
    const archive = await packageArchive("framed", "1.0.0");
    const good = frame({ name: "framed" }, archive);

    for (const body of [
      new Uint8Array([1, 0]),
      good.subarray(0, 10),
      good.subarray(0, good.length - 1),
      new Uint8Array([...good, 0]),
    ]) {
      const res = await put(body);
      expect(res.status).toBe(400);
      expect(await errorOf(res)).toMatch(/malformed/);
    }

    const notJson = await put(frame("{not json", archive));
    expect(notJson.status).toBe(400);
    expect(await errorOf(notJson)).toMatch(/JSON/);
  });

  it("refuses archives over 10 MiB", async () => {
    const token = await createToken("dave");
    const body = new Uint8Array(4 + 2 + 4);
    body.set([2, 0, 0, 0]);
    body.set(new TextEncoder().encode("{}"), 4);
    new DataView(body.buffer).setUint32(6, 11 * 1024 * 1024, true);
    const res = await call("/api/v1/publish", { method: "PUT", headers: { authorization: `Bearer ${token}` }, body });
    expect(res.status).toBe(413);
  });

  it("checks the name, version and edition", async () => {
    const token = await createToken("dave");
    const cases: [Record<string, unknown>, RegExp][] = [
      [{ name: "has-dash" }, /not a valid registry package name/],
      [{ name: "Upper" }, /not a valid registry package name/],
      [{ version: "1.0.0+build" }, /build metadata/],
      [{ version: "1.0" }, /not a valid version/],
      [{ edition: "2025" }, /Edition/],
    ];
    for (const [metadata, message] of cases) {
      const res = await publish(token, uniqueName("meta"), "1.0.0", { metadata });
      expect(res.status, JSON.stringify(metadata)).toBe(400);
      expect(await errorOf(res)).toMatch(message);
    }
  });

  it("rejects an entry outside the package directory", async () => {
    const token = await createToken("erin");
    const name = uniqueName("escape");
    const archive = await packageArchive(name, "1.0.0", [{ path: "other/evil.txt", data: "x" }]);
    const res = await publish(token, name, "1.0.0", { archive });
    expect(res.status).toBe(400);
    expect(await errorOf(res)).toMatch(/not under `[a-z0-9_]+-1\.0\.0\/`/);
  });

  it("rejects `..` and absolute paths", async () => {
    const token = await createToken("erin");
    for (const path of ["{root}/../evil", "/etc/passwd", "{root}/./x"]) {
      const name = uniqueName("dots");
      const archive = await packageArchive(name, "1.0.0", [{ path: path.replace("{root}", `${name}-1.0.0`), data: "x" }]);
      const res = await publish(token, name, "1.0.0", { archive });
      expect(res.status, path).toBe(400);
    }
  });

  it("rejects links and other entry types", async () => {
    const token = await createToken("erin");
    const name = uniqueName("links");
    const archive = await packageArchive(name, "1.0.0", [{ path: `${name}-1.0.0/link`, type: "2" }]);
    const res = await publish(token, name, "1.0.0", { archive });
    expect(res.status).toBe(400);
    expect(await errorOf(res)).toMatch(/not a regular file or a directory/);
  });

  it("requires brim.toml", async () => {
    const token = await createToken("erin");
    const name = uniqueName("nomanifest");
    const root = `${name}-1.0.0`;
    const archive = await gzip(tar([{ path: `${root}/src/lib.brim`, data: "x" }]));
    const res = await publish(token, name, "1.0.0", { archive });
    expect(res.status).toBe(400);
    expect(await errorOf(res)).toBe(`The archive has no \`${root}/brim.toml\`.`);
  });

  it("rejects an archive that is not gzip", async () => {
    const token = await createToken("erin");
    const name = uniqueName("raw");
    const archive = tar([{ path: `${name}-1.0.0/brim.toml`, data: "x" }]);
    const res = await publish(token, name, "1.0.0", { archive });
    expect(res.status).toBe(400);
    expect(await errorOf(res)).toMatch(/gzip/);
  });

  it("accepts long paths through the ustar prefix", async () => {
    const token = await createToken("erin");
    const name = uniqueName("deep");
    const long = `${name}-1.0.0/src/${"a".repeat(60)}/${"b".repeat(60)}/mod.brim`;
    const archive = await packageArchive(name, "1.0.0", [{ path: long, data: "x" }]);
    const res = await publish(token, name, "1.0.0", { archive });
    expect(res.status).toBe(200);
  });

  it("only lets the owner publish", async () => {
    const owner = await createToken("frank");
    const other = await createToken("grace");
    const name = uniqueName("owned");
    expect((await publish(owner, name, "1.0.0")).status).toBe(200);
    const res = await publish(other, name, "1.1.0");
    expect(res.status).toBe(403);
    expect(await errorOf(res)).toMatch(/belongs to another user/);
    expect((await publish(owner, name, "1.1.0")).status).toBe(200);
  });

  it("refuses a version that exists", async () => {
    const token = await createToken("heidi");
    const name = uniqueName("dup");
    expect((await publish(token, name, "1.0.0")).status).toBe(200);
    const res = await publish(token, name, "1.0.0", { description: "changed" });
    expect(res.status).toBe(409);
    expect(await errorOf(res)).toMatch(/already exists/);
  });

  it("checks dependencies", async () => {
    const token = await createToken("ivan");
    const dep = uniqueName("json_extra");
    expect((await publish(token, dep, "0.3.1")).status).toBe(200);

    const name = uniqueName("app");
    const missing = await publish(token, name, "1.0.0", { dependencies: { [dep]: "^0.3", nothere_xyz: "1" } });
    expect(missing.status).toBe(400);
    expect(await errorOf(missing)).toBe("Dependency `nothere_xyz` is not on the registry.");

    const malformed = await publish(token, name, "1.0.0", { dependencies: { [dep]: "^0.3+x" } });
    expect(malformed.status).toBe(400);
    expect(await errorOf(malformed)).toMatch(/not well-formed/);

    const good = await publish(token, name, "1.0.0", { dependencies: { [dep]: "^0.3" } });
    expect(good.status).toBe(200);
    const index = (await (await call(`/api/v1/index/${name}`)).json()) as IndexBody;
    expect(index.versions[0]!.dependencies).toEqual({ [dep]: "^0.3" });
  });

  it("refuses names that differ only in `_`", async () => {
    const token = await createToken("judy");
    const base = uniqueName("httprouter").replaceAll("_", "");
    const underscored = `${base.slice(0, 4)}_${base.slice(4)}`;
    expect((await publish(token, base, "1.0.0")).status).toBe(200);
    const res = await publish(token, underscored, "1.0.0");
    expect(res.status).toBe(400);
    expect(await errorOf(res)).toMatch(/too close to the existing package/);
  });
});

describe("yank", () => {
  it("yanks and unyanks, owner only", async () => {
    const owner = await createToken("kim");
    const other = await createToken("leo");
    const name = uniqueName("yankee");
    expect((await publish(owner, name, "1.0.0")).status).toBe(200);
    const yanked = async () => ((await (await call(`/api/v1/index/${name}`)).json()) as IndexBody).versions[0]!.yanked;

    const forbidden = await call(`/api/v1/packages/${name}/1.0.0/yank`, { method: "DELETE", headers: { authorization: `Bearer ${other}` } });
    expect(forbidden.status).toBe(403);

    const yank = await call(`/api/v1/packages/${name}/1.0.0/yank`, { method: "DELETE", headers: { authorization: `Bearer ${owner}` } });
    expect(yank.status).toBe(200);
    expect(await yank.json()).toEqual({ ok: true });
    expect(await yanked()).toBe(true);

    // Yanked archives stay downloadable for existing lock files.
    expect((await call(`/api/v1/download/${name}/1.0.0`)).status).toBe(200);

    const unyank = await call(`/api/v1/packages/${name}/1.0.0/unyank`, { method: "PUT", headers: { authorization: `Bearer ${owner}` } });
    expect(unyank.status).toBe(200);
    expect(await yanked()).toBe(false);

    const missing = await call(`/api/v1/packages/${name}/9.9.9/yank`, { method: "DELETE", headers: { authorization: `Bearer ${owner}` } });
    expect(missing.status).toBe(404);
    const wrongMethod = await call(`/api/v1/packages/${name}/1.0.0/yank`, { method: "PUT", headers: { authorization: `Bearer ${owner}` } });
    expect(wrongMethod.status).toBe(405);
  });
});

describe("index", () => {
  it("lists versions oldest first by semver precedence", async () => {
    const token = await createToken("mia");
    const name = uniqueName("ordered");
    for (const v of ["1.10.0", "1.2.0", "1.0.0-beta.2", "1.0.0", "1.0.0-beta.11", "0.9.0", "2.0.0-rc.1"]) {
      expect((await publish(token, name, v)).status, v).toBe(200);
    }
    const body = (await (await call(`/api/v1/index/${name}`)).json()) as IndexBody;
    expect(body.versions.map((v) => v.version)).toEqual(["0.9.0", "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0", "1.2.0", "1.10.0", "2.0.0-rc.1"]);
  });

  it("answers 404 for an unknown package", async () => {
    const res = await call("/api/v1/index/no_such_package");
    expect(res.status).toBe(404);
    expect(await errorOf(res)).toMatch(/no package/);
  });
});

describe("download", () => {
  it("answers 404 for an unknown version", async () => {
    const res = await call("/api/v1/download/no_such_package/1.0.0");
    expect(res.status).toBe(404);
    expect(await errorOf(res)).toMatch(/no version 1\.0\.0/);
  });
});

describe("search", () => {
  it("finds packages by name or description, with the newest non-yanked version", async () => {
    const token = await createToken("nina");
    const tag = uniqueName("zebra");
    const a = `${tag}_alpha`;
    const b = uniqueName("other");
    expect((await publish(token, a, "1.0.0")).status).toBe(200);
    expect((await publish(token, a, "1.1.0")).status).toBe(200);
    expect((await publish(token, a, "2.0.0-beta.1")).status).toBe(200);
    expect((await publish(token, b, "0.1.0", { description: `Works well with ${tag}` })).status).toBe(200);
    await call(`/api/v1/packages/${a}/1.1.0/yank`, { method: "DELETE", headers: { authorization: `Bearer ${token}` } });

    const res = await call(`/api/v1/search?q=${encodeURIComponent(tag)}`);
    expect(res.status).toBe(200);
    const { packages } = (await res.json()) as { packages: { name: string; version: string; description: string }[] };
    // Newest first: b was published last.
    expect(packages.map((p) => p.name)).toEqual([b, a]);
    expect(packages[1]).toEqual({ name: a, version: "1.0.0", description: `The ${a} package` });

    const all = (await (await call("/api/v1/search?q=")).json()) as { packages: unknown[] };
    expect(all.packages.length).toBeGreaterThanOrEqual(2);
    expect(all.packages.length).toBeLessThanOrEqual(50);
  });

  it("treats `%` and `_` literally", async () => {
    const res = await call("/api/v1/search?q=%25%25%25");
    expect(((await res.json()) as { packages: unknown[] }).packages).toEqual([]);
  });
});

describe("pages", () => {
  it("serves the package list and a package page", async () => {
    const token = await createToken("olga");
    const name = uniqueName("pretty");
    expect((await publish(token, name, "0.4.2", { description: "<b>bold</b> claims" })).status).toBe(200);

    const home = await call("/");
    expect(home.status).toBe(200);
    expect(home.headers.get("content-type")).toContain("text/html");
    const html = await home.text();
    expect(html).toContain(`/packages/${name}`);
    expect(html).toContain("&lt;b&gt;bold&lt;/b&gt;");
    expect(html).not.toContain("—");
    expect(html.toLowerCase()).not.toContain("cloudflare");

    const pkg = await call(`/packages/${name}`);
    expect(pkg.status).toBe(200);
    const page = await pkg.text();
    expect(page).toContain(`${name} = "0.4"`);
    expect(page).not.toContain("—");
    expect(page.toLowerCase()).not.toContain("cloudflare");

    expect((await call("/packages/nope_nope")).status).toBe(404);
    for (const path of ["/static/site.css", "/static/search.js"]) {
      const res = await call(path);
      expect(res.status).toBe(200);
      const text = await res.text();
      expect(text).not.toContain("—");
    }
  });
});
