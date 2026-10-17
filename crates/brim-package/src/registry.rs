//! The package registry: its index, archives and API (see
//! `docs/registry.md`).
//!
//! ```text
//! $BRIM_HOME/registry/src/<registry key>/<name>-<version>/   unpacked archive
//! ```
//!
//! A registry URL is `http(s)://...`, or `file:///dir` for a directory laid
//! out like the API (`dir/api/v1/index/<name>`,
//! `dir/api/v1/download/<name>/<version>`), which the tests use.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use brim_manifest::{Version, VersionReq};
use serde::Deserialize;

/// The registry used when `$BRIM_REGISTRY` is not set.
pub const DEFAULT_REGISTRY: &str = "https://registry.brimlang.com";

/// The largest archive the registry accepts.
pub const MAX_ARCHIVE: usize = 10 * 1024 * 1024;

/// The registry to use: `$BRIM_REGISTRY` or [`DEFAULT_REGISTRY`], without
/// a trailing `/`.
pub fn default_url() -> String {
    std::env::var("BRIM_REGISTRY")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// A version in the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexVersion {
    pub version: Version,
    pub checksum: String,
    pub dependencies: Vec<(String, VersionReq)>,
    pub yanked: bool,
}

#[derive(Deserialize)]
struct RawIndex {
    versions: Vec<RawIndexVersion>,
}

#[derive(Deserialize)]
struct RawIndexVersion {
    version: String,
    checksum: String,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default)]
    yanked: bool,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

/// A registry at one URL.
#[derive(Clone, Debug)]
pub struct Registry {
    pub url: String,
    /// Never use the network (see `Options::offline`).
    pub offline: bool,
}

/// The most a registry response may hold: archives are at most 10 MiB.
const MAX_RESPONSE: usize = 32 * 1024 * 1024;

