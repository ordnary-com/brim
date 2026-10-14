#!/bin/sh
# Checks the editor packages in the editors themselves. Expects `brim`,
# `nvim` (0.10 or newer), `hx`, `kak`, `nano`, `micro`, a C compiler, and
# KSyntaxHighlighting's `kate-syntax-highlighter` on the PATH (CI installs
# them). Each check prints "<editor>: ok".
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
example="$root/examples/stack.brim"
BRIM=$(command -v brim)
export BRIM

# A program with a type error, for the language server.
mkdir -p "$tmp/broken"
printf 'fn main() {\n    let x: i64 = "text"\n}\n' > "$tmp/broken/main.brim"

# ---- Neovim: filetype, tree-sitter highlighting, language server --------
mkdir -p "$tmp/nvim/parser"
cc -shared -fPIC -O2 -I "$root/editors/tree-sitter-brim/src" \
    "$root/editors/tree-sitter-brim/src/parser.c" -o "$tmp/nvim/parser/brim.so"
BRIM_TEST_FILE="$example" BRIM_TEST_BROKEN="$tmp/broken/main.brim" \
    nvim --headless --clean \
    --cmd "set rtp^=$root/editors/neovim" --cmd "set rtp+=$tmp/nvim" \
    -c "luafile $root/editors/test/neovim.lua"

# ---- Helix: grammar build, queries and language server ------------------
mkdir -p "$tmp/helix/helix/runtime"
cp "$root/editors/helix/languages.toml" "$tmp/helix/helix/languages.toml"
cp -R "$root/editors/helix/runtime/queries" "$tmp/helix/helix/runtime/"
export XDG_CONFIG_HOME="$tmp/helix"
hx --grammar fetch > /dev/null
hx --grammar build > /dev/null
health=$(hx --health brim)
echo "$health" | grep -q "Highlight queries: ✓" || { echo "$health"; echo "helix: highlight queries missing" >&2; exit 1; }
echo "$health" | grep -q "brim: $BRIM" || echo "$health" | grep -q "Binary for language server: ✓" \
    || { echo "$health"; echo "helix: language server not found" >&2; exit 1; }
hx --health brim | grep -q "✘" && { hx --health brim; echo "helix: a health check failed" >&2; exit 1; }
unset XDG_CONFIG_HOME
echo "helix: ok"

# ---- Kakoune: the script loads, the highlighters compile ----------------
kak -n -ui dummy -e "
    try %{
        source $root/editors/kakoune/brim.kak
        edit $example
        nop %sh{ [ \"\$kak_opt_filetype\" = brim ] || echo 'filetype' > $tmp/kak-error }
        add-highlighter window/test ref brim
    } catch %{
        nop %sh{ echo \"\$kak_error\" > $tmp/kak-error }
    }
    quit!
"
if [ -s "$tmp/kak-error" ]; then echo "kakoune: $(cat "$tmp/kak-error")" >&2; exit 1; fi
echo "kakoune: ok"

# ---- nano: the syntax loads without complaints --------------------------
# nano shows rc file errors on its screen; run it briefly in a pseudo-terminal.
printf 'include "%s/editors/nano/brim.nanorc"\n' "$root" > "$tmp/nanorc"
screen=$(TERM=xterm timeout 3 script -qec "nano --rcfile=$tmp/nanorc $example" /dev/null 2>&1 || true)
if echo "$screen" | grep -qi "error in\|bad regex\|unknown"; then
    echo "$screen" | grep -i "error in\|bad regex\|unknown" >&2
    echo "nano: the syntax file has errors" >&2
    exit 1
fi
echo "nano: ok"

# ---- micro: the syntax file loads -----------------------------------------
# micro reports a broken syntax file on its screen; run it briefly in a
# pseudo-terminal with the file open.
mkdir -p "$tmp/micro/syntax"
cp "$root/editors/micro/brim.yaml" "$tmp/micro/syntax/"
screen=$(TERM=xterm timeout 3 script -qec "micro -config-dir $tmp/micro $example" /dev/null 2>&1 || true)
if echo "$screen" | grep -qi "error"; then
    echo "$screen" | grep -i "error" >&2
    echo "micro: the syntax file has errors" >&2
    exit 1
fi
echo "micro: ok"

# ---- Kate / KSyntaxHighlighting: Brim is known and highlights -----------
mkdir -p "$tmp/kate/org.kde.syntax-highlighting/syntax"
cp "$root/editors/kate/brim.xml" "$tmp/kate/org.kde.syntax-highlighting/syntax/"
html=$(XDG_DATA_HOME="$tmp/kate" kate-syntax-highlighter --syntax Brim --output-format html "$example" 2>&1)
echo "$html" | grep -q "<html" || { echo "$html" | head -5; echo "kate: no output" >&2; exit 1; }
echo "$html" | grep -q "struct" || { echo "kate: the output lacks the code" >&2; exit 1; }
[ "$(echo "$html" | grep -c 'style="color')" -gt 10 ] || { echo "kate: nothing is highlighted" >&2; exit 1; }
echo "kate: ok"
