// Semantic versions and version requirements, as described in docs/registry.md.
//
// Versions are `MAJOR.MINOR.PATCH` with an optional `-pre.release` part and no
// build metadata. Requirements are comparators separated by `,` that must all hold.

export interface Version {
  major: bigint;
  minor: bigint;
  patch: bigint;
  /** Pre-release identifiers; empty for a release. */
  pre: string[];
}

const NUMBER = /^(0|[1-9][0-9]*)$/;
const PRE_PART = /^[0-9A-Za-z-]+$/;
const U64_MAX = (1n << 64n) - 1n;

function parseNumber(text: string): bigint | null {
  if (!NUMBER.test(text)) return null;
  const n = BigInt(text);
  return n <= U64_MAX ? n : null;
}

function parsePre(text: string): string[] | null {
  const parts = text.split(".");
  // Numeric identifiers have no leading zeros.
  return parts.every((p) => PRE_PART.test(p) && !/^0[0-9]+$/.test(p)) ? parts : null;
}

/** Parses `MAJOR.MINOR.PATCH[-pre]`. Build metadata (`+...`) is not allowed. */
export function parseVersion(text: string): Version | null {
  const dash = text.indexOf("-");
  const core = dash === -1 ? text : text.slice(0, dash);
  const pre = dash === -1 ? [] : parsePre(text.slice(dash + 1));
  if (pre === null) return null;
  const numbers = core.split(".");
  if (numbers.length !== 3) return null;
  const [major, minor, patch] = numbers.map(parseNumber);
  if (major == null || minor == null || patch == null) return null;
  return { major, minor, patch, pre };
}

export function formatVersion(v: Version): string {
  const core = `${v.major}.${v.minor}.${v.patch}`;
  return v.pre.length ? `${core}-${v.pre.join(".")}` : core;
}

function cmpBig(a: bigint, b: bigint): number {
  return a < b ? -1 : a > b ? 1 : 0;
}

