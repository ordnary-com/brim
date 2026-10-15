# Playground

Write a program and run it here, in your browser: the Brim interpreter runs
as WebAssembly, so nothing is sent anywhere. Standard input is the text in
the box below the editor. Programs can use every `std` module except the
ones that need files, processes or sockets, and stop after 200 million
steps. Press Ctrl+Enter (⌘+Enter on a Mac) to run; **Share** puts the
program in the page's link.

<div id="pg">
  <div class="pg-bar">
    <label class="pg-label" for="pg-example">Example</label>
    <select id="pg-example"></select>
    <span class="pg-spacer"></span>
    <span id="pg-status" role="status"></span>
    <button id="pg-share" type="button">Share</button>
    <button id="pg-run" type="button" class="pg-primary">Run</button>
  </div>
  <textarea id="pg-source" spellcheck="false" autocapitalize="off" autocomplete="off" aria-label="Program"></textarea>
  <details id="pg-input-box">
    <summary>Standard input</summary>
    <textarea id="pg-input" spellcheck="false" aria-label="Standard input"></textarea>
  </details>
  <pre id="pg-output" aria-live="polite"></pre>
</div>

<link rel="stylesheet" href="playground/playground.css">
<script src="playground/playground.js"></script>
