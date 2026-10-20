# Brim for Visual Studio Code

Syntax highlighting and a file icon for `.brim` files and the project file
(`.brimproj`, or the older `brim.toml`), and the Brim language server
(`brim lsp`): diagnostics as you type, formatting, an outline, go to
definition and hover.

## Install from source

The extension needs `brim` on your `PATH` (or set `brim.path`).

```sh
cd editors/vscode
npm install
npx @vscode/vsce package
code --install-extension brim-0.1.0.vsix
```

The file icons (the Brim mark, and the mark with a gear for `.brimproj`)
appear in file icon themes without their own icon for Brim,
such as the default theme (Seti).

Format on save works with the usual setting:

```json
"[brim]": { "editor.formatOnSave": true }
```