function cmpPreIdent(a: string, b: string): number {
  const an = /^[0-9]+$/.test(a);
  const bn = /^[0-9]+$/.test(b);
  if (an && bn) return cmpBig(BigInt(a), BigInt(b));
  if (an) return -1;
  if (bn) return 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Semantic version precedence: negative if `a < b`, zero if equal, positive if `a > b`. */
export function compareVersions(a: Version, b: Version): number {
  const core = cmpBig(a.major, b.major) || cmpBig(a.minor, b.minor) || cmpBig(a.patch, b.patch);
  if (core !== 0) return core;
  if (a.pre.length === 0 || b.pre.length === 0) return b.pre.length - a.pre.length;
  const n = Math.min(a.pre.length, b.pre.length);
  for (let i = 0; i < n; i++) {
    const c = cmpPreIdent(a.pre[i]!, b.pre[i]!);
    if (c !== 0) return c;
  }
  return a.pre.length - b.pre.length;
}

/** Compares version strings; unparsable versions sort first. */
export function compareVersionStrings(a: string, b: string): number {
  const va = parseVersion(a);
  const vb = parseVersion(b);
  if (!va || !vb) return va ? 1 : vb ? -1 : a < b ? -1 : a > b ? 1 : 0;
  return compareVersions(va, vb);
}

/** `<!` is `<` that also keeps out pre-releases of the bound's own version (`<2` and `2.0.0-beta`). */
type BoundOp = ">=" | ">" | "<" | "<!" | "<=" | "=";

interface Bound {
  op: BoundOp;
  version: Version;
}

interface Comparator {
  bounds: Bound[];
  /** The `MAJOR.MINOR.PATCH` of a comparator written with a pre-release. */
  preTriple: Version | null;
}

export interface Requirement {
  comparators: Comparator[];
}

const v = (major: bigint, minor: bigint, patch: bigint, pre: string[] = []): Version => ({
  major,
  minor,
  patch,
  pre,
});

interface Partial {
  major: bigint;
  minor: bigint | null;
  patch: bigint | null;
  pre: string[];
}

function parsePartial(text: string): Partial | null {
  const dash = text.indexOf("-");
  const core = dash === -1 ? text : text.slice(0, dash);
  const pre = dash === -1 ? [] : parsePre(text.slice(dash + 1));
  if (pre === null) return null;
  const numbers = core.split(".");
  if (numbers.length < 1 || numbers.length > 3) return null;
  const parsed = numbers.map(parseNumber);
  if (parsed.some((n) => n === null)) return null;
  // A pre-release needs the full MAJOR.MINOR.PATCH.
  if (pre.length && parsed.length !== 3) return null;
  return { major: parsed[0]!, minor: parsed[1] ?? null, patch: parsed[2] ?? null, pre };
}

function caretBounds(p: Partial): Bound[] {
  const lower: Bound = { op: ">=", version: v(p.major, p.minor ?? 0n, p.patch ?? 0n, p.pre) };
  let upper: Version;
  if (p.major > 0n || p.minor === null) upper = v(p.major + 1n, 0n, 0n);
  else if (p.minor > 0n || p.patch === null) upper = v(0n, p.minor + 1n, 0n);
  else upper = v(0n, 0n, p.patch + 1n);
  return [lower, { op: "<", version: upper }];
}

function tildeBounds(p: Partial): Bound[] {
  const lower: Bound = { op: ">=", version: v(p.major, p.minor ?? 0n, p.patch ?? 0n, p.pre) };
  const upper = p.minor === null ? v(p.major + 1n, 0n, 0n) : v(p.major, p.minor + 1n, 0n);
  return [lower, { op: "<", version: upper }];
}

/** The versions a partial version stands for, as `[lowest, first after]`. */
function partialRange(p: Partial): [Version, Version] {
  if (p.minor === null) return [v(p.major, 0n, 0n), v(p.major + 1n, 0n, 0n)];
  return [v(p.major, p.minor, 0n), v(p.major, p.minor + 1n, 0n)];
}

function parseComparator(text: string): Comparator | null {
  const match = /^(\^|~|=|>=|<=|>|<)?\s*(.*)$/.exec(text);
  if (!match) return null;
  const op = match[1] ?? "^";
  const p = parsePartial(match[2]!);
  if (!p) return null;
  const full = p.minor !== null && p.patch !== null;
  const exact = v(p.major, p.minor ?? 0n, p.patch ?? 0n, p.pre);
  const preTriple = p.pre.length ? v(p.major, p.minor!, p.patch!) : null;
  let bounds: Bound[];
  switch (op) {
    case "^":
      bounds = caretBounds(p);
      break;
    case "~":
      bounds = tildeBounds(p);
      break;
    case "=": {
      if (full) bounds = [{ op: "=", version: exact }];
      else {
        const [lo, hi] = partialRange(p);
        bounds = [{ op: ">=", version: lo }, { op: "<", version: hi }];
      }
      break;
    }
    case ">":
      bounds = [full ? { op: ">", version: exact } : { op: ">=", version: partialRange(p)[1] }];
      break;
    case ">=":
      bounds = [{ op: ">=", version: exact }];
      break;
    case "<":
      bounds = [{ op: p.pre.length ? "<" : "<!", version: exact }];
      break;
    case "<=":
      bounds = [full ? { op: "<=", version: exact } : { op: "<", version: partialRange(p)[1] }];
      break;
    default:
      return null;
  }
  return { bounds, preTriple };
}

/** Parses a requirement such as `^1.2`, `>=1.0, <2` or `*`. */
export function parseRequirement(text: string): Requirement | null {
  const trimmed = text.trim();
  if (trimmed === "") return null;
  // `*` stands alone; it is not a comparator in a list.
  if (trimmed === "*") return { comparators: [] };
  const comparators: Comparator[] = [];
  for (const part of trimmed.split(",")) {
    const c = parseComparator(part.trim());
    if (!c) return null;
    comparators.push(c);
  }
  return { comparators };
}

function holds(bound: Bound, version: Version): boolean {
  const c = compareVersions(version, bound.version);
  switch (bound.op) {
    case "=":
      return c === 0;
    case ">":
      return c > 0;
    case ">=":
      return c >= 0;
    case "<":
      return c < 0;
    case "<!":
      return c < 0 && !sameTriple(version, bound.version);
    case "<=":
      return c <= 0;
  }
}

const sameTriple = (a: Version, b: Version) =>
  a.major === b.major && a.minor === b.minor && a.patch === b.patch;

/** Whether `version` meets every comparator of `req`. */
export function matches(req: Requirement, version: Version): boolean {
  if (!req.comparators.every((c) => c.bounds.every((b) => holds(b, version)))) return false;
  if (version.pre.length === 0) return true;
  // A pre-release only matches when some comparator names a pre-release of the same triple.
  return req.comparators.some((c) => c.preTriple !== null && sameTriple(c.preTriple, version));
}
