// Checks a package archive: a gzip-compressed tar file whose entries are all
// regular files or directories under `{name}-{version}/`, with the manifest at
// `{name}-{version}/.brimproj` (or `brim.toml`, its name before 1.0, which
// clients up to 1.0.0-rc.1 write).

/** How much a valid archive may unpack to. Guards against gzip bombs. */
export const MAX_UNPACKED_BYTES = 128 * 1024 * 1024;

const BLOCK = 512;

export class ArchiveError extends Error {}

/** Reads a NUL-terminated (or field-filling) ASCII/UTF-8 string from a header field. */
function field(header: Uint8Array, offset: number, length: number): string {
  const bytes = header.subarray(offset, offset + length);
  const end = bytes.indexOf(0);
  try {
    return new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(end === -1 ? bytes : bytes.subarray(0, end));
  } catch {
    throw new ArchiveError("The archive is not a valid tar file: an entry name is not UTF-8.");
  }
}

function octal(header: Uint8Array, offset: number, length: number, what: string): number {
  if (header[offset]! & 0x80) throw new ArchiveError(`The archive is not a valid tar file: an entry has a binary ${what}.`);
  const text = field(header, offset, length).trim();
  if (text === "") return 0;
  if (!/^[0-7]+$/.test(text)) throw new ArchiveError(`The archive is not a valid tar file: an entry has a malformed ${what}.`);
  return parseInt(text, 8);
}

function checksumOk(header: Uint8Array): boolean {
  const stored = octal(header, 148, 8, "checksum");
  let sum = 0;
  for (let i = 0; i < BLOCK; i++) sum += i >= 148 && i < 156 ? 0x20 : header[i]!;
  return sum === stored;
}

/** Names Windows gives a meaning of their own, with or without an extension. */
const WINDOWS_DEVICES = /^(con|prn|aux|nul|com[0-9]|lpt[0-9])(\..*)?$/i;

