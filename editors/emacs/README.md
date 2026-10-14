# Brim for Emacs

`brim-mode.el` highlights and indents `.brim` files and registers the
language server `brim lsp` with Eglot (built into Emacs 29).

```elisp
(add-to-list 'load-path "/path/to/brim/editors/emacs")
(require 'brim-mode)
(add-hook 'brim-mode-hook #'eglot-ensure)
;; Format on save:
(add-hook 'brim-mode-hook
          (lambda () (add-hook 'before-save-hook #'eglot-format-buffer nil t)))
```

With lsp-mode instead of Eglot:

```elisp
(with-eval-after-load 'lsp-mode
  (add-to-list 'lsp-language-id-configuration '(brim-mode . "brim"))
  (lsp-register-client
   (make-lsp-client :new-connection (lsp-stdio-connection '("brim" "lsp"))
                    :activation-fn (lsp-activate-on "brim")
                    :server-id 'brim)))
```
