//! `wiki read` — read the structure of a markdown wiki directory.
//!
//! Returns: page count, category breakdown, standard knowledge-area
//! counts, last-modified time, global context from `AGENTS.md`, the
//! contents of `index.md`, and optionally the last N entries of `log.md`
//! (parseable prefix `## [YYYY-MM-DD]`).
//!
//! All output is plain text so a follow-up `wiki search` or
//! `wiki read <page>` can pick up the paths it surfaces.

use crate::shared::{
    display_path, knowledge_area_for_relative_path, AGENTS_FILENAME, DEFAULT_WIKI_ROOT,
    INDEX_FILENAME, KNOWLEDGE_AREAS, LOG_FILENAME,
};
use std::collections::{BTreeSet, VecDeque};
use std::fs;
#[cfg(test)]
use std::io::Cursor;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;
#[allow(dead_code)]
const DEFAULT_INDEX_HEAD_LIMIT: usize = 200;
const HARD_INDEX_HEAD_LIMIT: usize = 2_000;
const HARD_LOG_LAST: usize = 50;
const HARD_PAGE_CAP: usize = 5_000;
/// Read options; all `bool`s default to true so the most useful
/// representation is also the one with the fewest flags.
#[derive(Clone, Debug)]
pub struct ReadOptions {
    /// Absolute path of the wiki root, or a path relative to the session
    /// working directory. Defaults to `docs/`.
    pub path: Option<String>,
    /// Include the wiki's `index.md` contents.
    pub include_index: bool,
    /// Include root `AGENTS.md` global context before the index.
    pub include_agents: bool,
    /// Number of most-recent `log.md` entries to include. 0 = omit the
    /// log section entirely.
    pub include_log_last: usize,
    /// Hard cap on the lines of `index.md` returned.
    pub index_head_limit: usize,
    /// Hard cap on lines of root `AGENTS.md` returned.
    pub agents_head_limit: usize,
    /// When true (default) the tool reports whether the directory looks
    /// like a wiki. When false it treats the directory as an arbitrary
    /// markdown tree and skips the hint.
    pub strict: bool,
}

impl ReadOptions {
    fn resolve_log_last(&self) -> usize {
        self.include_log_last.min(HARD_LOG_LAST)
    }

    fn resolve_index_head(&self) -> usize {
        self.index_head_limit.clamp(1, HARD_INDEX_HEAD_LIMIT)
    }

    fn resolve_agents_head(&self) -> usize {
        self.agents_head_limit.clamp(1, HARD_INDEX_HEAD_LIMIT)
    }
}

#[derive(Debug)]
pub enum ReadError {
    /// Caller-side problem: empty path, missing directory, file pointed
    /// at, etc. The CLI maps this to a non-zero exit with the message.
    Invalid(String),
    /// Filesystem error during scan.
    Io(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Io(message) => f.write_str(message),
        }
    }
}