function checkPath(path: string, root: string, isDir: boolean): void {
  if (path.startsWith("/") || /^[A-Za-z]:/.test(path) || path.includes("\\")) {
    throw new ArchiveError(`The archive contains an absolute or non-portable path: ${JSON.stringify(path)}.`);
  }
  const trimmed = isDir && path.endsWith("/") ? path.slice(0, -1) : path;
  const parts = trimmed.split("/");
  if (parts.some((p) => p === ".." || p === "." || p === "")) {
    throw new ArchiveError(`The archive contains a path with an empty, \`.\` or \`..\` component: ${JSON.stringify(path)}.`);
  }
  // Paths every system can unpack the same way.
  if (parts.some((p) => /[<>:"|?*\x00-\x1f]/.test(p) || /[. ]$/.test(p) || WINDOWS_DEVICES.test(p))) {
    throw new ArchiveError(`The archive contains a path that is not portable to every system: ${JSON.stringify(path)}.`);
  }
  if (parts[0] !== root || (parts.length === 1 && !isDir)) {
    throw new ArchiveError(`The archive contains ${JSON.stringify(path)}, which is not under \`${root}/\`.`);
  }
}

/** Reads a stream in pieces of exactly the requested size. */
class Reader {
  private chunks: Uint8Array[] = [];
  private buffered = 0;
  total = 0;
  private done = false;

  constructor(private readonly reader: ReadableStreamDefaultReader<Uint8Array>) {}

  private async fill(n: number): Promise<boolean> {
    while (this.buffered < n && !this.done) {
      const { value, done } = await this.reader.read();
      if (done) {
        this.done = true;
        break;
      }
      this.total += value.byteLength;
      if (this.total > MAX_UNPACKED_BYTES) {
        throw new ArchiveError(`The archive unpacks to more than ${MAX_UNPACKED_BYTES / 1024 / 1024} MiB.`);
      }
      this.chunks.push(value);
      this.buffered += value.byteLength;
    }
    return this.buffered >= n;
  }

  /** The next `n` bytes, or null at the end of the stream. */
  async read(n: number): Promise<Uint8Array | null> {
    if (!(await this.fill(n))) return null;
    const out = new Uint8Array(n);
    let filled = 0;
    while (filled < n) {
      const chunk = this.chunks[0]!;
      const take = Math.min(chunk.byteLength, n - filled);
      out.set(chunk.subarray(0, take), filled);
      filled += take;
      if (take === chunk.byteLength) this.chunks.shift();
      else this.chunks[0] = chunk.subarray(take);
    }
    this.buffered -= n;
    return out;
  }

  /** Skips `n` bytes; false if the stream ends first. */
  async skip(n: number): Promise<boolean> {
    let left = n;
    while (left > 0) {
      if (!(await this.fill(Math.min(left, 64 * 1024)))) return false;
      const chunk = this.chunks[0]!;
      const take = Math.min(chunk.byteLength, left);
      if (take === chunk.byteLength) this.chunks.shift();
      else this.chunks[0] = chunk.subarray(take);
      this.buffered -= take;
      left -= take;
    }
    return true;
  }

  async cancel(): Promise<void> {
    await this.reader.cancel().catch(() => {});
  }
}

/**
 * Checks the archive for `{name}-{version}`. Throws an [`ArchiveError`] whose
 * message is a sentence for the user when it is not acceptable.
 */
/** The most a package's `.brimproj` may hold. */
const MAX_MANIFEST_BYTES = 64 * 1024;

export async function checkArchive(archive: Uint8Array, name: string, version: string): Promise<string> {
  const root = `${name}-${version}`;
  const stream = new Blob([archive]).stream().pipeThrough(new DecompressionStream("gzip"));
  const reader = new Reader(stream.getReader());
  let manifest: string | null = null;
  const seen = new Set<string>();
  try {
    for (;;) {
      const header = await reader.read(BLOCK);
      if (header === null) break; // Tolerate a missing end-of-archive marker.
      if (header.every((b) => b === 0)) break; // End of archive.
      if (!checksumOk(header)) throw new ArchiveError("The archive is not a valid tar file: a header checksum is wrong.");

      const magic = field(header, 257, 6);
      if (magic !== "ustar" && magic !== "ustar ") {
        throw new ArchiveError("The archive is not a ustar tar file.");
      }
      let path = field(header, 0, 100);
      // The POSIX ustar prefix field; GNU headers use those bytes for other things.
      if (header[262] === 0) {
        const prefix = field(header, 345, 155);
        if (prefix) path = `${prefix}/${path}`;
      }

      const type = String.fromCharCode(header[156]!);
      const isFile = type === "0" || type === "\0";
      const isDir = type === "5";
      if (!isFile && !isDir) {
        throw new ArchiveError(
          `The archive contains ${JSON.stringify(path)}, which is not a regular file or a directory; only those are allowed.`,
        );
      }
      checkPath(path, root, isDir);
      // Paths that differ only in case are one file on Windows and macOS.
      const folded = (isDir && path.endsWith("/") ? path.slice(0, -1) : path).toLowerCase();
      if (seen.has(folded)) {
        throw new ArchiveError(`The archive contains ${JSON.stringify(path)} twice, or twice in different case.`);
      }
      seen.add(folded);

      const size = octal(header, 124, 12, "size");
      if (isDir && size !== 0) throw new ArchiveError(`The archive is not a valid tar file: the directory ${JSON.stringify(path)} has a size.`);
      const padded = Math.ceil(size / BLOCK) * BLOCK;
      if (isFile && (path === `${root}/.brimproj` || path === `${root}/brim.toml`)) {
        if (manifest !== null) throw new ArchiveError("The archive has two manifests.");
        if (size > MAX_MANIFEST_BYTES) throw new ArchiveError("The archive's .brimproj is larger than 64 KiB.");
        const bytes = await reader.read(padded);
        if (bytes === null) throw new ArchiveError("The archive is truncated.");
        try {
          manifest = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(bytes.subarray(0, size));
        } catch {
          throw new ArchiveError("The archive's .brimproj is not UTF-8.");
        }
      } else if (!(await reader.skip(padded))) {
        throw new ArchiveError("The archive is truncated.");
      }
    }
  } catch (e) {
    await reader.cancel();
    if (e instanceof ArchiveError) throw e;
    throw new ArchiveError("The archive is not a valid gzip file.");
  }
  await reader.cancel();
  if (manifest === null) throw new ArchiveError(`The archive has no \`${root}/.brimproj\`.`);
  return manifest;
}

/** The `.brimproj` that `brim publish` writes: its sections as key-value maps. */
export function parsePublishedManifest(text: string): Map<string, Map<string, string>> {
  const bad = (why: string) => new ArchiveError(`The archive's .brimproj is not as \`brim publish\` writes it: ${why}.`);
  const sections = new Map<string, Map<string, string>>();
  let current: Map<string, string> | null = null;
  for (const raw of text.split("\n")) {
    const line = raw.replace(/\r$/, "").trim();
    if (line === "" || line.startsWith("#")) continue;
    const header = /^\[([a-z]+)\]$/.exec(line);
    if (header) {
      if (sections.has(header[1]!)) throw bad(`[${header[1]}] appears twice`);
      current = new Map();
      sections.set(header[1]!, current);
      continue;
    }
    const pair = /^([a-z][a-z0-9_-]*) = "((?:[^"\\]|\\.)*)"$/.exec(line);
    if (!pair || current === null) throw bad(`unexpected line ${JSON.stringify(line.slice(0, 80))}`);
    const value = JSON.parse(`"${pair[2]!.replace(/\\u([0-9A-F]{4})/g, "\\u$1")}"`) as string;
    if (current.has(pair[1]!)) throw bad(`\`${pair[1]}\` appears twice`);
    current.set(pair[1]!, value);
  }
  return sections;
}
