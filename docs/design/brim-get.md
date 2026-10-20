# Proposal: `brim get`

Status: proposal, for discussion. Nothing here is built yet.

## Goal

Adding a library should be one command, as with `go get`:

```text
brim get github.com/ordnary-com/brim-redis
brim get github.com/ordnary-com/brim-redis@v1.4.2
```

The command finds the version, adds the dependency to `.brimproj`, records
the exact commit and a checksum in `.brimlock`, and fetches the code. The
next `brim build` works offline.

## What exists

- `[dependencies]` in `.brimproj` with `path = ...` or
  `git = ..., tag | branch | rev = ...`.
- `.brimlock` records the commit of every git dependency; `brim update`
  moves them to the newest commits.
- A cache in `~/.brim/git` (`$BRIM_HOME`): a bare clone per repository and
  a checkout per commit.
- `import name` uses the dependency's `[package] name`.

So `brim get` is mostly a front end to what exists, plus versions.

## Proposal

### 1. Names and imports

A package is fetched by its repository path (`github.com/owner/repo`), but
imported by a short name, the key in `[dependencies]`:

```toml
[dependencies]
redis = { git = "https://github.com/ordnary-com/brim-redis", version = "1.4" }
```

```brim
use @redis
```

The key defaults to the package's `[package] name`; `brim get ... --as
cache` picks another one when two packages share a name. Go imports by full
path instead; short names keep Brim's imports as they are today.

### 2. Versions are git tags

A version is a tag `vMAJOR.MINOR.PATCH` (semver). Without `@`, `brim get`
takes the newest tag that is not a pre-release; without tags, the newest
commit of the default branch (recorded as a commit). `@v1` means the newest
`v1.x.y`; `@main` or `@<commit>` pin a branch or commit.

In `.brimproj`, `version = "1.4"` means "1.4.0 or newer, below 2.0.0".

### 3. Resolution: minimal version selection

Every package states the lowest versions it needs. For each dependency the
build uses the highest of those minimums: the newest version that someone
asked for, never one nobody tested with. This is Go's algorithm: no solver,
the same answer on every machine, and no surprise upgrades when a library
publishes a release. `brim get -u` and `brim update` are the only ways to
move up.

A new major version is a different dependency: `v2` of a package is
imported under its own name and can sit next to `v1`.

### 4. Integrity

`.brimlock` gets a SHA-256 of each package's files next to its commit.
Every fetch checks it, so a moved tag or a changed repository fails the
build instead of changing it silently. Brim packages have no build
scripts, so fetching never runs code.

### 5. Commands

| Command | Effect |
|---------|--------|
| `brim get <repo>[@version]` | add or change a dependency, fetch it, update `.brimlock` |
| `brim get -u [name]` | newest compatible versions (all, or one) |
| `brim remove <name>` | drop a dependency |
| `brim update` | as today: newest commits within what `.brimproj` allows |
| `brim deps` | the dependency tree with versions |

Private repositories use git's own credentials (SSH keys, credential
helpers). `BRIM_OFFLINE=1` uses only the cache.

### 6. Later: a registry

A registry (`brim publish`, a search page, a cache in front of git hosts
and a checksum database like Go's `sum.golang.org`) can come after this
without changing `.brimproj`: a `version` without `git` would then come from
the registry.

## Questions to decide

1. **Imports:** short names (above) or full paths as in Go?
2. **Versions:** git tags only for now, with a registry later?
3. **Resolution:** minimal version selection (Go), or the newest compatible
   version (Cargo, npm)?
4. **Host:** should a registry, when it comes, run under Ordnary's own
   domain?

## Steps

1. `version = "..."` in `.brimproj`: semver tags, `@` forms, tag listing
   through `git ls-remote`.
2. Minimal version selection over the dependency graph.
3. Checksums in `.brimlock`.
4. `brim get`, `brim get -u`, `brim remove`, `brim deps`; documentation.
5. Later: the registry and `brim publish`.