/// Whether a URL is plain `http://` to another machine, where a token would
/// travel readably.
fn insecure(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    !matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

impl Registry {
    pub fn new(url: &str) -> Registry {
        Registry {
            url: url.trim_end_matches('/').to_string(),
            offline: false,
        }
    }

    /// The registry's directory name in the cache.
    pub fn key(&self) -> String {
        let host = self
            .url
            .split("://")
            .nth(1)
            .unwrap_or(&self.url)
            .split(['/', ':'])
            .next()
            .unwrap_or("")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let host = if host.is_empty() {
            "local".into()
        } else {
            host
        };
        format!("{host}-{:016x}", crate::git::fnv1a(self.url.as_bytes()))
    }

    /// The versions of `name`; `Ok(None)` when the registry has no such
    /// package.
    pub fn index(&self, name: &str) -> Result<Option<Vec<IndexVersion>>, String> {
        let response = self.get(&format!("/api/v1/index/{name}"))?;
        if response.status == 404 {
            return Ok(None);
        }
        let body = response.ok()?;
        let raw: RawIndex = serde_json::from_slice(&body)
            .map_err(|e| format!("the registry sent an invalid index for `{name}`: {e}"))?;
        let mut versions = Vec::new();
        for v in raw.versions {
            // Versions this compiler cannot read are left out rather than
            // failing every project that uses the package.
            let Some(version) = Version::parse(&v.version) else {
                continue;
            };
            let Ok(dependencies) = v
                .dependencies
                .iter()
                .map(|(n, r)| VersionReq::parse(r).map(|r| (n.clone(), r)))
                .collect::<Result<Vec<_>, _>>()
            else {
                continue;
            };
            versions.push(IndexVersion {
                version,
                checksum: v.checksum,
                dependencies,
                yanked: v.yanked,
            });
        }
        versions.sort_by(|a, b| a.version.cmp(&b.version));
        Ok(Some(versions))
    }

    /// The directory with `name` `version` unpacked, downloading it when it
    /// is not in the cache (then also `true`). The archive must have the
    /// SHA-256 `checksum`.
    pub fn source(
        &self,
        home: &Path,
        name: &str,
        version: &Version,
        checksum: &str,
    ) -> Result<(PathBuf, bool), String> {
        let dir = self.source_dir(home, name, version);
        // The cache is used only for an archive with this very checksum.
        if brim_manifest::manifest_path(&dir).is_file()
            && cached_checksum(&dir).as_deref() == Some(checksum)
        {
            return Ok((dir, false));
        }
        let response = self.get(&format!("/api/v1/download/{name}/{version}"))?;
        let archive = response.ok()?;
        let actual = hex(&brim_runtime::crypto::sha256(&archive));
        if actual != checksum {
            return Err(format!(
                "the archive of {name} {version} has the checksum {actual}, \
                 but the index says {checksum}"
            ));
        }
        let parent = dir.parent().expect("source dirs have a parent");
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let tmp = parent.join(format!("{name}-{version}.tmp"));
        let _ = std::fs::remove_dir_all(&tmp);
        unpack(&archive, &format!("{name}-{version}"), &tmp)?;
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&tmp, &dir).map_err(|e| e.to_string())?;
        std::fs::write(checksum_file(&dir), checksum).map_err(|e| e.to_string())?;
        Ok((dir, true))
    }

    /// Where [`Registry::source`] unpacks a version.
    pub fn source_dir(&self, home: &Path, name: &str, version: &Version) -> PathBuf {
        home.join("registry")
            .join("src")
            .join(self.key())
            .join(format!("{name}-{version}"))
    }

    /// Publishes an archive (see `PUT /api/v1/publish`).
    pub fn publish(&self, token: &str, metadata: &str, archive: &[u8]) -> Result<(), String> {
        let mut body = Vec::with_capacity(metadata.len() + archive.len() + 8);
        body.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        body.extend_from_slice(metadata.as_bytes());
        body.extend_from_slice(&(archive.len() as u32).to_le_bytes());
        body.extend_from_slice(archive);
        self.send("PUT", "/api/v1/publish", Some(token), &body)?
            .ok()
            .map(|_| ())
    }

    /// Yanks a version, or takes the yank back.
    pub fn yank(&self, token: &str, name: &str, version: &str, undo: bool) -> Result<(), String> {
        let (method, action) = if undo {
            ("PUT", "unyank")
        } else {
            ("DELETE", "yank")
        };
        let path = format!("/api/v1/packages/{name}/{version}/{action}");
        self.send(method, &path, Some(token), &[])?.ok().map(|_| ())
    }

    /// The user a token belongs to.
    pub fn me(&self, token: &str) -> Result<String, String> {
        #[derive(Deserialize)]
        struct Me {
            user: String,
        }
        let body = self.send("GET", "/api/v1/me", Some(token), &[])?.ok()?;
        serde_json::from_slice::<Me>(&body)
            .map(|me| me.user)
            .map_err(|e| format!("the registry sent an invalid answer: {e}"))
    }

    fn get(&self, path: &str) -> Result<Response, String> {
        if self.offline {
            return Err("it is not fetched yet: run `brim check` to fetch dependencies".into());
        }
        self.send("GET", path, None, &[])
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: &[u8],
    ) -> Result<Response, String> {
        if let Some(dir) = self.url.strip_prefix("file://") {
            if method != "GET" {
                return Err("a `file://` registry is read-only".into());
            }
            let file = Path::new(dir).join(path.trim_start_matches('/'));
            return Ok(match std::fs::read(&file) {
                Ok(body) => Response { status: 200, body },
                Err(_) => Response {
                    status: 404,
                    body: br#"{"error": "not found"}"#.to_vec(),
                },
            });
        }
        if token.is_some() && insecure(&self.url) {
            return Err(format!(
                "not sending an API token to {} over plain http: use https",
                self.url
            ));
        }
        http::send(method, &format!("{}{path}", self.url), token, body)
    }
}

struct Response {
    status: u32,
    body: Vec<u8>,
}

impl Response {
    /// The body of a successful response, or the registry's error.
    fn ok(self) -> Result<Vec<u8>, String> {
        if (200..300).contains(&self.status) {
            return Ok(self.body);
        }
        let message = serde_json::from_slice::<ErrorBody>(&self.body)
            .map(|e| e.error)
            .unwrap_or_else(|_| format!("the registry answered with status {}", self.status));
        Err(message)
    }
}

#[cfg(not(target_family = "wasm"))]
mod http {
    use super::Response;
    use std::time::Duration;