/// Reads a wiki and returns the structured overview. Plain text suitable
/// for the CLI default.
pub fn read(options: &ReadOptions) -> Result<String, ReadError> {
    let root = resolve_root(options.path.as_deref())?;
    let include_log_last = options.resolve_log_last();
    let index_head_limit = options.resolve_index_head();
    let agents_head_limit = options.resolve_agents_head();
    let strict = options.strict;

    let scan = scan_wiki(
        &root,
        options.include_agents,
        options.include_index,
        include_log_last,
        agents_head_limit,
        index_head_limit,
    )
    .map_err(ReadError::Io)?;

    let mut out = String::new();
    out.push_str(&format!("== Wiki at {} ==\n", display_path(&root)));
    out.push_str(&format!(
        "Pages: {}    Categories: {}",
        scan.page_count,
        scan.categories.len()
    ));
    if !scan.categories.is_empty() {
        out.push_str(&format!(" ({})", scan.categories.join(", ")));
    }
    out.push('\n');
    out.push_str("Knowledge areas:\n");
    for area in &scan.areas {
        out.push_str(&format!(
            "  {}: {} page(s) — {}\n",
            area.name, area.page_count, area.purpose
        ));
    }
    if let (Some(mtime), Some(page)) = (scan.last_modified, scan.last_modified_page.as_deref()) {
        if let Some(human) = format_iso8601_short(mtime) {
            out.push_str(&format!("Last modified: {human} ({page})\n"));
        }
    }
    if strict && !scan.looks_like_wiki {
        out.push_str(
            "Note: this directory does not contain AGENTS.md or index.md; treating as a plain markdown tree.\n",
        );
    }
    out.push('\n');

    if options.include_agents {
        out.push_str("== Global context (AGENTS.md) ==\n");
        match scan.agents_body {
            Some(body) => {
                if body.is_empty() {
                    out.push_str("(AGENTS.md is empty.)\n");
                } else {
                    out.push_str(&body);
                }
                if scan.agents_truncated {
                    out.push_str(&format!(
                        "\n(Partial: AGENTS.md exceeded agents_head_limit={agents_head_limit}. Raise the limit or open the file directly for the rest.)\n"
                    ));
                }
            }
            None => {
                out.push_str(
                    "(AGENTS.md not present — no global context is available. Existing index.md-based wikis remain supported; run `wiki init` to add the standard structure.)\n",
                );
            }
        }
        out.push('\n');
    }

    if options.include_index {
        out.push_str("== Index ==\n");
        match scan.index_body {
            Some(body) => {
                if body.is_empty() {
                    out.push_str("(index.md is empty.)\n");
                } else {
                    out.push_str(&body);
                }
                if scan.index_truncated {
                    out.push_str(&format!(
                        "\n(Partial: index.md exceeded index_head_limit={index_head_limit}. Raise the limit or open the file directly for the rest.)\n"
                    ));
                }
            }
            None => {
                out.push_str(&format!(
                    "({INDEX_FILENAME} not present — the catalog has not been created yet.)\n"
                ));
            }
        }
        out.push('\n');
    }

    if include_log_last > 0 {
        out.push_str(&format!(
            "== Recent log entries (last {include_log_last}) ==\n"
        ));
        match scan.log_body {
            Some(body) if !body.is_empty() => {
                out.push_str(&body);
            }
            Some(_) => {
                out.push_str("(log.md is empty.)\n");
            }
            None => {
                out.push_str(&format!(
                    "({LOG_FILENAME} not present — no chronology recorded yet.)\n"
                ));
            }
        }
        out.push('\n');
    }

    if scan.truncated {
        out.push_str(&format!(
            "\n(Partial: wiki has more than {HARD_PAGE_CAP} .md pages; counts reflect the first {HARD_PAGE_CAP} discovered. Move older pages to an archive directory or narrow the search.)\n"
        ));
    } else {
        out.push_str(&format!(
            "\n(Complete: {} page(s) indexed.)\n",
            scan.page_count
        ));
    }

    Ok(out)
}

