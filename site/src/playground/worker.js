// The playground's worker: loads the interpreter once and runs programs.
importScripts('brim-wasm.js');

let ready = null;

self.onmessage = async (event) => {
  try {
    if (ready === null) {
      ready = fetch('brim.wasm')
        .then((response) => {
          if (!response.ok) throw new Error(`cannot load brim.wasm (${response.status})`);
          return response.arrayBuffer();
        })
        .then(loadBrim);
    }
    const run = await ready;
    const started = performance.now();
    const result = run(event.data.source, event.data.input);
    result.millis = performance.now() - started;
    self.postMessage(result);
  } catch (error) {
    ready = null;
    self.postMessage({ stdout: '', stderr: String(error), code: -1 });
  }
};
