# Tools and editors

| Command | What it does |
|---------|--------------|
| `brim run [PATH] [ARGS...]` | Check a program and run it with the interpreter |
| `brim build [PATH]` | Compile a native executable (`--release` to optimise, `-o` for the path) |
| `brim check [PATH]` | Report errors without running |
| `brim test [PATH] [FILTER]` | Run the tests |
| `brim fmt [PATH...]` | Format source files (`--check` to only report) |
| `brim new PATH` | Create a project |
| `brim update [PATH]` | Update git dependencies in `brim.lock` |
| `brim gen ts [PATH]` | Write TypeScript types for the program's API |
| `brim lsp` | Run the language server |

`brim help COMMAND` explains each command.

## Formatting

`brim fmt` sets the indentation (four spaces), the spaces between tokens
and the blank lines, and keeps comments and line breaks. There are no
options: every Brim program looks the same.

## API types for TypeScript

A backend written in Brim can share its API types with a TypeScript
frontend. `brim gen ts` writes a declaration for every `pub` struct and
enum:

```brim
pub struct Task {
    pub id: i64,
    pub title: string,
    pub assignee: Option[string],
}

pub enum Priority {
    Low,
    High,
}
```

becomes

```typescript
export interface Task {
  id: number;
  title: string;
  assignee: string | null;
}

export type Priority = "Low" | "High";
```

Section 9.4 of the [specification](docs/language-spec.md) has the
whole mapping.

## Editors

`brim lsp` is a language server: it shows errors as you type, formats,
lists a file's declarations, and offers go to definition and hover. The
Brim repository has:

support for VS Code, Neovim, Vim, Helix, Zed, Emacs, Sublime Text,
JetBrains IDEs, Kate, Kakoune, nano, micro and Notepad++ in `editors/`,
each with its own instructions. Other editors can use the TextMate grammar
(`editors/vscode/syntaxes/brim.tmLanguage.json`) or the tree-sitter grammar
(`editors/tree-sitter-brim`), and start `brim lsp` from any LSP client.
