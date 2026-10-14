# Brim for Helix

1. Append `languages.toml` to `~/.config/helix/languages.toml`.
2. Copy `runtime/queries/brim` to `~/.config/helix/runtime/queries/brim`.
3. Build the grammar: `hx --grammar fetch && hx --grammar build`.

Helix then highlights `.brim` files, starts `brim lsp` (which must be on
your `PATH`) and formats on save. `hx --health brim` checks the setup.
