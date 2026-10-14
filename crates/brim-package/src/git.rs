//! Git dependencies: a bare clone per repository and a checkout per commit.
//!
//! ```text
//! $BRIM_HOME/git/db/<name>-<hash of URL>/                 bare clone
//! $BRIM_HOME/git/checkouts/<name>-<hash of URL>/<commit>/ working tree
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use brim_manifest::GitRef;

pub(crate) struct Checkout {
    pub dir: PathBuf,
    pub commit: String,
}

/// The checkout of `url` at `locked`, or at the commit `reference` names
/// now. Clones or fetches the repository when needed; `fetched` collects
/// the URLs that were.
pub(crate) fn checkout(
    home: &Path,
    name: &str,
    url: &str,
    reference: &GitRef,
    locked: Option<&str>,
    fetched: &mut Vec<String>,
) -> Result<Checkout, String> {
    let key = format!("{name}-{:016x}", fnv1a(url.as_bytes()));
    let db = home.join("git").join("db").join(&key);
    let checkouts = home.join("git").join("checkouts").join(&key);

    // A locked commit that is already checked out needs no git at all.
    if let Some(commit) = locked {
        let dir = checkouts.join(commit);
        if dir.is_dir() {
            return Ok(Checkout {
                dir,
                commit: commit.to_string(),
            });
        }
    }

    let mut up_to_date = false;
    if !db.is_dir() {
        std::fs::create_dir_all(db.parent().expect("db has a parent"))
            .map_err(|e| e.to_string())?;
        git(None, &["clone", "--bare", "--quiet", url, &path_arg(&db)])?;
        fetched.push(url.to_string());
        up_to_date = true;
    }
    let commit = match locked {
        Some(commit) => {
            if !has_commit(&db, commit) {
                fetch(&db, url, fetched)?;
                if !has_commit(&db, commit) {
                    return Err(format!(
                        "the locked commit {commit} is not in the repository; run `brim update`"
                    ));
                }
            }
            commit.to_string()
        }
        None => {
            if !up_to_date {
                fetch(&db, url, fetched)?;
            }
            rev_parse(&db, reference)?
        }
    };

    let dir = checkouts.join(&commit);
    if !dir.is_dir() {
        std::fs::create_dir_all(&checkouts).map_err(|e| e.to_string())?;
        let tmp = checkouts.join(format!("{commit}.tmp"));
        let _ = std::fs::remove_dir_all(&tmp);
        git(
            None,
            &[
                "clone",
                "--quiet",
                "--no-checkout",
                &path_arg(&db),
                &path_arg(&tmp),
            ],
        )?;
        git(Some(&tmp), &["checkout", "--quiet", "--detach", &commit])?;
        std::fs::rename(&tmp, &dir).map_err(|e| e.to_string())?;
    }
    Ok(Checkout { dir, commit })
}

fn fetch(db: &Path, url: &str, fetched: &mut Vec<String>) -> Result<(), String> {
    git(
        Some(db),
        &[
            "fetch",
            "--quiet",
            "--force",
            url,
            "+refs/heads/*:refs/heads/*",
            "+refs/tags/*:refs/tags/*",
        ],
    )?;
    fetched.push(url.to_string());
    Ok(())
}

fn has_commit(db: &Path, commit: &str) -> bool {
    git(
        Some(db),
        &["cat-file", "-e", &format!("{commit}^{{commit}}")],
    )
    .is_ok()
}

fn rev_parse(db: &Path, reference: &GitRef) -> Result<String, String> {
    let rev = match reference {
        GitRef::DefaultBranch => "HEAD".to_string(),
        GitRef::Branch(b) => format!("refs/heads/{b}"),
        GitRef::Tag(t) => format!("refs/tags/{t}"),
        GitRef::Rev(r) => r.clone(),
    };
    git(
        Some(db),
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ],
    )
    .map(|out| out.trim().to_string())
    .map_err(|_| format!("the repository has no {reference}"))
}

/// Runs git; returns its standard output, or a message with its error.
fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    command.args(args).env("GIT_TERMINAL_PROMPT", "0");
    let output = command
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        Err(if message.is_empty() {
            format!("`git {}` failed", args[0])
        } else {
            format!("git: {message}")
        })
    }
}

fn path_arg(path: &Path) -> String {
    path.display().to_string()
}

/// FNV-1a: a stable hash for cache directory names.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
