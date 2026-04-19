//! Source span tracking for Cirq AST nodes.

/// A byte-offset span in the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Byte offset of the start (inclusive).
    pub start: u32,
    /// Byte offset of the end (exclusive).
    pub end: u32,
}

impl Span {
    /// Create a new span.
    pub fn new(start: u32, end: u32) -> Self {
        debug_assert!(start <= end, "span start {start} > end {end}");
        Self { start, end }
    }

    /// A dummy span for synthetic nodes.
    pub fn dummy() -> Self {
        Self {
            start: u32::MAX,
            end: u32::MAX,
        }
    }

    /// Is this a dummy/synthetic span?
    pub fn is_dummy(&self) -> bool {
        self.start == u32::MAX
    }

    /// Length in bytes.
    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    /// Is this span empty?
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Merge two spans into one that covers both.
    pub fn merge(self, other: Self) -> Self {
        if self.is_dummy() {
            return other;
        }
        if other.is_dummy() {
            return self;
        }
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }
}
