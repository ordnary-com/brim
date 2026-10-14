(function_declaration
  (visibility)? @context
  "fn" @context
  name: (identifier) @name) @item

(struct_declaration
  (visibility)? @context
  "struct" @context
  name: (type_identifier) @name) @item

(enum_declaration
  (visibility)? @context
  "enum" @context
  name: (type_identifier) @name) @item

(impl_declaration
  "impl" @context
  type: (type_identifier) @name) @item

(field_declaration name: (field_identifier) @name) @item

(variant name: (identifier) @name) @item
