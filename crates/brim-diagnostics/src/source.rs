use std::fmt;

/// A half-open byte range `[start, end)` into a [`SourceFile`]'s text.
///
/// Offsets always fall on UTF-8 character boundaries.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end);
        Span { start, end }
    }

    /// An empty span at `offset`.
    pub const fn point(offset: usize) -> Self {
        Span {
            start: offset,
            end: offset,
        }
    }

    /// The smallest span covering both `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub const fn len(self) -> usize {
        self.end - self.start
    }

    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// A human-readable position: 1-based line and 1-based column.
///
/// The column counts Unicode scalar values from the start of the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    pub line: usize,
    pub column: usize,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// A named piece of Brim source text together with a line table.
#[derive(Clone, Debug)]
pub struct SourceFile {
    name: String,
    text: String,
    /// Byte offset of the first character of every line.
    line_starts: Vec<usize>,
}

impl SourceFile {
    /// Creates a source file. `name` is what diagnostics display (usually the
    /// path as the user typed it).
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        SourceFile {
            name: name.into(),
            text,
            line_starts,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The source text covered by `span`.
    pub fn slice(&self, span: Span) -> &str {
        &self.text[span.start..span.end]
    }

    /// 0-based index of the line containing byte `offset`.
    fn line_index(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    /// Converts a byte offset into a 1-based line/column location.
    ///
    /// Offsets past the end of the text are clamped to the end.
    pub fn location(&self, offset: usize) -> Location {
        let offset = offset.min(self.text.len());
        let line = self.line_index(offset);
        let line_start = self.line_starts[line];
        let column = self.text[line_start..offset].chars().count() + 1;
        Location {
            line: line + 1,
            column,
        }
    }

    /// The number of lines in the file (an empty file has one empty line).
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// The text of 1-based line `line`, without its line terminator.
    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_starts[line - 1];
        let end = self
            .line_starts
            .get(line)
            .map_or(self.text.len(), |next| next - 1);
        let text = &self.text[start..end];
        text.strip_suffix('\r').unwrap_or(text)
    }

    /// Byte offset at which 1-based line `line` starts.
    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts[line - 1]
    }
}

/// Where diagnostics find the source text their spans point into.
pub trait Sources {
    /// The file containing global byte `offset`, the offset at which that
    /// file starts, and the file's index.
    fn locate(&self, offset: usize) -> (&SourceFile, usize, usize);

    /// The file named by diagnostics without a location.
    fn main_file(&self) -> &SourceFile;
}

impl Sources for SourceFile {
    fn locate(&self, _offset: usize) -> (&SourceFile, usize, usize) {
        (self, 0, 0)
    }

    fn main_file(&self) -> &SourceFile {
        self
    }
}

/// Several source files in one span space: each file's offsets are moved by
/// a *base*, so a [`Span`] identifies both a file and a position in it. The
/// first file has base 0, so programs of one file have the same spans as
/// before.
#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
    bases: Vec<usize>,
}

impl SourceMap {
    pub fn new() -> SourceMap {
        SourceMap::default()
    }

    /// Adds a file; returns the base its spans must be shifted by.
    pub fn add(&mut self, file: SourceFile) -> usize {
        // One unused offset between files keeps end-of-file positions apart.
        let base = match (self.files.last(), self.bases.last()) {
            (Some(last), Some(&b)) => b + last.text().len() + 1,
            _ => 0,
        };
        self.files.push(file);
        self.bases.push(base);
        base
    }

    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }

    /// The base of file `index`.
    pub fn base(&self, index: usize) -> usize {
        self.bases[index]
    }
}

impl Sources for SourceMap {
    fn locate(&self, offset: usize) -> (&SourceFile, usize, usize) {
        let index = match self.bases.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        (&self.files[index], self.bases[index], index)
    }

    fn main_file(&self) -> &SourceFile {
        &self.files[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_are_one_based() {
        let src = SourceFile::new("t.brim", "ab\ncd\n");
        assert_eq!(src.location(0), Location { line: 1, column: 1 });
        assert_eq!(src.location(1), Location { line: 1, column: 2 });
        assert_eq!(src.location(2), Location { line: 1, column: 3 });
        assert_eq!(src.location(3), Location { line: 2, column: 1 });
        assert_eq!(src.location(6), Location { line: 3, column: 1 });
        assert_eq!(src.line_count(), 3);
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        // "é" is two bytes in UTF-8.
        let src = SourceFile::new("t.brim", "\"é\" x");
        let x = src.text().find('x').unwrap();
        assert_eq!(x, 5);
        assert_eq!(src.location(x), Location { line: 1, column: 5 });
    }

    #[test]
    fn line_text_strips_terminators() {
        let src = SourceFile::new("t.brim", "one\r\ntwo\nthree");
        assert_eq!(src.line_text(1), "one");
        assert_eq!(src.line_text(2), "two");
        assert_eq!(src.line_text(3), "three");
    }

    #[test]
    fn offsets_past_end_are_clamped() {
        let src = SourceFile::new("t.brim", "ab");
        assert_eq!(src.location(99), Location { line: 1, column: 3 });
    }
}
