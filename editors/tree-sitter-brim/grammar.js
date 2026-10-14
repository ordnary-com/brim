/**
 * Tree-sitter grammar for Brim, for editor highlighting and navigation
 * (Neovim, Helix, Zed and others). The compiler's own parser is the
 * reference: this grammar accepts every valid program, but treats newlines
 * as whitespace, so it is more lenient about where statements end.
 */

const PREC = {
  or: 1,
  and: 2,
  equality: 3,
  comparison: 4,
  additive: 5,
  multiplicative: 6,
  unary: 7,
  postfix: 8,
};

const typeId = ($) => alias($.identifier, $.type_identifier);
const fieldId = ($) => alias($.identifier, $.field_identifier);
const commaSep = (rule) => optional(seq(rule, repeat(seq(',', rule)), optional(',')));

module.exports = grammar({
  name: 'brim',

  extras: ($) => [/\s/, $.comment],

  word: ($) => $.identifier,

  conflicts: ($) => [
    [$._expression, $.struct_expression],
    [$.call_expression, $._expression],
    [$._expression, $.scoped_type_identifier],
  ],

  rules: {
    source_file: ($) => repeat(choice($.import_declaration, $._item, ';')),

    comment: (_) => token(seq('//', /.*/)),

    import_declaration: ($) =>
      seq(
        'import',
        field('path', $.module_path),
        optional(
          choice(
            seq('.', '{', commaSep($.identifier), '}'),
            seq('as', field('alias', $.identifier)),
          ),
        ),
      ),

    // One token, so that `a.b.{x}` needs no lookahead past the `.`.
    module_path: (_) => /[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)*/,

    _item: ($) =>
      choice($.function_declaration, $.struct_declaration, $.enum_declaration, $.impl_declaration),

    visibility: (_) => 'pub',

    function_declaration: ($) =>
      seq(
        optional($.visibility),
        'fn',
        field('name', $.identifier),
        optional($.type_parameters),
        $.parameters,
        optional(seq('->', field('return_type', $._type))),
        field('body', $.block),
      ),

    parameters: ($) => seq('(', commaSep(choice($.self_parameter, $.parameter)), ')'),

    self_parameter: ($) => seq(optional('mut'), $.self),

    parameter: ($) => seq(field('name', $.identifier), ':', field('type', $._type)),

    type_parameters: ($) => seq('[', commaSep($.identifier), ']'),

    struct_declaration: ($) =>
      seq(
        optional($.visibility),
        'struct',
        field('name', typeId($)),
        optional($.type_parameters),
        '{',
        repeat(seq($.field_declaration, optional(choice(',', ';')))),
        '}',
      ),

    field_declaration: ($) => seq(field('name', fieldId($)), ':', field('type', $._type)),

    enum_declaration: ($) =>
      seq(
        optional($.visibility),
        'enum',
        field('name', typeId($)),
        optional($.type_parameters),
        '{',
        repeat(seq($.variant, optional(choice(',', ';')))),
        '}',
      ),

    variant: ($) => seq(field('name', $.identifier), optional(seq('(', commaSep($._type), ')'))),

    impl_declaration: ($) =>
      seq(
        'impl',
        field('type', typeId($)),
        optional($.type_parameters),
        '{',
        repeat(choice($.function_declaration, ';')),
        '}',
      ),

    _type: ($) => choice($.primitive_type, $.generic_type, $.path_type),

    primitive_type: (_) => choice('i64', 'bool', 'string', 'void'),

    path_type: ($) => prec.right(seq($.identifier, repeat(seq('.', $.identifier)))),

    generic_type: ($) => seq(field('name', $.path_type), '[', commaSep($._type), ']'),

    block: ($) => seq('{', repeat(choice($._statement, ';')), '}'),

    _statement: ($) =>
      choice(
        $.let_statement,
        $.assignment_statement,
        $.if_statement,
        $.while_statement,
        $.for_statement,
        $.match_statement,
        $.break_statement,
        $.continue_statement,
        $.return_statement,
        $.block,
        $.expression_statement,
      ),

    let_statement: ($) =>
      seq(
        'let',
        optional('mut'),
        field('name', $.identifier),
        ':',
        field('type', $._type),
        '=',
        field('value', $._expression),
      ),

    assignment_statement: ($) =>
      seq(
        field('target', $._place),
        field('operator', choice('=', '+=', '-=', '*=', '/=', '%=')),
        field('value', $._expression),
      ),

    _place: ($) => choice($.identifier, $.self, $.field_expression, $.index_expression),

    if_statement: ($) =>
      prec.right(
        seq(
          'if',
          field('condition', $._expression),
          field('consequence', $.block),
          optional(seq('else', field('alternative', choice($.if_statement, $.block)))),
        ),
      ),

    while_statement: ($) => seq('while', field('condition', $._expression), field('body', $.block)),

    for_statement: ($) =>
      seq(
        'for',
        field('variable', $.identifier),
        'in',
        field('iterable', choice($.range_expression, $._expression)),
        field('body', $.block),
      ),

    range_expression: ($) => prec.left(seq($._expression, '..', $._expression)),

    match_statement: ($) =>
      seq('match', field('value', $._expression), '{', repeat(choice($.match_arm, ',', ';')), '}'),

    match_arm: ($) =>
      prec.right(seq(field('pattern', $._pattern), '=>', field('body', $._statement))),

    _pattern: ($) => choice($.wildcard_pattern, $.literal_pattern, $.variant_pattern),

    wildcard_pattern: (_) => '_',

    literal_pattern: ($) =>
      choice(seq(optional('-'), $.integer), $.string, $.boolean),

    variant_pattern: ($) =>
      seq(
        optional(seq(field('type', typeId($)), '.')),
        field('name', $.identifier),
        optional(seq('(', commaSep(choice($.identifier, '_')), ')')),
      ),

    break_statement: (_) => 'break',

    continue_statement: (_) => 'continue',

    return_statement: ($) => prec.right(seq('return', optional($._expression))),

    expression_statement: ($) => $._expression,

    _expression: ($) =>
      choice(
        $.integer,
        $.string,
        $.boolean,
        $.identifier,
        $.self,
        $.parenthesized_expression,
        $.list_expression,
        $.map_expression,
        $.unary_expression,
        $.binary_expression,
        $.call_expression,
        $.method_call_expression,
        $.field_expression,
        $.index_expression,
        $.struct_expression,
        $.try_expression,
      ),

    parenthesized_expression: ($) => seq('(', $._expression, ')'),

    list_expression: ($) => seq('[', commaSep($._expression), ']'),

    map_expression: ($) =>
      seq('[', choice(':', seq($.map_entry, repeat(seq(',', $.map_entry)), optional(','))), ']'),

    map_entry: ($) => seq(field('key', $._expression), ':', field('value', $._expression)),

    unary_expression: ($) =>
      prec(PREC.unary, seq(field('operator', choice('-', '!')), field('operand', $._expression))),

    binary_expression: ($) => {
      const table = [
        [PREC.or, '||'],
        [PREC.and, '&&'],
        [PREC.equality, choice('==', '!=')],
        [PREC.comparison, choice('<', '<=', '>', '>=')],
        [PREC.additive, choice('+', '-')],
        [PREC.multiplicative, choice('*', '/', '%')],
      ];
      return choice(
        ...table.map(([p, op]) =>
          prec.left(
            p,
            seq(field('left', $._expression), field('operator', op), field('right', $._expression)),
          ),
        ),
      );
    },

    arguments: ($) => seq('(', commaSep($._expression), ')'),

    // `f[T](x)` and `xs[i]` start alike; the parser tries both.
    call_expression: ($) =>
      seq(
        field('function', $.identifier),
        optional(field('type_arguments', $.type_arguments)),
        field('arguments', $.arguments),
      ),

    type_arguments: ($) => seq('[', commaSep($._type), ']'),

    // Above field access: `a.b(x)` is a call, not `a.b` followed by `(x)`.
    method_call_expression: ($) =>
      prec(
        PREC.postfix + 1,
        seq(
          field('receiver', $._expression),
          '.',
          field('method', $.identifier),
          field('arguments', $.arguments),
        ),
      ),

    field_expression: ($) =>
      prec.left(
        PREC.postfix,
        seq(field('value', $._expression), '.', field('field', fieldId($))),
      ),

    index_expression: ($) =>
      prec(PREC.postfix, seq(field('value', $._expression), '[', field('index', $._expression), ']')),

    // `Name { x: 1 }` and `if name { ... }` start alike; the parser tries
    // both.
    struct_expression: ($) =>
      seq(
        field('name', choice(typeId($), $.scoped_type_identifier)),
        '{',
        commaSep($.field_initializer),
        '}',
      ),

    scoped_type_identifier: ($) => seq($.identifier, '.', typeId($)),

    field_initializer: ($) =>
      seq(field('name', fieldId($)), ':', field('value', $._expression)),

    try_expression: ($) => prec(PREC.postfix, seq($._expression, '?')),

    integer: (_) => /[0-9][0-9_]*/,

    string: ($) => seq('"', repeat(choice($.escape_sequence, token.immediate(/[^"\\\n]+/))), '"'),

    escape_sequence: (_) => token.immediate(choice(/\\[nrt0\\"]/, /\\u\{[0-9a-fA-F]{1,6}\}/)),

    boolean: (_) => choice('true', 'false'),

    self: (_) => 'self',

    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,
  },
});
