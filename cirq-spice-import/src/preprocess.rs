//! `.include` / `.lib` preprocessor.
//!
//! Runs BEFORE the SPICE tokenizer. Resolves three directive forms:
//!
//! 1. `.include <path>` — read the referenced file (relative to the current
//!    file's directory, then each `lib_paths` entry) and splice its contents
//!    inline at the include point.
//!
//! 2. `.lib <path> <libname>` (two-argument form) — open `<path>`, locate
//!    `.lib <libname>` ... `.endl [<libname>]`, and splice ONLY that block.
//!
//! 3. `.lib <libname>` / `.endl [<libname>]` pairs *inside* an already-included
//!    file — conditional inclusion regions, scoped to the file they appear in.
//!    Only the section currently activated by an outer two-arg `.lib` call (or
//!    none at all, when reading top-level) is emitted.
//!
//! The preprocessor produces a flat SPICE string with no `.include`, no
//! two-arg `.lib`, and no `.lib`/`.endl` markers — ready to feed to
//! `Netlist::parse`.
//!
//! Encoding: UTF-8 is tried first. On UTF-8 failure the bytes are reinterpreted
//! as Latin-1 (each byte maps directly to a Unicode codepoint 0x00-0xFF) and a
//! single notice is written to stderr.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ImportError;

/// Options for the include preprocessor.
#[derive(Debug, Clone, Default)]
pub struct IncludeOptions {
    /// Directory of the originating source file. Used as the first search
    /// directory for relative `.include` / `.lib` paths.
    pub source_dir: Option<PathBuf>,
    /// Additional search directories tried (in order) after `source_dir`.
    pub lib_paths: Vec<PathBuf>,
}

impl IncludeOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the source directory (used to resolve relative include paths).
    pub fn with_source_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.source_dir = Some(dir.into());
        self
    }

    /// Append a library search path.
    pub fn add_lib_path(mut self, dir: impl Into<PathBuf>) -> Self {
        self.lib_paths.push(dir.into());
        self
    }
}

/// Run the include preprocessor over `source` and return the flattened SPICE
/// text. `opts` controls search-path behaviour.
pub fn preprocess_includes(source: &str, opts: &IncludeOptions) -> Result<String, ImportError> {
    let mut visiting: HashSet<PathBuf> = HashSet::new();
    let mut out = String::with_capacity(source.len());

    // Top-level expansion is not inside any library file; treat it as if it
    // were the originating source. The originating source's path is unknown
    // at this layer (we only have its text), so we record a synthetic key
    // that cannot collide with real file paths.
    let synthetic = PathBuf::from("<input>");
    visiting.insert(synthetic.clone());

    expand_text(
        source,
        opts.source_dir.as_deref(),
        opts,
        &mut visiting,
        &mut out,
        // No active library section at the top level: emit everything.
        None,
        0,
    )?;

    visiting.remove(&synthetic);
    Ok(out)
}

/// Maximum depth of `.include` nesting allowed. Mirrors ngspice's safety net
/// for runaway recursion that escapes the visited-set check (e.g. when each
/// level produces a unique path via symlinks).
const MAX_INCLUDE_DEPTH: usize = 64;