fn resolve_root(input: Option<&str>) -> Result<PathBuf, ReadError> {
    let raw = input.unwrap_or(DEFAULT_WIKI_ROOT);
    if raw.trim().is_empty() {
        return Err(ReadError::Invalid(
            "wiki read path must be a non-empty directory path.".to_string(),
        ));
    }
    let path = PathBuf::from(raw);
    if !path.exists() {
        return Err(ReadError::Invalid(format!(
            "wiki read path does not exist: {}",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(ReadError::Invalid(format!(
            "wiki read path is not a directory: {}",
            path.display()
        )));
    }
    Ok(path)
}

struct WikiScan {
    page_count: usize,
    categories: Vec<String>,
    looks_like_wiki: bool,
    areas: Vec<AreaPageCount>,
    /// `AGENTS.md` body, truncated to `agents_head_limit` lines, trailing
    /// newline preserved. `None` when the file is absent.
    agents_body: Option<String>,
    /// True when `AGENTS.md` was longer than `agents_head_limit` lines.
    agents_truncated: bool,
    /// `index.md` body, truncated to `index_head_limit` lines, trailing
    /// newline preserved. `None` when the file is absent.
    index_body: Option<String>,
    /// True when `index.md` was longer than `index_head_limit` lines.
    index_truncated: bool,
    /// Last `n` recognised `## [...]` entries from `log.md`. `None` when
    /// the file is absent.
    log_body: Option<String>,
    last_modified: Option<SystemTime>,
    last_modified_page: Option<String>,
    /// True when `page_count` hit the hard cap.
    truncated: bool,
}

struct AreaPageCount {
    name: &'static str,
    purpose: &'static str,
    page_count: usize,
}

fn scan_wiki(
    root: &Path,
    include_agents: bool,
    include_index: bool,
    log_last_n: usize,
    agents_head_limit: usize,
    index_head_limit: usize,
) -> Result<WikiScan, String> {
    let mut page_count = 0usize;
    let mut categories_set: BTreeSet<String> = BTreeSet::new();
    let mut areas: Vec<AreaPageCount> = KNOWLEDGE_AREAS
        .iter()
        .map(|(name, purpose)| AreaPageCount {
            name,
            purpose,
            page_count: 0,
        })
        .collect();
    let mut has_agents = false;
    let mut has_index = false;
    let mut has_log = false;
    let mut last_modified: Option<(SystemTime, String)> = None;

    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let name = entry.file_name();
            if name == ".git" || name == "node_modules" || name == "target" || name == ".venv" {
                return false;
            }
            true
        });

    for entry in walker {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap_or(path);
        page_count += 1;
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                let rel = relative
                    .to_str()
                    .map(|s| s.replace('\\', "/"))
                    .or_else(|| {
                        path.file_name()
                            .and_then(|s| s.to_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                if last_modified
                    .as_ref()
                    .map_or(true, |(previous, _)| *previous < mtime)
                {
                    last_modified = Some((mtime, rel));
                }
            }
        }
        if relative == Path::new(AGENTS_FILENAME) {
            has_agents = true;
            continue;
        }
        if relative == Path::new(INDEX_FILENAME) {
            has_index = true;
            continue;
        }
        if relative == Path::new(LOG_FILENAME) {
            has_log = true;
            continue;
        }
        if let Some(parent) = relative
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            if let Some(component) = parent.components().next() {
                if let Some(name) = component.as_os_str().to_str() {
                    if name != "." && !name.is_empty() {
                        categories_set.insert(name.to_string());
                    }
                }
            }
        }
        if let Some(area) = knowledge_area_for_relative_path(relative) {
            if let Some(area_count) = areas.iter_mut().find(|count| count.name == area.0) {
                area_count.page_count += 1;
            }
        }
        if page_count >= HARD_PAGE_CAP {
            break;
        }
    }

    let truncated = page_count >= HARD_PAGE_CAP;
    let (last_modified, last_modified_page) = match last_modified {
        Some((time, page)) => (Some(time), Some(page)),
        None => (None, None),
    };

    let (agents_body, agents_truncated) = if include_agents {
        read_root_markdown(&root.join(AGENTS_FILENAME), has_agents, agents_head_limit)
    } else {
        (None, false)
    };
    let (index_body, index_truncated) = if include_index {
        read_root_markdown(&root.join(INDEX_FILENAME), has_index, index_head_limit)
    } else {
        (None, false)
    };

    let log_body = if has_log && log_last_n > 0 {
        read_log_entries(&root.join(LOG_FILENAME), log_last_n)
    } else {
        None
    };
    let categories: Vec<String> = categories_set.into_iter().collect();
    let looks_like_wiki = has_agents || has_index;
    Ok(WikiScan {
        page_count,
        categories,
        looks_like_wiki,
        areas,
        agents_body,
        agents_truncated,
        index_body,
        index_truncated,
        log_body,
        last_modified,
        last_modified_page,
        truncated,
    })
}

fn read_root_markdown(path: &Path, present: bool, head_limit: usize) -> (Option<String>, bool) {
    if !present {
        return (None, false);
    }
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return (Some(String::new()), false),
    };
    let mut reader = BufReader::new(file);
    let mut body = String::new();
    let mut line = String::new();

    for _ in 0..head_limit {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return (Some(body), false),
            Ok(_) => body.push_str(&line),
            Err(_) => return (Some(String::new()), false),
        }
    }

    line.clear();
    match reader.read_line(&mut line) {
        Ok(0) => (Some(body), false),
        Ok(_) => (Some(normalize_head(&body)), true),
        Err(_) => (Some(String::new()), false),
    }
}

fn normalize_head(body: &str) -> String {
    let mut normalized = String::new();
    for line in body.lines() {
        normalized.push_str(line);
        normalized.push('\n');
    }
    normalized
}

