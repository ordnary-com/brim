# tree-sitter-brim

A [tree-sitter](https://tree-sitter.github.io/) grammar for Brim, used for
highlighting and navigation in Neovim, Helix, Zed and other editors. The
generated parser (`src/`) is committed, so editors can build it directly.

The compiler's parser is the reference. This grammar accepts every valid
program, but treats newlines as whitespace, so it is more lenient about
where statements end.

```sh
npm install
npx tree-sitter generate   # after changing grammar.js
npx tree-sitter test       # the cases in test/corpus
```

CI checks that `src/` matches `grammar.js` and that every example and test
program parses without errors. `queries/highlights.scm` is the source of
the highlight queries; the copies for Neovim, Helix and Zed are generated
by `editors/sync-queries.py`.
