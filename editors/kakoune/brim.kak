# Brim support for Kakoune: copy to ~/.config/kak/autoload/brim.kak

hook global BufCreate .*\.brim %{
    set-option buffer filetype brim
}

hook global WinSetOption filetype=brim %{
    require-module brim
    add-highlighter window/brim ref brim
    set-option window comment_line '//'
    set-option window indentwidth 4
    hook -once -always window WinSetOption filetype=.* %{ remove-highlighter window/brim }
}

provide-module brim %§
    add-highlighter shared/brim regions
    add-highlighter shared/brim/code default-region group
    add-highlighter shared/brim/string region '"' (?<!\\)(\\\\)*" fill string
    add-highlighter shared/brim/comment region '//' '$' fill comment

    add-highlighter shared/brim/code/ regex \b(fn|let|mut|return|if|else|while|for|in|match|break|continue|struct|enum|impl|pub|import|as)\b 0:keyword
    add-highlighter shared/brim/code/ regex \b(async|await|const|loop|trait|type|use)\b 0:Error
    add-highlighter shared/brim/code/ regex \b(i64|i32|u8|u64|f64|bool|string|void|Option|Result|List|Map|Box)\b 0:type
    add-highlighter shared/brim/code/ regex \b[A-Z][A-Za-z0-9_]*\b 0:type
    add-highlighter shared/brim/code/ regex \b(true|false|self|Some|None|Ok|Err)\b 0:value
    add-highlighter shared/brim/code/ regex \b(print|eprint|to_string|to_float|to_int|to_i32|to_u8|to_u64|string_from_bytes|abs|sqrt|floor|ceil|round|pow|min|max|clamp|read_line|read_file|write_file|args|env|exit|assert|assert_eq)\b 0:builtin
    add-highlighter shared/brim/code/ regex \b[0-9][0-9_]*(\.[0-9][0-9_]*)?([eE][+-]?[0-9]+)?\b 0:value
    add-highlighter shared/brim/code/ regex \bfn\s+([A-Za-z_][A-Za-z0-9_]*) 1:function
§

# The language server, with kakoune-lsp: add to its kak-lsp.toml
#
#   [language_server.brim]
#   filetypes = ["brim"]
#   roots = ["brim.toml", ".git"]
#   command = "brim"
#   args = ["lsp"]