/// Returns the last `n` log entries — each entry starts with a line
/// beginning with `## [`. Falls back to the last `n` lines when no
/// recognised entry markers are present.
#[cfg(test)]
fn extract_last_log_entries(bytes: &[u8], n: usize) -> String {
    extract_last_log_entries_from_reader(Cursor::new(bytes), n).unwrap_or_default()
}

fn read_log_entries(path: &Path, n: usize) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    extract_last_log_entries_from_reader(BufReader::new(file), n).ok()
}

fn extract_last_log_entries_from_reader<R: BufRead>(mut reader: R, n: usize) -> io::Result<String> {
    if n == 0 {
        return Ok(String::new());
    }

    let mut tail_lines = VecDeque::with_capacity(n);
    let mut entries = VecDeque::with_capacity(n);
    let mut current_entry: Option<String> = None;
    let mut saw_marker = false;
    let mut bytes = Vec::new();

    loop {
        bytes.clear();
        if reader.read_until(b'\n', &mut bytes)? == 0 {
            break;
        }
        let line = normalize_log_line(&bytes);
        if line.trim_start().starts_with("## [") {
            if let Some(entry) = current_entry.take() {
                push_bounded(&mut entries, entry, n);
            }
            saw_marker = true;
            current_entry = Some(format!("{line}\n"));
        } else if let Some(entry) = current_entry.as_mut() {
            entry.push_str(&line);
            entry.push('\n');
        } else {
            push_bounded(&mut tail_lines, format!("{line}\n"), n);
        }
    }

    if let Some(entry) = current_entry {
        push_bounded(&mut entries, entry, n);
    }

    let selected = if saw_marker { entries } else { tail_lines };
    Ok(selected.into_iter().collect())
}

fn normalize_log_line(bytes: &[u8]) -> String {
    let has_newline = bytes.ends_with(b"\n");
    let line = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let line = if has_newline {
        line.strip_suffix(b"\r").unwrap_or(line)
    } else {
        line
    };
    String::from_utf8_lossy(line).into_owned()
}

fn push_bounded<T>(items: &mut VecDeque<T>, item: T, limit: usize) {
    if items.len() == limit {
        items.pop_front();
    }
    items.push_back(item);
}

