# Brim for Kate (and KWrite, KDevelop)

Copy `brim.xml` to `~/.local/share/org.kde.syntax-highlighting/syntax/`
(on Windows `%LOCALAPPDATA%\org.kde.syntax-highlighting\syntax\`).

For the language server, add to **Settings > Configure Kate > LSP Client >
User Server Settings**:

```json
{
  "servers": {
    "brim": {
      "command": ["brim", "lsp"],
      "rootIndicationFileNames": ["brim.toml"],
      "highlightingModeRegex": "^Brim$"
    }
  }
}
```
