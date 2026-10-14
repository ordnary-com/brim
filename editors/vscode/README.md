# Brim for Visual Studio Code

Syntax highlighting for `.brim` files, and the Brim language server
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

Format on save works with the usual setting:

```json
"[brim]": { "editor.formatOnSave": true }
```
