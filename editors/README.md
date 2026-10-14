# Editor support

- `vscode/`: a Visual Studio Code extension with syntax highlighting and
  the language server.
- `vim/`: syntax highlighting for Vim and Neovim. Copy `ftdetect/` and
  `syntax/` into `~/.vim` (or `~/.config/nvim`).

Other editors can use the TextMate grammar in
`vscode/syntaxes/brim.tmLanguage.json` and start the language server with
`brim lsp`, which speaks the Language Server Protocol over standard input
and output. For Neovim:

```lua
vim.lsp.start({ name = "brim", cmd = { "brim", "lsp" }, root_dir = vim.fs.root(0, "brim.toml") })
```