    pub(super) fn send(
        method: &str,
        url: &str,
        token: Option<&str>,
        body: &[u8],
    ) -> Result<Response, String> {
        let run = || -> Result<Response, curl::Error> {
            let mut easy = curl::easy::Easy::new();
            easy.url(url)?;
            easy.custom_request(method)?;
            easy.connect_timeout(Duration::from_secs(15))?;
            easy.timeout(Duration::from_secs(120))?;
            easy.useragent(concat!("brim/", env!("CARGO_PKG_VERSION")))?;
            let mut headers = curl::easy::List::new();
            if let Some(token) = token {
                headers.append(&format!("authorization: Bearer {token}"))?;
            }
            if !body.is_empty() {
                headers.append("content-type: application/octet-stream")?;
                easy.post_fields_copy(body)?;
            }
            easy.http_headers(headers)?;
            let mut out = Vec::new();
            {
                let mut transfer = easy.transfer();
                transfer.write_function(|data| {
                    // A larger answer stops the transfer: no registry sends one.
                    if out.len() + data.len() > super::MAX_RESPONSE {
                        return Ok(0);
                    }
                    out.extend_from_slice(data);
                    Ok(data.len())
                })?;
                transfer.perform()?;
            }
            Ok(Response {
                status: easy.response_code()?,
                body: out,
            })
        };
        run().map_err(|e| format!("cannot reach the registry at {url}: {}", e.description()))
    }
}

#[cfg(target_family = "wasm")]
mod http {
    use super::Response;

    pub(super) fn send(_: &str, url: &str, _: Option<&str>, _: &[u8]) -> Result<Response, String> {
        Err(format!("cannot reach the registry at {url} here"))
    }
}

/// Lowercase hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpacks a `.tar.gz` whose entries are all under `prefix/` into `dest`.
/// Refuses links, other special entries and paths that leave `prefix/`.
pub fn unpack(archive: &[u8], prefix: &str, dest: &Path) -> Result<(), String> {
    let bad = |why: &str| format!("the archive is invalid: {why}");
    let gz = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(gz.take(256 * 1024 * 1024));
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let entries = tar.entries().map_err(|e| bad(&e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| bad(&e.to_string()))?;
        let path = entry.path().map_err(|e| bad(&e.to_string()))?.into_owned();
        let mut components = path.components();
        if components.next() != Some(Component::Normal(prefix.as_ref())) {
            return Err(bad(&format!("`{}` is outside `{prefix}/`", path.display())));
        }
        let rest: PathBuf = components.clone().collect();
        if components.any(|c| !matches!(c, Component::Normal(_))) {
            return Err(bad(&format!("`{}` is not a plain path", path.display())));
        }
        let target = dest.join(&rest);
        match entry.header().entry_type() {
            tar::EntryType::Directory => {
                std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            }
            tar::EntryType::Regular => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let mut data = Vec::new();
                entry
                    .read_to_end(&mut data)
                    .map_err(|e| bad(&e.to_string()))?;
                std::fs::write(&target, data).map_err(|e| e.to_string())?;
            }
            other => {
                return Err(bad(&format!(
                    "`{}` is not a file or directory ({other:?})",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

/// A reproducible `.tar.gz` of `files` (paths relative to `root`, with `/`)
/// under `prefix/`, with `extra` files given by content (e.g. the
/// published manifest).
pub fn pack(
    root: &Path,
    files: &[String],
    extra: &[(String, Vec<u8>)],
    prefix: &str,
) -> Result<Vec<u8>, String> {
    let mut contents: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for file in files {
        let data =
            std::fs::read(root.join(file)).map_err(|e| format!("cannot read {file}: {e}"))?;
        contents.insert(file.clone(), data);
    }
    for (file, data) in extra {
        contents.insert(file.clone(), data.clone());
    }
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    for (file, data) in &contents {
        let mut header = tar::Header::new_ustar();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Regular);
        tar.append_data(&mut header, format!("{prefix}/{file}"), data.as_slice())
            .map_err(|e| format!("cannot pack {file}: {e}"))?;
    }
    let gz = tar.into_inner().map_err(|e| e.to_string())?;
    gz.finish().map_err(|e| e.to_string())
}

/// The file next to an unpacked version that records its archive's checksum.
fn checksum_file(dir: &Path) -> PathBuf {
    dir.with_extension("sha256")
}

/// The checksum of the archive a cached version was unpacked from.
pub fn cached_checksum(dir: &Path) -> Option<String> {
    std::fs::read_to_string(checksum_file(dir))
        .ok()
        .map(|s| s.trim().to_string())
}
