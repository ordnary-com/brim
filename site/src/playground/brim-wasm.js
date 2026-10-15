// Runs Brim programs with the interpreter compiled to WebAssembly
// (crates/brim-playground). Works in a Web Worker and in Node.
//
//   const run = await loadBrim(bytes);
//   const { stdout, stderr, code } = run(source, input);
//
// The module is built for WASI; the few system calls it makes (the clock,
// random numbers, sleeping) are provided here, and every other one answers
// "not supported" (files and sockets are not available in the browser).

const ENOSYS = 52;
// What the interpreter wrote to its own standard error: a panic message.
let lastPanic = '';
const EBADF = 8;

function wasiImports(memory) {
  const view = () => new DataView(memory().buffer);
  const bytes = () => new Uint8Array(memory().buffer);
  const now = () => (typeof performance !== 'undefined' ? performance.timeOrigin + performance.now() : Date.now());
  const known = {
    random_get(ptr, len) {
      const out = bytes().subarray(ptr, ptr + len);
      for (let i = 0; i < len; i += 65536) {
        crypto.getRandomValues(out.subarray(i, Math.min(len, i + 65536)));
      }
      return 0;
    },
    clock_time_get(id, _precision, out) {
      // Realtime (0) or monotonic (1), in nanoseconds.
      const ms = id === 0 ? Date.now() : now();
      view().setBigUint64(out, BigInt(Math.round(ms * 1e6)), true);
      return 0;
    },
    environ_sizes_get(count, size) {
      view().setUint32(count, 0, true);
      view().setUint32(size, 0, true);
      return 0;
    },
    environ_get() {
      return 0;
    },
    fd_write(fd, iovs, count, written) {
      // Only a panic message would get here; the program's own output is
      // returned by brim_run.
      let total = 0;
      for (let i = 0; i < count; i++) {
        const ptr = view().getUint32(iovs + i * 8, true);
        const len = view().getUint32(iovs + i * 8 + 4, true);
        if (fd === 2) {
          lastPanic += new TextDecoder().decode(bytes().subarray(ptr, ptr + len));
        }
        total += len;
      }
      view().setUint32(written, total, true);
      return 0;
    },
    fd_prestat_get() {
      return EBADF;
    },
    poll_oneoff(subscriptions, events, count, eventCount) {
      // `time.sleep`: one clock subscription. Waits by spinning, which is
      // fine in a worker.
      const v = view();
      if (count === 1 && v.getUint8(subscriptions + 8) === 0) {
        const timeout = Number(v.getBigUint64(subscriptions + 24, true)) / 1e6;
        const absolute = (v.getUint16(subscriptions + 40, true) & 1) === 1;
        const until = absolute ? timeout : now() + timeout;
        while (now() < until) {
          // spin
        }
        v.setBigUint64(events, v.getBigUint64(subscriptions, true), true);
        v.setUint16(events + 8, 0, true);
        v.setUint8(events + 10, 0);
        v.setUint32(eventCount, 1, true);
        return 0;
      }
      return ENOSYS;
    },
    proc_exit(code) {
      throw new Error(`the interpreter stopped (exit ${code})`);
    },
  };
  return new Proxy(known, {
    get: (target, name) => target[name] ?? (() => ENOSYS),
  });
}

async function loadBrim(wasmBytes) {
  let instance;
  const memory = () => instance.exports.memory;
  const result = await WebAssembly.instantiate(wasmBytes, {
    wasi_snapshot_preview1: wasiImports(memory),
  });
  instance = result.instance;
  const { brim_alloc, brim_free, brim_run } = instance.exports;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const put = (text) => {
    const data = encoder.encode(text);
    const ptr = brim_alloc(data.length);
    new Uint8Array(memory().buffer, ptr, data.length).set(data);
    return [ptr, data.length];
  };
  return function run(source, input) {
    const [sourcePtr, sourceLen] = put(source);
    const [inputPtr, inputLen] = put(input);
    const out = brim_run(sourcePtr, sourceLen, inputPtr, inputLen);
    const length = new DataView(memory().buffer).getUint32(out, true);
    const json = decoder.decode(new Uint8Array(memory().buffer, out + 4, length));
    brim_free(out, length + 4);
    brim_free(sourcePtr, sourceLen);
    brim_free(inputPtr, inputLen);
    return JSON.parse(json);
  };
}

if (typeof module !== 'undefined') {
  module.exports = { loadBrim };
}
