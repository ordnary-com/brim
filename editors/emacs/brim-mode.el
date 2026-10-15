;;; brim-mode.el --- Major mode for the Brim programming language -*- lexical-binding: t; -*-

;; Version: 0.1.0
;; Package-Requires: ((emacs "29.1"))
;; Keywords: languages
;; URL: https://github.com/ordnary-com/brim

;;; Commentary:

;; Highlighting and indentation for Brim (`.brim' files), and the Brim
;; language server (`brim lsp') through Eglot: diagnostics, formatting,
;; imenu, go to definition and hover.
;;
;;   (add-to-list 'load-path "/path/to/brim/editors/emacs")
;;   (require 'brim-mode)
;;   (add-hook 'brim-mode-hook #'eglot-ensure)

;;; Code:

(defgroup brim nil
  "Support for the Brim programming language."
  :group 'languages)

(defcustom brim-indent-offset 4
  "Indentation of Brim code, in columns."
  :type 'integer)

(defcustom brim-command "brim"
  "The `brim' executable, for the language server."
  :type 'string)

(defconst brim-keywords
  '("fn" "let" "mut" "return" "if" "else" "while" "for" "in" "match" "break"
    "continue" "struct" "enum" "impl" "pub" "import" "as"))

(defconst brim-reserved '("async" "await" "const" "loop" "trait" "type" "use"))

(defconst brim-types '("i64" "i32" "u8" "u64" "f64" "bool" "string" "void" "Option" "Result" "List" "Map" "Box"))

(defconst brim-builtins
  '("print" "eprint" "to_string" "to_float" "to_int" "to_i32" "to_u8" "to_u64" "string_from_bytes" "read_line" "read_file" "write_file" "args" "env"
    "exit" "assert" "assert_eq"))

(defconst brim-constants '("true" "false" "self" "Some" "None" "Ok" "Err"))

(defvar brim-font-lock-keywords
  `((,(regexp-opt brim-keywords 'symbols) . font-lock-keyword-face)
    (,(regexp-opt brim-reserved 'symbols) . font-lock-warning-face)
    (,(regexp-opt brim-types 'symbols) . font-lock-type-face)
    (,(regexp-opt brim-constants 'symbols) . font-lock-constant-face)
    (,(concat (regexp-opt brim-builtins 'symbols) "\\s-*(") 1 font-lock-builtin-face)
    ("\\_<fn\\s-+\\([A-Za-z_][A-Za-z0-9_]*\\)" 1 font-lock-function-name-face)
    ("\\_<[A-Z][A-Za-z0-9_]*\\_>" . font-lock-type-face)
    ("\\_<[0-9][0-9_]*\\(?:\\.[0-9][0-9_]*\\)?\\(?:[eE][+-]?[0-9]+\\)?\\_>" . font-lock-constant-face)))

(defvar brim-mode-syntax-table
  (let ((table (make-syntax-table)))
    (modify-syntax-entry ?/ ". 12" table)
    (modify-syntax-entry ?\n ">" table)
    (modify-syntax-entry ?\" "\"" table)
    (modify-syntax-entry ?\\ "\\" table)
    (modify-syntax-entry ?_ "_" table)
    table)
  "Syntax table for `brim-mode'.")

(defun brim-indent-line ()
  "Indent the current line by the depth of its brackets."
  (interactive)
  (let* ((depth (save-excursion (car (syntax-ppss (line-beginning-position)))))
         (closing (save-excursion
                    (back-to-indentation)
                    (looking-at-p "[]})]")))
         (column (* brim-indent-offset (max 0 (- depth (if closing 1 0))))))
    (if (<= (current-column) (current-indentation))
        (indent-line-to column)
      (save-excursion (indent-line-to column)))))

(defvar brim-imenu-generic-expression
  '(("Functions" "^\\s-*\\(?:pub\\s-+\\)?fn\\s-+\\([A-Za-z_][A-Za-z0-9_]*\\)" 1)
    ("Structs" "^\\s-*\\(?:pub\\s-+\\)?struct\\s-+\\([A-Za-z_][A-Za-z0-9_]*\\)" 1)
    ("Enums" "^\\s-*\\(?:pub\\s-+\\)?enum\\s-+\\([A-Za-z_][A-Za-z0-9_]*\\)" 1)))

;;;###autoload
(define-derived-mode brim-mode prog-mode "Brim"
  "Major mode for editing Brim code."
  (setq-local font-lock-defaults '(brim-font-lock-keywords))
  (setq-local comment-start "// ")
  (setq-local comment-start-skip "//+\\s-*")
  (setq-local indent-line-function #'brim-indent-line)
  (setq-local indent-tabs-mode nil)
  (setq-local imenu-generic-expression brim-imenu-generic-expression)
  (setq-local electric-indent-chars (append "{}()[]" electric-indent-chars)))

;;;###autoload
(add-to-list 'auto-mode-alist '("\\.brim\\'" . brim-mode))

(with-eval-after-load 'eglot
  (add-to-list 'eglot-server-programs `(brim-mode . (,brim-command "lsp"))))

(provide 'brim-mode)
;;; brim-mode.el ends here
