# Brim for Neovim

Highlighting with tree-sitter, and the language server `brim lsp`
(diagnostics, formatting, symbols, go to definition, hover).

With [lazy.nvim](https://github.com/folke/lazy.nvim):

```lua
{
  "ordnary-com/brim",
  config = function(plugin)
    vim.opt.rtp:append(plugin.dir .. "/editors/neovim")
    require("brim").setup()
  end,
}
```

Or add `editors/neovim` of a Brim checkout to your `runtimepath` and call
`require("brim").setup()`. Then run `:TSInstall brim` once (with
[nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter)).
`brim` must be on your `PATH`, or pass `setup({ cmd = { "/path/to/brim", "lsp" } })`.

Format on save:

```lua
vim.api.nvim_create_autocmd("BufWritePre", {
  pattern = "*.brim",
  callback = function() vim.lsp.buf.format() end,
})
```

Without tree-sitter, the Vim syntax file in `editors/vim` also works.
