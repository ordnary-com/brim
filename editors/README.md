# Editor support

Every editor gets highlighting, and the editors that support the Language
Server Protocol also get the language server `brim lsp`: diagnostics as
you type, formatting, an outline, go to definition and hover. `brim` must
be on the `PATH`.

| Editor | Directory | Highlighting | Language server |
|--------|-----------|--------------|-----------------|
| Visual Studio Code (and VSCodium, Cursor, Windsurf) | `vscode/` | TextMate grammar, file icon | built in |
| Neovim | `neovim/` | tree-sitter | built in (`vim.lsp`) |
| Vim | `vim/` | syntax file | vim-lsp or coc.nvim |
| Helix | `helix/` | tree-sitter | built in |
| Zed | `zed/` | tree-sitter, outline | built in |
| Emacs | `emacs/` | `brim-mode` | Eglot or lsp-mode |
| Sublime Text (and bat) | `sublime/` | sublime-syntax | LSP package |
| JetBrains IDEs | `jetbrains/` | TextMate bundle from `vscode/` | LSP4IJ |
| Kate, KWrite, KDevelop | `kate/` | KSyntaxHighlighting | built in |
| Kakoune | `kakoune/` | `brim.kak` | kakoune-lsp |
| nano | `nano/` | `brim.nanorc` | |
| micro | `micro/` | `brim.yaml` | |
| Notepad++ | `notepad++/` | user defined language | |

Each directory has its own instructions. Other editors can use one of these
grammars: the TextMate grammar (`vscode/syntaxes/brim.tmLanguage.json`)
is understood by many, the tree-sitter grammar (`tree-sitter-brim/`) by
the newer ones, and any LSP client can start `brim lsp`, which talks over
standard input and output.

A test keeps every grammar in step with the language's keywords and
builtins. The tree-sitter highlight queries are copied to the Neovim,
Helix and Zed packages by `sync-queries.py`.