/// Recursively expand `text` line by line, resolving any `.include` /
/// `.lib path libname` directives encountered. `active_section` is `Some` when
/// the current text is inside a library file scoped to a specific section.
fn expand_text(
    text: &str,
    current_dir: Option<&Path>,
    opts: &IncludeOptions,
    visiting: &mut HashSet<PathBuf>,
    out: &mut String,
    active_section: Option<&str>,
    depth: usize,
) -> Result<(), ImportError> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(ImportError::Include(IncludeError::TooDeep(depth)));
    }

    // `current_section` tracks which conditional region we're currently in,
    // for files that contain `.lib <name>` / `.endl` markers inline.
    // None = not inside any region (emit if active_section is None).
    let mut current_section: Option<String> = None;

    for raw_line in text.split_inclusive('\n') {
        let line = raw_line.trim_end_matches('\n').trim_end_matches('\r');
        let trimmed = line.trim_start();
        let upper = trimmed.to_ascii_uppercase();

        // -- conditional region markers ----------------------------------
        if upper.starts_with(".LIB ") || upper == ".LIB" {
            // Distinguish `.lib <libname>` (1-arg, region marker) from
            // `.lib <path> <libname>` (2-arg, file include).
            let tokens = tokenize_args(trimmed);
            // tokens[0] is ".lib"
            match tokens.len() {
                2 => {
                    // 1-arg form: region marker. Open the named section.
                    let name = tokens[1].clone();
                    current_section = Some(name);
                    continue;
                }
                n if n >= 3 => {
                    // 2-arg form: include + section extraction.
                    // Should we emit at all? Only if we're in the right
                    // conditional region for this file.
                    if !should_emit(active_section, current_section.as_deref()) {
                        continue;
                    }
                    let path_arg = strip_quotes(&tokens[1]);
                    let lib_name = tokens[2].clone();
                    let resolved = resolve_path(path_arg, current_dir, opts)?;
                    if visiting.contains(&resolved) {
                        return Err(ImportError::Include(IncludeError::Circular(cycle_message(
                            visiting, &resolved,
                        ))));
                    }
                    let contents = read_file_lossy(&resolved)?;
                    let block = extract_lib_section(&contents, &lib_name).ok_or_else(|| {
                        ImportError::Include(IncludeError::LibSectionNotFound {
                            file: resolved.clone(),
                            name: lib_name.clone(),
                        })
                    })?;
                    visiting.insert(resolved.clone());
                    let child_dir = resolved.parent().map(|p| p.to_path_buf());
                    let res = expand_text(
                        block,
                        child_dir.as_deref(),
                        opts,
                        visiting,
                        out,
                        // Inside the spliced block, all top-level lines are
                        // active; nested `.lib name` markers inside the
                        // block can still open sub-regions if they appear.
                        None,
                        depth + 1,
                    );
                    visiting.remove(&resolved);
                    res?;
                    continue;
                }
                _ => {
                    // Malformed .lib — leave it for the SPICE parser to
                    // complain about (it will emit a clearer message).
                }
            }
        }

        if upper == ".ENDL" || upper.starts_with(".ENDL ") {
            // End of a conditional region inside this file.
            current_section = None;
            continue;
        }

        // -- emit gate ---------------------------------------------------
        if !should_emit(active_section, current_section.as_deref()) {
            continue;
        }

        // -- .include resolution ----------------------------------------
        let is_include = upper.starts_with(".INCLUDE ")
            || upper == ".INCLUDE"
            || upper.starts_with(".INC ")
            || upper == ".INC";
        if is_include {
            let tokens = tokenize_args(trimmed);
            if tokens.len() < 2 {
                return Err(ImportError::Include(IncludeError::MalformedInclude(
                    trimmed.to_string(),
                )));
            }
            let path_arg = strip_quotes(&tokens[1]);
            let resolved = resolve_path(path_arg, current_dir, opts)?;
            if visiting.contains(&resolved) {
                return Err(ImportError::Include(IncludeError::Circular(cycle_message(
                    visiting, &resolved,
                ))));
            }
            let contents = read_file_lossy(&resolved)?;
            visiting.insert(resolved.clone());
            let child_dir = resolved.parent().map(|p| p.to_path_buf());
            let res = expand_text(
                &contents,
                child_dir.as_deref(),
                opts,
                visiting,
                out,
                None,
                depth + 1,
            );
            visiting.remove(&resolved);
            res?;
            // Ensure a separator between spliced files.
            if !out.ends_with('\n') {
                out.push('\n');
            }
            continue;
        }

        // -- plain pass-through -----------------------------------------
        out.push_str(raw_line);
    }

    Ok(())
}

/// Whether a line should be emitted given the active outer section (set by an
/// outer two-arg `.lib` call) and the current inline section marker.
///
/// Rules:
///   - No active outer section, no current inline section: emit. (top level)
///   - No active outer section, inside an inline section: emit. (whole file
///     containing region markers is being included; the region just narrows
///     to what each `.lib name` block emits — but at this layer we're not
///     filtering by name when there's no outer constraint, since the user
///     explicitly asked for this file's full contents.)
///   - Active outer section name == current inline section: emit.
///   - Active outer section name != current inline section: skip.
fn should_emit(active_section: Option<&str>, current_inline: Option<&str>) -> bool {
    match (active_section, current_inline) {
        (None, _) => true,
        (Some(_), None) => true,
        (Some(want), Some(have)) => want.eq_ignore_ascii_case(have),
    }
}

/// Pull out the `.lib <name>` ... `.endl [<name>]` block from `contents`.
/// Returns the inner text (without the markers).
fn extract_lib_section<'a>(contents: &'a str, name: &str) -> Option<&'a str> {
    let mut start: Option<usize> = None;
    let mut end: Option<usize> = None;

    // Walk character-by-character using line offsets so the returned slice is
    // a valid &str over the input.
    let mut pos = 0;
    for line in contents.split_inclusive('\n') {
        let line_start = pos;
        let line_end = pos + line.len();
        pos = line_end;

        let trimmed = line.trim();
        let upper = trimmed.to_ascii_uppercase();

        if start.is_none() && upper.starts_with(".LIB ") {
            let tokens = tokenize_args(trimmed);
            if tokens.len() == 2 && tokens[1].eq_ignore_ascii_case(name) {
                // Start AFTER this line.
                start = Some(line_end);
            }
        } else if start.is_some() && (upper == ".ENDL" || upper.starts_with(".ENDL ")) {
            // `.endl` or `.endl <name>`. ngspice accepts either; if a name is
            // given it must match. Otherwise we treat the unnamed form as
            // closing the most recent block.
            let tokens = tokenize_args(trimmed);
            let matches = match tokens.len() {
                1 => true,
                _ => tokens[1].eq_ignore_ascii_case(name),
            };
            if matches {
                end = Some(line_start);
                break;
            }
        }
    }

    match (start, end) {
        (Some(s), Some(e)) if e >= s => Some(&contents[s..e]),
        _ => None,
    }
}

