# Brim for Sublime Text

Copy this directory to your Packages folder as `Brim` (**Preferences >
Browse Packages...**). `.brim` files then get highlighting and `//`
comments. The same `Brim.sublime-syntax` works with
[bat](https://github.com/sharkdp/bat): copy it to `$(bat --config-dir)/syntaxes`
and run `bat cache --build`.

For the language server, install the [LSP](https://packagecontrol.io/packages/LSP)
package and add to its settings (**Preferences > Package Settings > LSP >
Settings**):

```json
{
  "clients": {
    "brim": {
      "enabled": true,
      "command": ["brim", "lsp"],
      "selector": "source.brim"
    }
  }
}
```
