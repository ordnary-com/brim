-- Run by editors/test/run.sh: opens a Brim file in headless Neovim with
-- editors/neovim on the runtimepath and checks the filetype, tree-sitter
-- highlighting and the language server.
local function fail(msg)
  io.stderr:write("neovim: " .. msg .. "\n")
  vim.cmd("cquit 1")
end

local file = assert(os.getenv("BRIM_TEST_FILE"))
local broken = assert(os.getenv("BRIM_TEST_BROKEN"))
require("brim").setup({ cmd = { assert(os.getenv("BRIM")), "lsp" } })

vim.cmd("edit " .. file)
if vim.bo.filetype ~= "brim" then fail("filetype is '" .. vim.bo.filetype .. "'") end
if vim.bo.commentstring ~= "// %s" then fail("commentstring is '" .. vim.bo.commentstring .. "'") end

local ok, parser = pcall(vim.treesitter.get_parser, 0, "brim")
if not ok then fail("no tree-sitter parser: " .. tostring(parser)) end
local root = parser:parse()[1]:root()
if root:has_error() then fail("the parse tree has errors") end
local query = vim.treesitter.query.get("brim", "highlights")
if not query then fail("no highlights query") end
local captures = {}
for id in query:iter_captures(root, 0) do captures[query.captures[id]] = true end
for _, name in ipairs({ "keyword", "function", "type", "string", "comment" }) do
  if not captures[name] then fail("no @" .. name .. " capture") end
end
if not vim.treesitter.highlighter.active[vim.api.nvim_get_current_buf()] then
  fail("tree-sitter highlighting is not active")
end

-- The language server reports the type error in the broken file.
vim.cmd("edit " .. broken)
local attached = vim.wait(10000, function() return #vim.lsp.get_clients({ bufnr = 0 }) > 0 end)
if not attached then fail("the language server did not attach") end
local diagnosed = vim.wait(10000, function() return #vim.diagnostic.get(0) > 0 end)
if not diagnosed then fail("no diagnostics from the language server") end
local d = vim.diagnostic.get(0)[1]
if d.code ~= "E0206" then fail("unexpected diagnostic " .. vim.inspect(d)) end

-- Formatting through the language server.
vim.api.nvim_buf_set_lines(0, 0, -1, false, { "fn main(){", "print( 1 )", "}" })
vim.lsp.buf.format({ timeout_ms = 5000 })
local text = table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), "\n")
if text ~= "fn main() {\n    print(1)\n}" then fail("formatting gave:\n" .. text) end

print("neovim: ok")
vim.cmd("qall!")
