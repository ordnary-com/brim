// Runs programs through the playground's WebAssembly in Node, the way the
// page does:
//
//   cargo build --release --target wasm32-wasip1 -p brim-playground
//   node crates/brim-playground/smoke.js target/wasm32-wasip1/release/brim_playground.wasm
const fs = require('fs');
const path = require('path');
const assert = require('assert');
const { loadBrim } = require(path.join(__dirname, '../../site/src/playground/brim-wasm.js'));

(async () => {
  const run = await loadBrim(fs.readFileSync(process.argv[2]));
  const check = (source, input, stdout, code = 0) => {
    const result = run(source, input);
    assert.strictEqual(result.code, code, JSON.stringify(result));
    assert.strictEqual(result.stdout, stdout, JSON.stringify(result));
    return result;
  };
  check('fn main() {\n    print([3, 1, 2].sorted())\n}\n', '', '[1, 2, 3]\n');
  check('fn main() {\n    match read_line() {\n        Some(l) => print(l)\n        None => {}\n    }\n}\n', 'hi\n', 'hi\n');
  check(
    'import std.json\nimport std.random\nimport std.time\nfn main() {\n    time.sleep(10)\n    print(random.int(0, 3) < 3)\n    print(json.encode([1]))\n}\n',
    '',
    'true\n[1]\n',
  );
  const bad = check('fn main() {\n    let x: i64 = "a"\n}\n', '', '', 1);
  assert.match(bad.stderr, /error\[E0206\]/);
  const endless = check('fn main() {\n    loop {}\n}\n', '', '', 101);
  assert.match(endless.stderr, /R0009/);
  console.log('playground: ok');
})();
