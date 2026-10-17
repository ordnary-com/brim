-- The registry index. Archives live in R2 under `{name}/{name}-{version}.tar.gz`.

CREATE TABLE users (
  name       TEXT PRIMARY KEY,
  created_at TEXT NOT NULL
);

-- Only the SHA-256 (lowercase hex) of a token is stored.
CREATE TABLE tokens (
  hash       TEXT PRIMARY KEY,
  user       TEXT NOT NULL REFERENCES users (name),
  created_at TEXT NOT NULL
);

CREATE INDEX tokens_user ON tokens (user);

CREATE TABLE packages (
  name        TEXT PRIMARY KEY,
  -- The name without `_`, so that `http_router` and `httprouter` cannot both exist.
  normalized  TEXT NOT NULL UNIQUE,
  owner       TEXT NOT NULL REFERENCES users (name),
  description TEXT,
  license     TEXT,
  repository  TEXT,
  created_at  TEXT NOT NULL,
  -- When a version was last published; search lists packages by this, newest first.
  updated_at  TEXT NOT NULL
);

CREATE INDEX packages_updated_at ON packages (updated_at);

CREATE TABLE versions (
  package      TEXT NOT NULL REFERENCES packages (name),
  version      TEXT NOT NULL,
  checksum     TEXT NOT NULL,
  edition      TEXT NOT NULL,
  -- JSON object of dependency name to version requirement.
  dependencies TEXT NOT NULL,
  yanked       INTEGER NOT NULL DEFAULT 0,
  published_at TEXT NOT NULL,
  -- Size of the archive in bytes.
  size         INTEGER NOT NULL,
  PRIMARY KEY (package, version)
);
