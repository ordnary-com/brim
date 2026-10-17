# Tools and editors

| Command | What it does |
|---------|--------------|
| `brim run [PATH] [ARGS...]` | Check a program and run it with the interpreter |
| `brim build [PATH]` | Compile a native executable (`--release` to optimise, `-o` for the path) |
| `brim check [PATH]` | Report errors without running |
| `brim test [PATH] [FILTER]` | Run the tests |
| `brim fmt [PATH...]` | Format source files (`--check` to only report) |
| `brim new PATH` | Create a project |
| `brim update [PATH]` | Update git dependencies in `.brimlock` |
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
whole mapping. `json.encode` and `json.decode` use the same shapes.

### An Angular client

A module whose `pub fn`s are the API is served with the built-in
`serve_api(module, request)`: `POST /api/add_note` with `{"text": "milk"}`
calls `add_note(text: "milk")`, and the result is the JSON response.

```brim
// notes.brim
pub fn add_note(text: string) -> Result[Note, NoteError] {
    ...
}

// main.brim
let response = serve_api(notes, request)
request.respond(response)?
```

`brim gen ts --angular -o client.ts` then writes, next to the types, an
Angular service with a method per function:

```typescript
@Injectable({ providedIn: 'root' })
export class NotesClient {
  addNote(text: string): Observable<Note> { ... }
}
```

A function that returns `Err(error)` answers with status 422, and the
`HttpErrorResponse` holds the error. Provide `API_BASE_URL` when the server
is on another origin. `examples/http/api` is a complete service with its
generated client.

## Documentation: brim doc

`brim doc` writes HTML documentation for a program's own modules: a page
per module with every `pub` struct, enum, trait, constant, function and
method, its signature and the `//` comment right above it, and an index
with a search box. The comment at the top of a file documents the module.
In comments, `code` is code, and lines indented by two or more spaces are
code examples:

```brim
// Notes, kept in a file.
//
//   let note = notes.add_note("milk")?

// Adds a note and returns it with its `id`.
pub fn add_note(text: string) -> Result[Note, NoteError] {
    ...
}
```

A project's documentation goes to `target/doc` (`-o DIR` picks another
place); `brim doc --std` documents the standard library.

## Build cache

`brim build` keeps what it can reuse in `~/.brim/cache` (or
`$BRIM_HOME/cache`): the compiled runtime, and for each executable what it
was built from. A build whose source files, options, C compiler and `brim`
are all unchanged keeps the executable and says `(up to date)`. Set
`BRIM_NO_CACHE=1` to build everything again; deleting the directory is
always safe.

## Debugging

A program built without `--release` carries debug information (DWARF), so
lldb and gdb show Brim source lines, function names and variables:

```text
$ brim build app.brim -o app
$ lldb ./app
(lldb) breakpoint set --file app.brim --line 12
(lldb) run
(lldb) bt
  * frame #0: app`brim.area at app.brim:12:12
    frame #1: app`brim.main at app.brim:20:17
(lldb) frame variable
(long) width = 3
(long) height = 4
```

Variables of the types `i64`, `f64` and `bool` can be printed; strings,
lists and other values not yet. On macOS the debug information is in
`app.dSYM` next to the executable.

## Editors

`brim lsp` is a language server: it shows errors as you type, completes
names (after `value.` the fields and methods of the value's type), goes to
definitions, finds references, renames across files, shows signatures
while you type a call, offers quick fixes for errors that say what to
write, formats, lists a file's declarations and shows types on hover. The
Brim repository has:

support for VS Code, Neovim, Vim, Helix, Zed, Emacs, Sublime Text,
JetBrains IDEs, Kate, Kakoune, nano, micro and Notepad++ in `editors/`,
each with its own instructions. Other editors can use the TextMate grammar
(`editors/vscode/syntaxes/brim.tmLanguage.json`) or the tree-sitter grammar
(`editors/tree-sitter-brim`), and start `brim lsp` from any LSP client.
