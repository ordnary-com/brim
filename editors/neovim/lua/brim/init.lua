-- Brim support for Neovim: the tree-sitter parser (with nvim-treesitter)
-- and the language server `brim lsp`.
--
--   require("brim").setup({ cmd = { "brim", "lsp" } })

local M = {}

local REPO = "https://github.com/ordnary-com/brim"

function M.setup(opts)
  opts = opts or {}
  local cmd = opts.cmd or { "brim", "lsp" }

  -- The parser; `:TSInstall brim` builds it from the Brim repository.
  local ok, parsers = pcall(require, "nvim-treesitter.parsers")
  if ok then
    local configs = parsers.get_parser_configs and parsers.get_parser_configs() or parsers
    configs.brim = {
      install_info = {
        url = REPO,
        location = "editors/tree-sitter-brim",
        files = { "src/parser.c" },
        branch = "main",
      },
      filetype = "brim",
    }
  end
  vim.api.nvim_create_autocmd("FileType", {
    pattern = "brim",
    callback = function(args)
      pcall(vim.treesitter.start, args.buf, "brim")
    end,
  })

  -- The language server.
  if vim.lsp.config then
    vim.lsp.config("brim", {
      cmd = cmd,
      filetypes = { "brim" },
      root_markers = { ".brimproj", "brim.toml", ".git" },
    })
    vim.lsp.enable("brim")
  else
    vim.api.nvim_create_autocmd("FileType", {
      pattern = "brim",
      callback = function(args)
        vim.lsp.start({
          name = "brim",
          cmd = cmd,
          root_dir = vim.fs.root(args.buf, { ".brimproj", "brim.toml", ".git" }),
        })
      end,
    })
  end
end

return M
