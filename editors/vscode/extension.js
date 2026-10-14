// Starts `brim lsp` for Brim files.
const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

let client;

function activate(context) {
  const command = vscode.workspace.getConfiguration("brim").get("path") || "brim";
  const server = { command, args: ["lsp"] };
  client = new LanguageClient(
    "brim",
    "Brim",
    { run: server, debug: server },
    { documentSelector: [{ scheme: "file", language: "brim" }] }
  );
  context.subscriptions.push(client);
  return client.start();
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
