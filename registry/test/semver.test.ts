import { describe, expect, it } from "vitest";
import { compareVersionStrings, formatVersion, matches, parseRequirement, parseVersion } from "../src/semver";

function ok(req: string, version: string): boolean {
  const r = parseRequirement(req);
  const v = parseVersion(version);
  if (!r) throw new Error(`bad requirement ${req}`);
  if (!v) throw new Error(`bad version ${version}`);
  return matches(r, v);
}

describe("parseVersion", () => {
  it("accepts versions", () => {
    for (const v of ["0.0.0", "1.2.3", "10.20.30", "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-0.3.7", "1.0.0-x-y-z.1"]) {
      expect(parseVersion(v), v).not.toBeNull();
      expect(formatVersion(parseVersion(v)!)).toBe(v);
    }
  });

  it("rejects malformed versions and build metadata", () => {
    for (const v of ["", "1", "1.2", "1.2.3.4", "01.2.3", "1.0.0-01", "1.02.3", "1.2.3-", "1.2.3-a..b", "1.2.3+build", "1.2.3-beta+b", "v1.2.3", "1.2.x", " 1.2.3", "18446744073709551616.0.0"]) {
      expect(parseVersion(v), v).toBeNull();
    }
  });
});

describe("compareVersions", () => {
  it("orders by semver precedence", () => {
    const ordered = [
      "0.9.0",
      "1.0.0-alpha",
      "1.0.0-alpha.1",
      "1.0.0-alpha.beta",
      "1.0.0-beta",
      "1.0.0-beta.2",
      "1.0.0-beta.11",
      "1.0.0-rc.1",
      "1.0.0",
      "1.2.0",
      "1.10.0",
      "2.0.0",
    ];
    const shuffled = [...ordered].reverse();
    expect(shuffled.sort(compareVersionStrings)).toEqual(ordered);
  });
});

describe("parseRequirement", () => {
  it("rejects malformed requirements", () => {
    for (const r of ["", " ", "1.2.3+b", "^", ">=", "1.x", "abc", "1.2,", ",1.2", "1.2-beta", "^^1", "=>1.0.0", "1.2.3.4", "*, >=1", ">=1, *"]) {
      expect(parseRequirement(r), r).toBeNull();
    }
  });

  it("accepts the documented forms", () => {
    for (const r of ["1.2.3", "^1.2.3", "1.2", "^1", "~1.2.3", "~1", "=1.2.3", ">1.2", ">=1.2", "<2", "<=1.4.0", "*", ">=1.0, <2", "^1.2.3-beta.1"]) {
      expect(parseRequirement(r), r).not.toBeNull();
    }
  });
});

describe("matches", () => {
  // Each row: requirement, versions that match, versions that do not.
  const table: [string, string[], string[]][] = [
    ["1.2.3", ["1.2.3", "1.9.0", "1.99.99"], ["1.2.2", "2.0.0", "0.9.0"]],
    ["^1.2.3", ["1.2.3", "1.3.0"], ["1.2.2", "2.0.0"]],
    ["1.2", ["1.2.0", "1.5.1"], ["1.1.9", "2.0.0"]],
    ["^1.2", ["1.2.0"], ["2.0.0"]],
    ["1", ["1.0.0", "1.9.9"], ["0.9.9", "2.0.0"]],
    ["^1", ["1.0.0"], ["2.0.0"]],
    ["^0.2.3", ["0.2.3", "0.2.9"], ["0.2.2", "0.3.0"]],
    ["^0.0.3", ["0.0.3"], ["0.0.2", "0.0.4"]],
    ["~1.2.3", ["1.2.3", "1.2.9"], ["1.2.2", "1.3.0"]],
    ["~1", ["1.0.0", "1.9.0"], ["2.0.0", "0.9.0"]],
    ["=1.2.3", ["1.2.3"], ["1.2.4", "1.2.2"]],
    [">1.2", ["1.3.0", "2.0.0"], ["1.2.0", "1.2.9"]],
    [">=1.2", ["1.2.0", "3.0.0"], ["1.1.9"]],
    ["<2", ["1.9.9", "0.1.0"], ["2.0.0", "2.0.1"]],
    ["<=1.4.0", ["1.4.0", "1.0.0"], ["1.4.1"]],
    ["*", ["0.0.1", "99.0.0"], []],
    [">=1.0, <1.5", ["1.0.0", "1.4.9"], ["1.5.0", "0.9.0"]],
    ["^0.2", ["0.2.0", "0.2.5"], ["0.3.0", "0.1.9"]],
  ];
  for (const [req, yes, no] of table) {
    it(req, () => {
      for (const v of yes) expect(ok(req, v), `${req} should match ${v}`).toBe(true);
      for (const v of no) expect(ok(req, v), `${req} should not match ${v}`).toBe(false);
    });
  }

  it("only picks pre-releases when asked for on the same version", () => {
    expect(ok("^1.2", "1.3.0-beta.1")).toBe(false);
    expect(ok("*", "1.3.0-beta.1")).toBe(false);
    expect(ok("^1.3.0-beta.1", "1.3.0-beta.1")).toBe(true);
    expect(ok("^1.3.0-beta.1", "1.3.0-beta.2")).toBe(true);
    expect(ok("^1.3.0-beta.1", "1.3.0-alpha")).toBe(false);
    expect(ok("^1.3.0-beta.1", "1.3.0")).toBe(true);
    expect(ok("^1.3.0-beta.1", "1.4.0-beta.1")).toBe(false);
    expect(ok("=1.3.0-beta.1", "1.3.0-beta.1")).toBe(true);
    expect(ok(">=1.3.0-beta.1, <2", "1.3.0-rc.1")).toBe(true);
    // `<2` keeps out 2.0.0's pre-releases, even when another comparator asks for them.
    expect(ok(">=2.0.0-alpha, <2", "2.0.0-beta")).toBe(false);
    expect(ok(">=2.0.0-alpha, <2.0.0-rc.1", "2.0.0-beta")).toBe(true);
  });
});
