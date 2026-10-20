; Brim highlighting. Capture names follow the common tree-sitter
; conventions (Neovim, Helix, Zed).

(comment) @comment

(integer) @number
(float) @number.float
(string) @string
(fstring) @string
(interpolation ["{" "}"] @punctuation.special)
(escape_sequence) @string.escape
(boolean) @boolean
(self) @variable.builtin

(primitive_type) @type.builtin
(type_identifier) @type
(path_type (identifier) @type)
((path_type (identifier) @type.builtin)
  (#any-of? @type.builtin "Option" "Result" "List" "Map" "Set" "Box"))
(type_parameter name: (identifier) @type)
(type_parameter bound: (path_type (identifier) @type))
(trait_declaration name: (type_identifier) @type)
(trait_method name: (identifier) @function)
(scoped_type_identifier (identifier) @module)

(function_declaration name: (identifier) @function)
(extern_function_declaration name: (identifier) @function)
(call_expression function: (identifier) @function.call)
(value_call_expression function: (identifier) @function.call)
((call_expression function: (identifier) @function.builtin)
  (#any-of? @function.builtin
    "print" "eprint" "to_string" "to_float" "to_int" "to_i32" "to_u8" "to_u64" "string_from_bytes" "from_code_point" "serve_api" "abs" "sqrt" "floor" "ceil" "round" "pow" "min" "max" "clamp" "read_line" "read_file" "write_file" "args" "env" "exit"
    "assert" "assert_eq"))
((call_expression function: (identifier) @constructor)
  (#any-of? @constructor "Some" "Ok" "Err"))
((identifier) @constant.builtin
  (#eq? @constant.builtin "None"))
(method_call_expression method: (identifier) @function.method.call)

(parameter name: (identifier) @variable.parameter)
(field_declaration name: (field_identifier) @property)
(field_initializer name: (field_identifier) @property)
(field_expression field: (field_identifier) @property)
(variant name: (identifier) @constructor)
; In patterns, capitalized names are variants and lowercase names bind the value.
((variant_pattern name: (identifier) @constructor)
  (#match? @constructor "^[A-Z]"))
(module_path) @module
(use_path) @module

[
  "fn" "struct" "enum" "impl" "trait" "const" "let"
] @keyword

[
  "if" "else" "match"
] @keyword.conditional

[
  "while" "loop" "for" "in"
] @keyword.repeat

(break_statement) @keyword.repeat
(continue_statement) @keyword.repeat
"return" @keyword.return

[
  "use" "import" "as"
] @keyword.import

(visibility) @keyword.modifier
"mut" @keyword.modifier
"extern" @keyword.modifier

[
  "=" "+=" "-=" "*=" "/=" "%=" "+" "-" "*" "/" "%" "==" "!=" "<" "<=" ">" ">="
  "&&" "||" "|" "!" "?" ".." "->" "=>"
] @operator

[ "(" ")" "[" "]" "{" "}" ] @punctuation.bracket
[ "," ";" ":" "." ] @punctuation.delimiter
(const_declaration name: (identifier) @constant)
