// Brim highlighting for the highlight.js that mdBook bundles.
(function () {
  if (typeof hljs === "undefined") return;
  hljs.registerLanguage("brim", function () {
    return {
      name: "Brim",
      keywords: {
        keyword:
          "fn let mut return if else while for in match break continue struct enum impl pub import as",
        literal: "true false self Some None Ok Err",
        type: "i64 bool string void Option Result List Map",
        built_in:
          "print eprint to_string read_line read_file write_file args env exit assert assert_eq",
      },
      contains: [
        hljs.C_LINE_COMMENT_MODE,
        { className: "string", begin: '"', end: '"', contains: [hljs.BACKSLASH_ESCAPE] },
        { className: "number", begin: "\\b\\d+\\b" },
        { className: "title", begin: "\\b[A-Z][A-Za-z0-9_]*\\b" },
      ],
    };
  });
  document.querySelectorAll("code.language-brim").forEach(function (block) {
    hljs.highlightBlock(block);
  });
})();
