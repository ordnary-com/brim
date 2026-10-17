# Brim for Vim

Copy `ftdetect/` and `syntax/` to `~/.vim` (or add this directory to your
`runtimepath`) for highlighting. Neovim users can use `editors/neovim`
instead, which adds tree-sitter and the language server.

For the language server `brim lsp` with
[vim-lsp](https://github.com/prabirshrestha/vim-lsp):

```vim
if executable('brim')
  au User lsp_setup call lsp#register_server({
    \ 'name': 'brim',
    \ 'cmd': ['brim', 'lsp'],
    \ 'allowlist': ['brim'],
    \ })
endif
```

With [coc.nvim](https://github.com/neoclide/coc.nvim), in `coc-settings.json`:

```json
{
  "languageserver": {
    "brim": {
      "command": "brim",
      "args": ["lsp"],
      "filetypes": ["brim"],
      "rootPatterns": [".brimproj", "brim.toml"]
    }
  }
}
```
