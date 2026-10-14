# Brim for JetBrains IDEs

IntelliJ IDEA, RustRover, WebStorm, PyCharm, GoLand, CLion and the other
JetBrains IDEs can use the TextMate grammar of the VS Code extension and
the language server.

**Highlighting.** Open **Settings > Editor > TextMate Bundles**, click
**+** and choose the `editors/vscode` directory of a Brim checkout.

**Language server.** Install the [LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij)
plugin. Under **Settings > Languages & Frameworks > Language Servers**, add
a server:

- Name: `Brim`
- Command: `brim lsp`
- Mappings > File name patterns: `*.brim`, language id `brim`

You then get diagnostics, formatting (**Code > Reformat Code**), the
structure view, go to declaration and quick documentation.