fn format_iso8601_short(t: SystemTime) -> Option<String> {
    let duration = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    let secs = duration.as_secs();
    let (year, month, day, hour, minute, second) = epoch_to_calendar(secs)?;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

/// Howard Hinnant's civil-from-days algorithm. Avoids pulling in `chrono`
/// for one human-readable string.
fn epoch_to_calendar(secs: u64) -> Option<(i32, u32, u32, u32, u32, u32)> {
    let days = (secs / 86_400) as i64;
    let secs_of_day = (secs % 86_400) as u32;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = (if m <= 2 { y + 1 } else { y }) as i32;
    Some((year, m, d, hour, minute, second))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn default_opts(path: &Path) -> ReadOptions {
        ReadOptions {
            path: Some(path.to_string_lossy().into_owned()),
            include_index: true,
            include_agents: true,
            include_log_last: 0,
            index_head_limit: DEFAULT_INDEX_HEAD_LIMIT,
            agents_head_limit: DEFAULT_INDEX_HEAD_LIMIT,
            strict: true,
        }
    }

    #[test]
    fn read_returns_pages_categories_and_index_for_a_typical_wiki() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            &root.join("index.md"),
            "# Index\n\n## Sources\n- [[article-foo]]\n",
        );
        touch(
            &root.join("log.md"),
            "noise\n## [2026-08-01] setup | init | [[index]]\n",
        );
        touch(
            &root.join("sources").join("article-foo.md"),
            "---\ntags: [source]\n---\n\n# Source: foo\n",
        );
        touch(
            &root.join("entities").join("feast.md"),
            "---\ntags: [entity]\n---\n\n# feast\n",
        );

        let mut opts = default_opts(root);
        opts.include_log_last = 1;
        let response = read(&opts).unwrap();
        assert!(response.contains("Pages: 4"), "{response}");
        assert!(response.contains("Categories: 2"), "{response}");
        assert!(response.contains("entities"), "{response}");
        assert!(response.contains("sources"), "{response}");
        assert!(response.contains("## [2026-08-01] setup"), "{response}");
        assert!(response.contains("Last modified:"), "{response}");
        assert!(response.contains("(Complete:"), "{response}");
    }

    #[test]
    fn read_surfaces_agents_before_index_and_summarizes_knowledge_areas() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            &root.join("AGENTS.md"),
            "# Global context\n\nRead notes before advising.\n",
        );
        touch(
            &root.join("index.md"),
            "# Index\n\n[[notes/architecture]]\n",
        );
        touch(
            &root.join("log.md"),
            "## [2026-08-09] init | created knowledge base | [[AGENTS]]\n",
        );
        touch(
            &root.join("notes").join("architecture").join("system.md"),
            "# System\n\nVerified fact.\n",
        );
        touch(
            &root.join("ideas").join("heuristics.md"),
            "# Heuristics\n\nOriginal reasoning.\n",
        );
        touch(
            &root.join("projects").join("wiki-redesign").join("plan.md"),
            "# Plan\n\nActive work.\n",
        );
        touch(
            &root.join("sdd").join("changes").join("article.md"),
            "# Article\n\nRaw capture.\n",
        );

        let response = read(&default_opts(root)).unwrap();
        assert!(response.contains("Pages: 7"), "{response}");
        assert!(
            response.contains("notes: 1 page(s) — verified source of truth"),
            "{response}"
        );
        assert!(
            response.contains("ideas: 1 page(s) — original reasoning and preferences"),
            "{response}"
        );
        assert!(
            response.contains("projects: 1 page(s) — active work and verification"),
            "{response}"
        );
        assert!(
            response.contains("sdd: 1 page(s) — spec-driven changes and archives"),
            "{response}"
        );
        let agents_at = response.find("== Global context (AGENTS.md) ==").unwrap();
        let index_at = response.find("== Index ==").unwrap();
        assert!(agents_at < index_at, "{response}");
        assert!(
            response.contains("Read notes before advising."),
            "{response}"
        );
    }

    #[test]
    fn read_keeps_legacy_index_wikis_compatible_when_agents_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("index.md"), "# Existing index\n");
        touch(&root.join("pages").join("existing.md"), "# Existing page\n");

        let response = read(&default_opts(root)).unwrap();
        assert!(response.contains("AGENTS.md not present"), "{response}");
        assert!(!response.contains("plain markdown tree"), "{response}");
        assert!(response.contains("# Existing index"), "{response}");
    }

    #[test]
    fn read_omits_log_when_include_log_last_is_zero() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("index.md"), "# Index\n");
        touch(
            &root.join("log.md"),
            "## [2026-08-01] setup | init | [[index]]\n",
        );

        let response = read(&default_opts(root)).unwrap();
        assert!(!response.contains("Recent log entries"), "{response}");
    }

    #[test]
    fn scan_skips_unrequested_root_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join(AGENTS_FILENAME), "# Rules\n");
        touch(&root.join(INDEX_FILENAME), "# Index\n");
        touch(&root.join(LOG_FILENAME), "## [2026-08-01] setup\n");

        let scan = scan_wiki(root, false, false, 0, 200, 200).unwrap();
        assert!(scan.agents_body.is_none());
        assert!(scan.index_body.is_none());
        assert!(scan.log_body.is_none());
    }

    #[test]
    fn read_takes_only_the_last_n_log_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            &root.join("log.md"),
            "## [2026-08-01] ingest | a | [[a]]\n## [2026-08-02] ingest | b | [[b]]\n## [2026-08-03] ingest | c | [[c]]\n",
        );

        let mut opts = default_opts(root);
        opts.include_index = false;
        opts.include_agents = false;
        opts.include_log_last = 2;
        let response = read(&opts).unwrap();
        assert!(response.contains("## [2026-08-02]"), "{response}");
        assert!(response.contains("## [2026-08-03]"), "{response}");
        assert!(!response.contains("## [2026-08-01]"), "{response}");
    }

    #[test]
    fn read_reports_missing_index_when_directory_has_no_index_md() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("entities").join("a.md"), "x");

        let response = read(&default_opts(root)).unwrap();
        assert!(response.contains("not present"), "{response}");
        assert!(response.contains("plain markdown tree"), "{response}");
    }

    #[test]
    fn read_rejects_non_existent_path() {
        let opts = ReadOptions {
            path: Some("Z:/nope/definitely/not/here".to_string()),
            ..default_opts(&std::env::temp_dir())
        };
        let err = read(&opts).unwrap_err();
        assert!(matches!(err, ReadError::Invalid(_)));
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn read_rejects_file_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.md");
        fs::write(&file, "x").unwrap();
        let mut opts = default_opts(&file);
        opts.path = Some(file.to_string_lossy().into_owned());
        let err = read(&opts).unwrap_err();
        assert!(matches!(err, ReadError::Invalid(_)));
        assert!(err.to_string().contains("not a directory"));
    }

    #[test]
    fn read_partial_marker_when_index_exceeds_head_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let big_index: String = (0..500).map(|i| format!("line {i}\n")).collect();
        touch(&root.join("index.md"), &big_index);

        let mut opts = default_opts(root);
        opts.index_head_limit = 50;
        let response = read(&opts).unwrap();
        assert!(
            response.contains("Partial: index.md exceeded index_head_limit=50"),
            "{response}"
        );
    }

    #[test]
    fn read_truncates_agents_with_its_own_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let big_agents: String = (0..100).map(|i| format!("rule {i}\n")).collect();
        touch(&root.join("AGENTS.md"), &big_agents);

        let mut opts = default_opts(root);
        opts.agents_head_limit = 10;
        let response = read(&opts).unwrap();
        assert!(
            response.contains("Partial: AGENTS.md exceeded agents_head_limit=10"),
            "{response}"
        );
        assert!(response.contains("rule 9"), "{response}");
        assert!(!response.contains("rule 10"), "{response}");
    }

    #[test]
    fn read_handles_wiki_with_no_categories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("index.md"), "# Index\n");
        touch(&root.join("a.md"), "x");

        let response = read(&default_opts(root)).unwrap();
        assert!(response.contains("Categories: 0"), "{response}");
    }

    #[test]
    fn read_indexes_pages_in_nested_category_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("index.md"), "# Index\n");
        touch(
            &root
                .join("projects")
                .join("adventure-global-repair")
                .join("plan.md"),
            "# Adventure repair plan\n",
        );
        touch(
            &root
                .join("notes")
                .join("architecture")
                .join("shared-ecs-world.md"),
            "# Shared ECS World\n",
        );

        let response = read(&default_opts(root)).unwrap();
        assert!(response.contains("Pages: 3"), "{response}");
        assert!(response.contains("Categories: 2"), "{response}");
        assert!(response.contains("notes"), "{response}");
        assert!(response.contains("projects"), "{response}");
        assert!(response.contains("notes: 1 page(s)"), "{response}");
        assert!(response.contains("projects: 1 page(s)"), "{response}");
    }

    #[test]
    fn extract_last_log_entries_picks_only_entries_with_marker_prefix() {
        let body = b"noise line\n## [2026-08-01] setup | init | [[i]]\nmore noise\n## [2026-08-02] ingest | a | [[a]]\n";
        let out = extract_last_log_entries(body, 1);
        assert!(out.contains("## [2026-08-02]"), "{out}");
        assert!(!out.contains("## [2026-08-01]"), "{out}");
    }

    #[test]
    fn extract_last_log_entries_falls_back_to_tail_when_no_markers() {
        let body = b"line a\nline b\nline c\nline d\n";
        let out = extract_last_log_entries(body, 2);
        assert!(out.contains("line c"), "{out}");
        assert!(out.contains("line d"), "{out}");
        assert!(!out.contains("line a"), "{out}");
    }

    #[test]
    fn extract_last_log_entries_preserves_selected_entry_bodies() {
        let body = b"noise\r\n## [2026-08-01] setup\r\nfirst\r\n## [2026-08-02] ingest\r\nsecond";
        let out = extract_last_log_entries(body, 1);
        assert_eq!(out, "## [2026-08-02] ingest\nsecond\n");
    }

    #[test]
    fn epoch_to_calendar_handles_the_unix_epoch() {
        let (y, m, d, h, mi, s) = epoch_to_calendar(0).unwrap();
        assert_eq!((y, m, d, h, mi, s), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn epoch_to_calendar_handles_a_known_instant() {
        let (y, m, d, _h, _mi, _s) = epoch_to_calendar(1_785_484_800).unwrap();
        assert_eq!((y, m, d), (2026, 7, 31));
    }
}
