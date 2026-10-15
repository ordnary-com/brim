// The playground page: an editor, examples, and a worker that runs the
// interpreter (worker.js, brim-wasm.js, brim.wasm).
(() => {
  const EXAMPLES = {
    'Hello': `fn main() {
    let name = "Brim"
    print(f"Hello, {name}!")
}
`,
    'Structs and enums': `struct Point {
    x: f64
    y: f64
}

enum Shape {
    Circle(Point, f64)
    Rect(Point, Point)
}

fn area(shape: Shape) -> f64 {
    return match shape {
        Shape.Circle(_, r) => PI * r * r
        Shape.Rect(a, b) => abs(b.x - a.x) * abs(b.y - a.y)
    }
}

fn main() {
    let shapes = [
        Shape.Circle(Point { x: 0.0, y: 0.0 }, 1.0),
        Shape.Rect(Point { x: 0.0, y: 0.0 }, Point { x: 2.0, y: 3.0 }),
    ]
    for s in shapes {
        print(f"{s}: {area(s)}")
    }
}
`,
    'Lists and closures': `fn main() {
    let numbers = [5, 3, 8, 1, 9, 2]
    let evens = numbers.filter(fn(n: i64) -> bool { return n % 2 == 0 })
    let squares = numbers.map(fn(n: i64) -> i64 { return n * n })
    print(numbers.sorted())
    print(evens)
    print(squares.sum())
    print(numbers.max())
}
`,
    'Errors with Result': `fn parse_age(text: string) -> Result[i64, string] {
    match text.trim().parse_int() {
        Some(n) => {
            if n < 0 {
                return Err(f"{n} is not an age")
            }
            return Ok(n)
        }
        None => return Err(f"\\"{text}\\" is not a number")
    }
}

fn main() {
    for input in ["36", "-1", "old"] {
        match parse_age(input) {
            Ok(age) => print(f"next year: {age + 1}")
            Err(e) => print(f"error: {e}")
        }
    }
}
`,
    'JSON': `import std.json

struct Note {
    id: i64
    text: string
    tags: List[string]
}

fn main() {
    let note = Note { id: 1, text: "buy milk", tags: ["shop"] }
    let text = json.encode(note)
    print(text)
    let back: Result[Note, string] = json.decode(text)
    print(back)
    let broken: Result[Note, string] = json.decode("{\\"id\\": \\"one\\"}")
    print(broken)
}
`,
    'Reading input': `fn main() {
    let mut total = 0
    loop {
        match read_line() {
            Some(line) => total += line.trim().parse_int().unwrap_or(0)
            None => break
        }
    }
    print(f"total: {total}")
}
`,
    'A type error': `fn main() {
    let count: i64 = "three"
    print(count + 1)
}
`,
  };
  const INPUTS = { 'Reading input': '1\n2\n39\n' };

  const $ = (id) => document.getElementById(id);
  const source = $('pg-source');
  const input = $('pg-input');
  const output = $('pg-output');
  const status = $('pg-status');
  const runButton = $('pg-run');
  const select = $('pg-example');
  if (!source) return;

  for (const name of Object.keys(EXAMPLES)) {
    const option = document.createElement('option');
    option.textContent = name;
    select.append(option);
  }
  const load = (name) => {
    source.value = EXAMPLES[name];
    input.value = INPUTS[name] ?? '';
    $('pg-input-box').open = name in INPUTS;
  };

  // A program shared in the link: #code=<base64url of the UTF-8 text>.
  const fromHash = () => {
    const match = location.hash.match(/^#code=([A-Za-z0-9_-]*)$/);
    if (!match) return false;
    try {
      const b64 = match[1].replace(/-/g, '+').replace(/_/g, '/');
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      source.value = new TextDecoder().decode(bytes);
      return true;
    } catch {
      return false;
    }
  };
  if (!fromHash()) load(select.value);
  select.addEventListener('change', () => load(select.value));

  let worker = null;
  let timer = null;
  const startWorker = () => {
    worker = new Worker('playground/worker.js');
    worker.onmessage = (event) => finish(event.data);
  };
  const show = (result) => {
    output.replaceChildren();
    const add = (text, cls) => {
      if (!text) return;
      const span = document.createElement('span');
      if (cls) span.className = cls;
      span.textContent = text;
      output.append(span);
    };
    add(result.stdout);
    add(result.stderr, 'pg-err');
    if (!result.stdout && !result.stderr) add('(no output)', 'pg-note');
  };
  const finish = (result) => {
    clearTimeout(timer);
    runButton.disabled = false;
    show(result);
    const took = result.millis !== undefined ? ` in ${Math.round(result.millis)} ms` : '';
    status.textContent = result.code === 0 ? `Done${took}` : `Exit code ${result.code}${took}`;
  };
  const run = () => {
    if (runButton.disabled) return;
    if (!worker) startWorker();
    runButton.disabled = true;
    status.textContent = 'Running...';
    // The step limit stops loops; this stops anything else.
    timer = setTimeout(() => {
      worker.terminate();
      worker = null;
      finish({ stdout: '', stderr: 'stopped after 20 seconds', code: -1 });
    }, 20000);
    worker.postMessage({ source: source.value, input: input.value });
  };
  runButton.addEventListener('click', run);

  $('pg-share').addEventListener('click', async () => {
    const bytes = new TextEncoder().encode(source.value);
    let binary = '';
    for (const b of bytes) binary += String.fromCharCode(b);
    const code = btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    history.replaceState(null, '', `#code=${code}`);
    try {
      await navigator.clipboard.writeText(location.href);
      status.textContent = 'Link copied';
    } catch {
      status.textContent = 'Link in the address bar';
    }
  });

  source.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      run();
    } else if (event.key === 'Tab' && !event.shiftKey) {
      event.preventDefault();
      const { selectionStart: start, selectionEnd: end } = source;
      source.setRangeText('    ', start, end, 'end');
    }
  });
})();