/// Resolve `path` against the current file's directory, then each entry in
/// `opts.lib_paths`, then the CWD. Returns the first existing match. If no
/// candidate exists, returns the union of search dirs in the error message.
fn resolve_path(
    path: &str,
    current_dir: Option<&Path>,
    opts: &IncludeOptions,
) -> Result<PathBuf, ImportError> {
    let p = Path::new(path);

    // Absolute paths bypass search entirely.
    if p.is_absolute() {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        return Err(ImportError::Include(IncludeError::FileNotFound {
            path: path.to_string(),
            tried: vec![p.to_path_buf()],
        }));
    }

    let mut tried: Vec<PathBuf> = Vec::new();

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = current_dir {
        candidates.push(dir.join(p));
    }
    if let Some(dir) = opts.source_dir.as_ref()
        && current_dir != Some(dir.as_path())
    {
        candidates.push(dir.join(p));
    }
    for extra in &opts.lib_paths {
        candidates.push(extra.join(p));
    }
    // Final fallback: CWD relative.
    candidates.push(p.to_path_buf());

    for c in candidates {
        if c.exists() {
            return Ok(c);
        }
        tried.push(c);
    }

    Err(ImportError::Include(IncludeError::FileNotFound {
        path: path.to_string(),
        tried,
    }))
}

/// Read `path` as a string. Tries UTF-8 first; on failure, falls back to
/// Latin-1 (single-byte → codepoint U+0000..U+00FF) and emits a one-line
/// stderr note.
fn read_file_lossy(path: &Path) -> Result<String, ImportError> {
    let bytes = std::fs::read(path).map_err(|e| {
        ImportError::Include(IncludeError::Io {
            path: path.to_path_buf(),
            source: e,
        })
    })?;
    match std::str::from_utf8(&bytes) {
        Ok(s) => Ok(s.to_owned()),
        Err(_) => {
            eprintln!(
                "warning: {} is not valid UTF-8; falling back to Latin-1 decoding",
                path.display()
            );
            Ok(bytes.iter().map(|&b| b as char).collect())
        }
    }
}

/// Tokenize directive arguments. Splits on whitespace but treats double-quoted
/// and single-quoted strings as single tokens (so paths with spaces work).
fn tokenize_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in line.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => {
                quote = None;
            }
            (Some(_), c) => cur.push(c),
            (None, '"') | (None, '\'') => {
                quote = Some(ch);
            }
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn strip_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

fn cycle_message(visiting: &HashSet<PathBuf>, repeat: &Path) -> String {
    let mut parts: Vec<String> = visiting.iter().map(|p| p.display().to_string()).collect();
    parts.sort();
    format!(
        "circular .include: {} would re-enter (already on stack: [{}])",
        repeat.display(),
        parts.join(", ")
    )
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors from include-resolution. Wrapped by [`ImportError::Include`].
#[derive(Debug, thiserror::Error)]
pub enum IncludeError {
    #[error("could not resolve .include `{path}` (tried: {})", format_tried(.tried))]
    FileNotFound { path: String, tried: Vec<PathBuf> },

    #[error("failed to read `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("malformed .include directive: `{0}`")]
    MalformedInclude(String),

    #[error(".lib section `{name}` not found in {file}")]
    LibSectionNotFound { file: PathBuf, name: String },

    #[error("{0}")]
    Circular(String),

    #[error(".include nesting exceeded {0} levels (likely a cycle the path check missed)")]
    TooDeep(usize),
}

fn format_tried(tried: &[PathBuf]) -> String {
    if tried.is_empty() {
        "<none>".to_string()
    } else {
        tried
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_handles_quoted_paths() {
        let out = tokenize_args(".include \"a b/c.lib\"");
        assert_eq!(out, vec![".include".to_string(), "a b/c.lib".to_string()]);
    }

    #[test]
    fn extract_lib_finds_named_block() {
        let s = ".lib tt\nR1 1 0 1k\n.endl tt\n.lib ss\nR2 2 0 2k\n.endl ss\n";
        let block = extract_lib_section(s, "ss").unwrap();
        assert!(block.contains("R2"));
        assert!(!block.contains("R1"));
    }

    #[test]
    fn extract_lib_case_insensitive() {
        let s = ".LIB TT\nR1 1 0 1k\n.ENDL TT\n";
        let block = extract_lib_section(s, "tt").unwrap();
        assert!(block.contains("R1"));
    }

    #[test]
    fn extract_lib_unnamed_endl_closes_block() {
        let s = ".lib tt\nR1 1 0 1k\n.endl\n";
        let block = extract_lib_section(s, "tt").unwrap();
        assert!(block.contains("R1"));
    }
}
