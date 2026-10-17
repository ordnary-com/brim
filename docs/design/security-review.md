# Security review before 1.0

Status: done in October 2026, for 1.0.0-rc.1. It covered the native
runtime (`crates/brim-codegen/runtime`), the package client
(`crates/brim-package`, `brim publish`, `brim login`) and the registry
service (`registry/`). The interpreter's runtime (`crates/brim-runtime`)
was compared with the native one where they must agree.

## Native runtime

| Finding | Severity | Outcome |
|---------|----------|---------|
| HTTPS requests split headers with `strtok`, which is not thread-safe: two tasks could send each other's headers | high | Fixed: `strtok_r` |
| A socket used by one task could be closed and freed by another (use after free, a reused descriptor reading another client's data) | high | Fixed: every socket has a count of its users and a lock per operation; `close` shuts it down at once, which wakes a waiting task, and frees it when the last user is done. Closed sockets are freed instead of kept |
| Windows: `.bat` and `.cmd` files are run through `cmd.exe`, whose quoting differs (command injection); programs were looked up in the current directory first | high | Fixed: programs are looked up in `PATH` only, and arguments that `cmd.exe` would read as commands are refused for batch files |
| Child processes inherited sockets and pipes, also those of other tasks | medium | Fixed: sockets and pipes are close-on-exec (not inheritable on Windows), and processes start one at a time on Windows |
| A string with a NUL inside was cut at the NUL by C calls (`"key\0.txt"` opened `key`) | medium | Fixed: such strings name no file, variable or host |
| `remove_all` could follow a link swapped in while it ran, or a Windows junction, and remove outside the tree | medium | Fixed: removal works relative to directory descriptors without following links; junctions are removed themselves |
| Map keys from outside (JSON members, headers) could be chosen to collide in the hash | medium | Fixed: the hash has a random key per process |
| Arguments, file names, environment values and HTTPS headers could make strings that are not UTF-8 | low | Fixed: invalid bytes become U+FFFD (`env` gives `""`, as in the interpreter), and `char_code` checks its bounds |
| A client sending a byte every few seconds could hold `Server.next()` forever, since the timeout applied per read call to the system | medium | Fixed in both backends: `set_timeout` is the time one read may take in all |
| The runtime's ignored `SIGPIPE` was passed on to child processes | low | Fixed: children get the default |
| Windows listeners used `SO_REUSEADDR`, which lets another program take the port | low | Fixed: `SO_EXCLUSIVEADDRUSE` |
| `crypto.equal` had a branch on each byte | low | Fixed: it adds up differences without branching on them |
| `random_bytes` opens `/dev/urandom` each time and stops the program when no file can be opened | low | Open |
| `read_all` and HTTPS response bodies have no size limit | low | Open: programs set their own limits with `read_until_max` |

## Package client

| Finding | Severity | Outcome |
|---------|----------|---------|
| The checksum in `brim.lock` was not enforced: with an empty cache, a registry or mirror could serve other contents for a locked version and brim rewrote the lock file | high | Fixed: a locked version must have the locked checksum (`M0015`); only `brim update` records a new one. Cached versions carry the checksum they were unpacked from and are used only for that checksum |
| `brim publish` followed symbolic links, so a link in `src/` could publish files from anywhere | medium | Fixed: links are refused |
| The language server resolved dependencies when a file was opened: an untrusted repository could make the editor clone, download and write `brim.lock` | medium | Fixed: the language server resolves offline, from the lock file and the cache only |
| Registry responses had no size limit | low | Fixed: 32 MiB |
| Git URLs and references were not checked, and could be read as options | low | Fixed: URLs that start with `-` or use helper transports (`ext::`) are refused, `--` separates options from arguments, and git may only use `https`, `http`, `ssh`, `git` and `file` |
| A malformed commit in `brim.lock` could point a checkout anywhere | low | Fixed: commits must be 40 or 64 hex digits |
| `credentials.toml` was readable by others for a moment while it was written | low | Fixed: it is created with mode 600 |
| A token could be sent over plain `http://` | low | Fixed: tokens only go over https, or http to `localhost` |
| A lock file written for another registry is resolved again without a warning | low | Open |

## Registry service

| Finding | Severity | Outcome |
|---------|----------|---------|
| Package metadata had no size limits, so a publisher could make the home page and search slow | medium | Fixed: limits on description, license, repository and dependencies; `repository` must be an http(s) URL |
| Tokens could not be revoked, and a typo when issuing one gave someone's packages to another person | medium | Fixed: `DELETE /api/v1/admin/tokens`, `DELETE /api/v1/me/token`, and `"existing": true` to add a token for a user who has one |
| The manifest inside an archive was not compared with the metadata, so a package's real dependencies could differ from the index | low | Fixed: the archive's `brim.toml` must have the metadata's name, version, edition and dependencies |
| A failed or repeated upload could write over a stored archive | low | Fixed: an archive is never written twice, and a write that landed despite an error keeps the version |
| Names such as `std` and `brim` could be claimed | low | Fixed: reserved |
| Archive paths that Windows reads differently (`CON`, `a:b`, names differing in case) were accepted | low | Fixed: refused |
| No rate limits on publishing | low | Open: tokens are issued by an administrator |

Areas checked and found sound include archive unpacking on the client, the
download checksum, TLS settings (certificates checked, no redirects), the
manifest's C sources and link names, the registry's SQL, HTML escaping,
admin token comparison, checksums computed by the server, and the channel
and task code of the runtime.
