//! Starts `brim lsp` for Brim files. `brim` must be on the `PATH`.

use zed_extension_api::{self as zed, LanguageServerId, Result};

struct BrimExtension;

impl zed::Extension for BrimExtension {
    fn new() -> Self {
        BrimExtension
    }

    fn language_server_command(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let command = worktree.which("brim").ok_or_else(|| {
            "`brim` was not found on the PATH; install it from \
             https://github.com/ordnary-com/brim/releases"
                .to_string()
        })?;
        Ok(zed::Command {
            command,
            args: vec!["lsp".into()],
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(BrimExtension);
