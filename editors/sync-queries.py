#!/usr/bin/env python3
"""Copies the tree-sitter highlight queries to the editor packages.

tree-sitter-brim/queries/highlights.scm is the source. Neovim and Zed use
it as it is; Helix names some captures differently. Run this after
changing the queries; a test checks that the copies are up to date.
"""

from pathlib import Path

HERE = Path(__file__).resolve().parent
SOURCE = HERE / "tree-sitter-brim" / "queries" / "highlights.scm"

# Capture names in Helix where they differ (longest names first).
HELIX = [
    ("@function.method.call", "@function.method"),
    ("@function.call", "@function"),
    ("@keyword.conditional", "@keyword.control.conditional"),
    ("@keyword.repeat", "@keyword.control.repeat"),
    ("@keyword.return", "@keyword.control.return"),
    ("@keyword.import", "@keyword.control.import"),
    ("@keyword.modifier", "@keyword.storage.modifier"),
    ("@variable.parameter", "@variable.parameter"),
    ("@string.escape", "@constant.character.escape"),
    ("@property", "@variable.other.member"),
    ("@number.float", "@constant.numeric.float"),
    ("@number", "@constant.numeric.integer"),
    ("@boolean", "@constant.builtin.boolean"),
    ("@module", "@namespace"),
]

# Capture names in Zed's themes where they differ.
ZED = [
    ("@function.method.call", "@function.method"),
    ("@function.call", "@function"),
    ("@function.builtin", "@function"),
    ("@variable.builtin", "@variable.special"),
    ("@variable.parameter", "@variable"),
    ("@module", "@type"),
    ("@number.float", "@number"),
]

COPIES = {
    "neovim/queries/brim/highlights.scm": None,
    "zed/languages/brim/highlights.scm": ZED,
    "helix/runtime/queries/brim/highlights.scm": HELIX,
}


def render(text, mapping):
    if mapping is None:
        return text
    out = []
    for line in text.splitlines(keepends=True):
        for old, new in mapping:
            line = line.replace(old + " ", new + " ").replace(old + ")", new + ")")
            if line.rstrip().endswith(old):
                line = line.rstrip()[: -len(old)] + new + "\n"
        out.append(line)
    return "".join(out)


def main():
    text = SOURCE.read_text()
    for path, mapping in COPIES.items():
        target = HERE / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(render(text, mapping))
        print("wrote", path)


if __name__ == "__main__":
    main()
