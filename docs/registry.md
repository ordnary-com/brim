# The package registry

Brim packages can be published to a registry and used by version:

```toml
[dependencies]
http_router = "1.2"
```

This document is the protocol between `brim` and a registry. The registry
service itself lives in `registry/` (a Cloudflare Worker with R2 for archives
and D1 for the index).

## Where the registry is

`brim` uses the registry at `$BRIM_REGISTRY`, or the default registry built
into the compiler (`brim_package::registry::DEFAULT_REGISTRY`). URLs below are
relative to it and have no trailing slash.

## Package names and versions

- Names follow the manifest's rules (`brim_manifest::valid_name`). Registry
  packages are imported by name, so names on the registry may not contain `-`
  (`brim_manifest::valid_import_name`). Names are compared exactly; the
  registry also refuses a new name that equals an existing one after `_` is
  removed, so `httprouter` and `http_router` cannot both exist.
- Versions are semantic versions, `MAJOR.MINOR.PATCH` with an optional
  `-pre.release` part. Build metadata (`+...`) is not allowed.
- A published version never changes and is never deleted. It can be yanked:
  yanked versions are not chosen for new lock files, but projects whose lock
  file names them keep working.

## Version requirements

A requirement is one or more comparators separated by `,`, which must all
hold:

| Requirement      | Means                                   |
|------------------|-----------------------------------------|
| `1.2.3`, `^1.2.3`| `>=1.2.3, <2.0.0`                       |
| `1.2`, `^1.2`    | `>=1.2.0, <2.0.0`                       |
| `1`, `^1`        | `>=1.0.0, <2.0.0`                       |
| `^0.2.3`         | `>=0.2.3, <0.3.0`                       |
| `^0.0.3`         | `>=0.0.3, <0.0.4`                       |
| `~1.2.3`         | `>=1.2.3, <1.3.0`                       |
| `~1`             | `>=1.0.0, <2.0.0`                       |
| `=1.2.3`         | exactly `1.2.3`                         |
| `>1.2`, `>=1.2`, `<2`, `<=1.4.0` | comparisons            |
| `*`              | any version                             |

A pre-release version (`1.3.0-beta.1`) only matches a requirement that has a
comparator with a pre-release on the same `MAJOR.MINOR.PATCH`, so `^1.2`
never picks a beta.

## Resolution

Every package of a project has one version (as before: a name is one
package). `brim` picks, for every registry package, the newest non-yanked
version that meets every requirement on it in the project, and keeps the
version in `brim.lock` as long as it still meets them. `brim update` forgets
the lock file and picks the newest versions again.

## The API

All responses are JSON (`content-type: application/json`) unless said
otherwise. Errors have a 4xx or 5xx status and the body
`{"error": "a sentence for the user"}`, which `brim` shows as it is.

### `GET /api/v1/index/{name}`

The versions of a package, oldest first.

```json
{
  "name": "http_router",
  "versions": [
    {
      "version": "1.2.0",
      "checksum": "<sha256 of the archive, lowercase hex>",
      "edition": "2026",
      "dependencies": { "json_extra": "^0.3" },
      "yanked": false,
      "published_at": "2026-10-17T12:00:00Z"
    }
  ]
}
```

`404` with an error when there is no such package. Responses may be cached
for 60 seconds.

### `GET /api/v1/download/{name}/{version}`

The archive, `content-type: application/gzip`: a gzip-compressed tar file
whose entries are all under `{name}-{version}/`, with the package's
`brim.toml` at `{name}-{version}/brim.toml`. Only regular files and
directories. Archives are immutable and may be cached forever.

### `PUT /api/v1/publish`

Publishes a version. `Authorization: Bearer <token>`. The body is binary:

```text
u32 little-endian   length of the metadata JSON
...                 metadata JSON (UTF-8)
u32 little-endian   length of the archive
...                 the archive (.tar.gz, as for downloads)
```

The metadata:

```json
{
  "name": "http_router",
  "version": "1.2.0",
  "edition": "2026",
  "checksum": "<sha256 of the archive, lowercase hex>",
  "dependencies": { "json_extra": "^0.3" },
  "description": "Routing for std.http servers",
  "license": "Apache-2.0",
  "repository": "https://github.com/example/http_router"
}
```

`description`, `license` and `repository` may be absent or `null`.

Without a valid token the registry answers `401`. It checks, and answers
`400` (or `403`, `409`, `413`) with an error otherwise:

- the body framing, and that the archive's SHA-256 is `checksum`;
- the name, the version (no build metadata) and the edition (`2026`);
- that the archive is at most 10 MiB;
- that every dependency name exists on the registry and every requirement
  is well-formed;
- that the name is new (it then belongs to the token's user) or belongs to
  the token's user (`403` otherwise);
- that the version does not exist yet (`409`).

Success: `200` with `{"ok": true}`.

### `DELETE /api/v1/packages/{name}/{version}/yank` and `PUT /api/v1/packages/{name}/{version}/unyank`

Yanks or unyanks a version. `Authorization: Bearer <token>` of the
package's owner. `200` with `{"ok": true}`; `401` without a valid token,
`403` for another user's package, `404` when the package or version does
not exist.

### `GET /api/v1/me`

`Authorization: Bearer <token>`. `200` with `{"user": "<name>"}`, or `401`.
`brim login` uses it to check a token.

### `GET /api/v1/search?q={text}`

Up to 50 packages whose name or description contains the text (all packages
when `q` is empty), newest first:

```json
{ "packages": [ { "name": "http_router", "version": "1.2.0", "description": "..." } ] }
```

`version` is the newest non-yanked version.

### `POST /api/v1/admin/tokens`

`Authorization: Bearer <ADMIN_TOKEN>` (a secret of the service). Body
`{"user": "<name>"}`. Creates an API token for that user (and the user, if
new): `200` with `{"token": "brim_<random>"}`. The registry stores only the
token's SHA-256. Until accounts come from Ordnary's identity service, this is
how publishers get a token.

## On the client

- `brim login` reads a token (from standard input, or `--token`) and stores
  it in `$BRIM_HOME/credentials.toml`; `BRIM_REGISTRY_TOKEN` overrides it.
- `brim publish [--dry-run]` checks the package, builds the archive (the
  manifest, `src/`, and `README*`, `LICENSE*` and `CHANGELOG*` at the top),
  and uploads it. Every dependency needs a `version`; `path` dependencies may
  have one too (`{ path = "../b", version = "0.2" }`), which is what the
  published manifest keeps. Git dependencies cannot be published.
- `brim yank NAME@VERSION [--undo]`.
- Downloaded archives are checked against the index checksum and unpacked
  into `$BRIM_HOME/registry/src/<registry>/<name>-<version>/`.
