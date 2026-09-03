//! `wiki search` — search a wiki directory for a term or regex across
//! markdown pages.
//!
//! Three output modes:
//!
//! - `summary` (default): per-page excerpt with `[[wiki-link]]` references
//!   so the next call can `cat` the page directly. Pages are sorted with
//!   `notes/` first, then `ideas/`, `projects/`, `sdd/`, then everything
//!   else.
//! - `files`: just file paths, like `grep -l`.
//! - `content`: matching lines with surrounding context, like `grep`.
//!
//! The default matching is a **literal substring** (case-insensitive).
//! Whitespace-separated literal queries use unordered AND semantics, which
//! keeps natural-language word order from suppressing relevant pages. Exact
//! phrases are ranked first. Pass `--regex` to switch to Rust regex syntax.
//! This default is the opposite of `grep` because wiki sources are often
//! natural language where `.` `(` `)` are common punctuation that should not
//! be regex metacharacters.

use crate::shared::{
    area_search_label, display_path, knowledge_area_for_relative_path, DEFAULT_WIKI_ROOT,
    KNOWLEDGE_AREAS,
};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
#[allow(dead_code)]
const DEFAULT_PER_FILE_LIMIT: usize = 5;
#[allow(dead_code)]
const DEFAULT_HEAD_LIMIT: usize = 50;
const HARD_PER_FILE_LIMIT: usize = 50;
const HARD_HEAD_LIMIT: usize = 500;
const HARD_PAGE_CAP: usize = 5_000;
const EXCERPT_CHAR_LIMIT: usize = 200;

/// Output mode for `wiki search`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SearchMode {
    /// Per-page summary: heading + one excerpt paragraph + match count.
    #[default]
    Summary,
    /// File paths only.
    Files,
    /// Matching lines with surrounding context.
    Content,
}

#[derive(Clone, Debug)]
pub struct SearchOptions {
    /// Search term (literal substring by default; whitespace-separated
    /// literals are unordered AND; Rust regex when `regex: true`).
    pub query: String,
    /// Wiki root. Defaults to `docs/`.
    pub path: Option<String>,
    /// Output mode.
    pub mode: SearchMode,
    /// Case-insensitive matching.
    pub case_insensitive: bool,
    /// Treat `query` as Rust regex instead of literal substring.
    pub regex: bool,
    /// Limit matches per file in summary mode.
    pub per_file_limit: usize,
    /// Hard ceiling on the number of files listed.
    pub head_limit: usize,
    /// Restrict to a standard knowledge area (`notes`, `ideas`, `projects`,
    /// `sdd`) or any nested relative directory.
    pub category: Option<String>,
    /// Lines of context before/after each match in content mode.
    pub context: usize,
}

#[derive(Debug)]
pub enum SearchError {
    /// Caller-side problem: empty query, invalid category, missing
    /// directory.
    Invalid(String),
    /// Filesystem error during scan.
    Io(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Io(message) => f.write_str(message),
        }
    }
}

/// Search a wiki and return the formatted result.
pub fn search(options: &SearchOptions) -> Result<String, SearchError> {
    if options.query.is_empty() {
        return Err(SearchError::Invalid(
            "wiki search query must be a non-empty string.".to_string(),
        ));
    }
    if let Some(category) = options.category.as_deref() {
        if category.contains("..") || category.starts_with('/') || category.starts_with('\\') {
            return Err(SearchError::Invalid(format!(
                "wiki search category must be a relative area or nested directory; got {category:?}."
            )));
        }
    }

    let root = resolve_root(options.path.as_deref())?;
    let search_root = match &options.category {
        Some(category) => root.join(category),
        None => root.clone(),
    };
    if !search_root.is_dir() {
        return Err(SearchError::Invalid(format!(
            "wiki search area or category directory does not exist: {}",
            search_root.display()
        )));
    }

    let per_file_limit = options.per_file_limit.clamp(1, HARD_PER_FILE_LIMIT);
    let head_limit = options.head_limit.clamp(1, HARD_HEAD_LIMIT);

    match options.mode {
        SearchMode::Files => {
            let matcher = Matcher::compile(&options.query, options.regex, options.case_insensitive)
                .map_err(SearchError::Invalid)?;
            let files = collect_matching_files(&search_root, &matcher, head_limit)?;
            Ok(format_files(&root, files))
        }
        SearchMode::Content => {
            let matcher = Matcher::compile(&options.query, options.regex, options.case_insensitive)
                .map_err(SearchError::Invalid)?;
            let pages = collect_pages(&search_root, HARD_PAGE_CAP).map_err(SearchError::Io)?;
            let mut matches_per_file: Vec<(PathBuf, Vec<ContentMatch>, MatchScore)> = Vec::new();
            for page in &pages {
                let bytes = match fs::read(page) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let text = String::from_utf8_lossy(&bytes);
                let hits = matcher.find_content_matches(&text, options.context);
                if !hits.is_empty() {
                    matches_per_file.push((page.clone(), hits, matcher.score(&text)));
                }
            }
            matches_per_file.sort_by(|(path_a, _, score_a), (path_b, _, score_b)| {
                page_sort_key(path_a, &root)
                    .0
                    .cmp(&page_sort_key(path_b, &root).0)
                    .then_with(|| score_b.cmp(score_a))
                    .then_with(|| page_sort_key(path_a, &root).cmp(&page_sort_key(path_b, &root)))
            });
            Ok(format_content(
                &root,
                &options.query,
                options.regex,
                options.case_insensitive,
                matcher.tokens(),
                head_limit,
                &matches_per_file,
            ))
        }
        SearchMode::Summary => {
            let matcher = Matcher::compile(&options.query, options.regex, options.case_insensitive)
                .map_err(SearchError::Invalid)?;
            let pages = collect_pages(&search_root, HARD_PAGE_CAP).map_err(SearchError::Io)?;
            if pages.is_empty() {
                return Ok(format!(
                    "(No markdown pages found under {}.)\n",
                    display_path(&search_root)
                ));
            }
            let mut pages_with_hits: Vec<(PathBuf, Vec<Excerpt>, MatchScore)> = Vec::new();
            for page in &pages {
                let bytes = match fs::read(page) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                let text = String::from_utf8_lossy(&bytes);
                let excerpts = matcher.find_excerpts(&text, per_file_limit, EXCERPT_CHAR_LIMIT);
                if !excerpts.is_empty() {
                    pages_with_hits.push((page.clone(), excerpts, matcher.score(&text)));
                }
            }
            pages_with_hits.sort_by(|(path_a, _, score_a), (path_b, _, score_b)| {
                page_sort_key(path_a, &root)
                    .0
                    .cmp(&page_sort_key(path_b, &root).0)
                    .then_with(|| score_b.cmp(score_a))
                    .then_with(|| page_sort_key(path_a, &root).cmp(&page_sort_key(path_b, &root)))
            });
            Ok(format_summary(
                &root,
                &options.query,
                options.regex,
                options.case_insensitive,
                matcher.tokens(),
                head_limit,
                &pages_with_hits,
            ))
        }
    }
}

fn resolve_root(input: Option<&str>) -> Result<PathBuf, SearchError> {
    let raw = input.unwrap_or(DEFAULT_WIKI_ROOT);
    if raw.trim().is_empty() {
        return Err(SearchError::Invalid(
            "wiki search path must be a non-empty directory path.".to_string(),
        ));
    }
    let path = PathBuf::from(raw);
    if !path.exists() {
        return Err(SearchError::Invalid(format!(
            "wiki search path does not exist: {}",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(SearchError::Invalid(format!(
            "wiki search path is not a directory: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn collect_pages(search_root: &Path, cap: usize) -> Result<Vec<PathBuf>, String> {
    let mut pages: Vec<PathBuf> = Vec::new();
    let walker = WalkDir::new(search_root)
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
        pages.push(path.to_path_buf());
        if pages.len() >= cap {
            break;
        }
    }
    pages.sort();
    Ok(pages)
}

fn collect_matching_files(
    search_root: &Path,
    matcher: &Matcher,
    head_limit: usize,
) -> Result<Vec<PathBuf>, SearchError> {
    let pages = collect_pages(search_root, HARD_PAGE_CAP).map_err(SearchError::Io)?;
    let mut hits: Vec<PathBuf> = Vec::new();
    for page in &pages {
        let Ok(bytes) = fs::read(page) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        if matcher.is_match(&text) {
            hits.push(page.clone());
            if hits.len() >= head_limit {
                break;
            }
        }
    }
    Ok(hits)
}

fn page_sort_key(path: &Path, root: &Path) -> (usize, String) {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let area_rank = knowledge_area_for_relative_path(relative)
        .and_then(|area| {
            KNOWLEDGE_AREAS
                .iter()
                .position(|candidate| candidate.0 == area.0)
        })
        .unwrap_or(KNOWLEDGE_AREAS.len());
    let path_text = relative.to_str().unwrap_or_default().replace('\\', "/");
    (area_rank, path_text)
}

#[derive(Debug)]
struct Matcher {
    regex: Regex,
    /// Token regexes used for unordered AND matching of literal multi-token
    /// queries. `None` for single-token literals and raw regex queries.
    token_regexes: Option<Vec<Regex>>,
    /// A whitespace/punctuation-tolerant phrase matcher used for ranking and
    /// excerpts when a multi-token query appears as a phrase.
    phrase_regex: Option<Regex>,
    /// Tokens produced when the literal query was split on whitespace.
    tokens: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct MatchScore {
    phrase_hits: usize,
    token_hits: usize,
}

impl Matcher {
    /// Returns a literal matcher. Multi-token literal queries use unordered
    /// AND semantics so natural-language word order does not suppress hits.
    /// Exact phrases are still tracked separately and ranked first.
    fn compile(query: &str, regex_mode: bool, case_insensitive: bool) -> Result<Self, String> {
        if query.is_empty() {
            return Err("wiki search query must be a non-empty string.".to_string());
        }
        let (pattern, token_patterns, phrase_pattern, tokens) = if regex_mode {
            (query.to_string(), None, None, None)
        } else {
            let split: Vec<&str> = query.split_whitespace().collect();
            if split.len() <= 1 {
                (regex::escape(query), None, None, None)
            } else {
                let token_patterns = split
                    .iter()
                    .map(|token| regex::escape(token))
                    .collect::<Vec<_>>();
                let phrase_pattern = split
                    .iter()
                    .map(|token| regex::escape(token))
                    .collect::<Vec<_>>()
                    .join(r"[\s\p{P}\p{S}/_-]*");
                (
                    regex::escape(query),
                    Some(token_patterns),
                    Some(phrase_pattern),
                    Some(split.into_iter().map(String::from).collect::<Vec<_>>()),
                )
            }
        };
        let regex = compile_regex(&pattern, case_insensitive)?;
        let token_regexes = token_patterns
            .map(|patterns| {
                patterns
                    .into_iter()
                    .map(|p| compile_regex(&p, case_insensitive))
                    .collect()
            })
            .transpose()?;
        let phrase_regex = phrase_pattern
            .map(|pattern| compile_regex(&pattern, case_insensitive))
            .transpose()?;
        Ok(Matcher {
            regex,
            token_regexes,
            phrase_regex,
            tokens,
        })
    }

    /// Tokens produced when the literal query was split on whitespace.
    fn tokens(&self) -> Option<&[String]> {
        self.tokens.as_deref()
    }

    fn is_match(&self, text: &str) -> bool {
        self.token_regexes
            .as_ref()
            .map(|regexes| regexes.iter().all(|regex| regex.is_match(text)))
            .unwrap_or_else(|| self.regex.is_match(text))
    }

    fn score(&self, text: &str) -> MatchScore {
        let phrase_hits = self
            .phrase_regex
            .as_ref()
            .map(|regex| regex.find_iter(text).count())
            .unwrap_or(0);
        let token_hits = self
            .token_regexes
            .as_ref()
            .map(|regexes| {
                regexes
                    .iter()
                    .map(|regex| regex.find_iter(text).count())
                    .sum()
            })
            .unwrap_or_else(|| self.regex.find_iter(text).count());
        MatchScore {
            phrase_hits,
            token_hits,
        }
    }

    fn match_ranges(&self, text: &str) -> Vec<(usize, usize)> {
        let mut ranges: Vec<(usize, usize)> = if let Some(phrase) = &self.phrase_regex {
            let phrase_ranges: Vec<_> = phrase
                .find_iter(text)
                .map(|m| (m.start(), m.end()))
                .collect();
            if !phrase_ranges.is_empty() {
                phrase_ranges
            } else {
                self.token_regexes
                    .as_ref()
                    .into_iter()
                    .flat_map(|regexes| regexes.iter())
                    .flat_map(|regex| regex.find_iter(text).map(|m| (m.start(), m.end())))
                    .collect()
            }
        } else {
            self.regex
                .find_iter(text)
                .map(|m| (m.start(), m.end()))
                .collect()
        };
        ranges.sort_unstable_by_key(|(start, _)| *start);
        ranges.dedup();
        ranges
    }

    /// Find up to `limit` excerpts in `text`. Each excerpt is a short
    /// block of context around the match, capped at `body_char_limit`
    /// characters.
    fn find_excerpts(&self, text: &str, limit: usize, body_char_limit: usize) -> Vec<Excerpt> {
        if !self.is_match(text) {
            return Vec::new();
        }
        let mut out: Vec<Excerpt> = Vec::new();
        for (start, end) in self.match_ranges(text) {
            if out.len() >= limit {
                break;
            }
            out.push(Excerpt {
                body: excerpt_around(text, start, end - start, body_char_limit),
            });
        }
        out
    }

    /// Find matching lines and merge the requested before/after context. For
    /// multi-token queries, a file is a hit only when every token exists, but
    /// content mode reports the lines containing each token so cross-line
    /// matches remain discoverable.
    fn find_content_matches(&self, text: &str, context: usize) -> Vec<ContentMatch> {
        let document_matches = self.is_match(text);
        if !document_matches {
            return Vec::new();
        }
        let lines: Vec<&str> = text.lines().collect();
        let mut matching_lines = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            let line_match = if let Some(regexes) = &self.token_regexes {
                regexes.iter().any(|regex| regex.is_match(line))
            } else {
                self.regex.is_match(line)
            };
            if line_match {
                matching_lines.push(index);
            }
        }
        if matching_lines.is_empty() {
            return Vec::new();
        }

        let mut ranges = Vec::new();
        for index in matching_lines {
            let start = index.saturating_sub(context);
            let end = (index + context + 1).min(lines.len());
            if let Some((_, previous_end)) = ranges.last_mut() {
                if start <= *previous_end {
                    *previous_end = (*previous_end).max(end);
                    continue;
                }
            }
            ranges.push((start, end));
        }

        ranges
            .into_iter()
            .flat_map(|(start, end)| {
                (start..end).map(|index| ContentMatch {
                    line_number: index + 1,
                    line: lines[index].to_string(),
                })
            })
            .collect()
    }
}

fn compile_regex(pattern: &str, case_insensitive: bool) -> Result<Regex, String> {
    let mut builder = regex::RegexBuilder::new(pattern);
    if case_insensitive {
        builder.case_insensitive(true);
    }
    builder
        .build()
        .map_err(|error| format!("Invalid regex pattern: {error}"))
}

#[derive(Debug)]
struct Excerpt {
    body: String,
}

#[derive(Debug)]
struct ContentMatch {
    line_number: usize,
    line: String,
}

fn excerpt_around(text: &str, match_pos: usize, match_len: usize, char_limit: usize) -> String {
    let line_start = text[..match_pos].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line_end = text[match_pos..]
        .find('\n')
        .map(|p| match_pos + p)
        .unwrap_or(text.len());
    let mut body = String::new();
    let mut cursor = line_start;
    let mut before_count = 0;
    while before_count < 2 && cursor > 0 {
        let prev_break = text[..cursor.saturating_sub(1)]
            .rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        if prev_break == cursor {
            break;
        }
        body = format!("{}{}\n", &text[prev_break..cursor], body);
        cursor = prev_break;
        before_count += 1;
    }
    let line = &text[line_start..line_end];
    body.push_str(line);
    body.push('\n');
    if body.chars().count() > char_limit {
        let truncate_at = body
            .char_indices()
            .nth(char_limit)
            .map(|(index, _)| index)
            .unwrap_or(body.len());
        body.truncate(truncate_at);
        body.push_str("...");
    }
    let _ = match_len;
    body
}

fn format_summary(
    root: &Path,
    query: &str,
    regex_mode: bool,
    case_insensitive: bool,
    tokens: Option<&[String]>,
    head_limit: usize,
    pages_with_hits: &[(PathBuf, Vec<Excerpt>, MatchScore)],
) -> String {
    let header = format_header("Wiki search", query, regex_mode, case_insensitive, tokens);
    if pages_with_hits.is_empty() {
        return format!("{header}(No pages contain this term.)\n");
    }
    let total_occurrences: usize = pages_with_hits.iter().map(|(_, e, _)| e.len()).sum();
    let mut out = String::new();
    out.push_str(&header);
    out.push_str(&format!(
        "Found in {} page(s); {} total occurrence(s):\n\n",
        pages_with_hits.len(),
        total_occurrences
    ));

    let mut shown_pages = 0usize;
    for (page, excerpts, _) in pages_with_hits {
        if shown_pages >= head_limit {
            out.push_str(&format!(
                "(Partial: more than head_limit={head_limit} pages contained the term. Raise the limit or narrow with `category` or `query`.)\n"
            ));
            break;
        }
        let rel = page
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .replace('\\', "/");
        let link = format!("[[{}]]", rel.trim_end_matches(".md"));
        if let Some(area) = knowledge_area_for_relative_path(Path::new(&rel)) {
            out.push_str(&format!("### {link} — {}\n", area_search_label(area.0)));
        } else {
            out.push_str(&format!("### {link}\n"));
        }
        for excerpt in excerpts {
            out.push_str("> ");
            out.push_str(&excerpt.body);
            if !excerpt.body.ends_with('\n') {
                out.push('\n');
            }
        }
        out.push('\n');
        shown_pages += 1;
    }

    out.push_str(&format!(
        "\n(Complete: {} page(s), {} occurrence(s) shown.)\n",
        shown_pages, total_occurrences
    ));

    out
}

fn format_files(root: &Path, files: Vec<PathBuf>) -> String {
    if files.is_empty() {
        return String::from("(No files matched.)\n");
    }
    let mut out = String::new();
    for file in &files {
        let rel = file
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .replace('\\', "/");
        out.push_str(&rel);
        out.push('\n');
    }
    out
}

/// Build the `== Wiki search: ... ==` header shared by the three formatters.
fn format_header(
    title: &str,
    query: &str,
    regex_mode: bool,
    case_insensitive: bool,
    tokens: Option<&[String]>,
) -> String {
    let mode = if regex_mode { "regex" } else { "literal" };
    let case = if case_insensitive {
        "case-insensitive"
    } else {
        "case-sensitive"
    };
    let match_kind = match (regex_mode, tokens) {
        (false, Some(parts)) => format!(
            "literal, unordered AND of {} token(s); exact phrase preferred: [{}]",
            parts.len(),
            parts.join(", ")
        ),
        _ => format!("{mode}, {case}"),
    };
    format!("== {title}: \"{query}\" ({match_kind}) ==\n")
}

fn format_content(
    root: &Path,
    query: &str,
    regex_mode: bool,
    case_insensitive: bool,
    tokens: Option<&[String]>,
    head_limit: usize,
    matches_per_file: &[(PathBuf, Vec<ContentMatch>, MatchScore)],
) -> String {
    let mut out = String::new();
    out.push_str(&format_header(
        "Wiki search (content)",
        query,
        regex_mode,
        case_insensitive,
        tokens,
    ));
    if matches_per_file.is_empty() {
        out.push_str("(No matches.)\n");
        return out;
    }
    for (shown_files, (page, hits, _)) in matches_per_file.iter().enumerate() {
        if shown_files >= head_limit {
            out.push_str(&format!(
                "(Partial: more than head_limit={head_limit} files contained the term.)\n"
            ));
            break;
        }
        let rel = page
            .strip_prefix(root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("")
            .replace('\\', "/");
        out.push_str(&format!("\n--- {rel} ---\n"));
        for hit in hits {
            out.push_str(&format!("{}: {}\n", hit.line_number, hit.line));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn default_opts(query: &str, root: &Path) -> SearchOptions {
        SearchOptions {
            query: query.to_string(),
            path: Some(root.to_string_lossy().into_owned()),
            mode: SearchMode::Summary,
            case_insensitive: true,
            regex: false,
            per_file_limit: DEFAULT_PER_FILE_LIMIT,
            head_limit: DEFAULT_HEAD_LIMIT,
            category: None,
            context: 2,
        }
    }

    #[test]
    fn literal_matcher_finds_substring_case_insensitive_by_default() {
        let m = Matcher::compile("feast.ID", false, true).unwrap();
        let text = "import feast.ID\nfeast.id is used.\n";
        let hits = m.find_excerpts(text, 5, 200);
        assert_eq!(hits.len(), 2, "{:?}", hits);
    }

    #[test]
    fn literal_matcher_does_not_treat_dot_as_regex_metacharacter() {
        let m = Matcher::compile("feast.ID", false, true).unwrap();
        let text = "feast.ID is the type. feastXID is unrelated.";
        let hits = m.find_excerpts(text, 5, 200);
        assert_eq!(hits.len(), 1, "{:?}", hits);
    }
    #[test]
    fn literal_matcher_splits_multi_token_queries_into_an_unordered_and() {
        let m = Matcher::compile("arena 战斗 战报", false, true).unwrap();
        let tokens = m.tokens().expect("multi-token literal must report tokens");
        assert_eq!(tokens, vec!["arena", "战斗", "战报"]);
        // Tokens may appear in any order and across newlines.
        let text = "前文\narena 起点\n中间任意文本\n战斗 在后\n更靠后的 战报 关键字\n";
        assert!(
            m.is_match(text),
            "expected all tokens to match; tokens={:?}",
            tokens
        );
        let ascii_text = "alpha\nbeta\ngamma\n";
        let m2 = Matcher::compile("alpha beta gamma", false, true).unwrap();
        assert!(m2.is_match(ascii_text), "ascii multi-token should match");
        // Out of order still matches; missing tokens do not.
        assert!(m.is_match("arena 战报\n战斗 关键词"));
        assert!(!m.is_match("只有 arena 和 战报 没有 combat-token"));
        assert!(!m.is_match("只有 arena 和 战斗 缺最后一个 token"));
    }

    #[test]
    fn literal_multi_token_query_matches_compact_cjk_terms() {
        let m = Matcher::compile("城池 任务", false, true).unwrap();
        let text = "本文介绍城池任务、声望和挑战。";
        assert!(m.is_match(text));
        assert_eq!(
            m.match_ranges(text).len(),
            1,
            "compact CJK phrase should rank as a phrase"
        );
        assert!(!m.is_match("本文只有城池和声望。"));
    }

    #[test]
    fn regex_matcher_compiles_and_finds() {
        let m = Matcher::compile(r"feast\.\w+", true, false).unwrap();
        let text = "feast.ID and feast.Name";
        let hits = m.find_excerpts(text, 5, 200);
        assert_eq!(hits.len(), 2, "{:?}", hits);
    }

    #[test]
    fn empty_query_is_rejected() {
        let err = Matcher::compile("", false, true).unwrap_err();
        assert!(err.contains("non-empty"), "{err}");
    }

    #[test]
    fn invalid_regex_is_rejected() {
        let err = Matcher::compile("(unclosed", true, false).unwrap_err();
        assert!(err.contains("Invalid regex"), "{err}");
    }

    #[test]
    fn excerpt_around_includes_the_matching_line() {
        let text = "header line\nfeast.ID is here\nfooter line\n";
        let body = excerpt_around(text, "header line\n".len(), "feast.ID".len(), 200);
        assert!(body.contains("feast.ID"), "{body}");
    }

    #[test]
    fn excerpt_around_truncates_long_excerpts() {
        let long = "a".repeat(1000);
        let text = format!("before\n{long}\nafter");
        let body = excerpt_around(&text, "before\n".len() + 50, 1, 80);
        assert!(body.len() <= 80 + 5, "body length: {}", body.len());
        assert!(body.ends_with("..."), "{body}");
    }

    #[test]
    fn excerpt_around_truncates_multibyte_text_at_a_character_boundary() {
        let text = "界".repeat(100);
        let body = excerpt_around(&text, 0, "界".len(), 80);
        assert_eq!(body, format!("{}...", "界".repeat(80)));
        assert!(body.is_char_boundary(body.len()));
    }

    #[test]
    fn search_rejects_invalid_category_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("a.md"), "x");
        let mut opts = default_opts("x", root);
        opts.category = Some("../escape".to_string());
        let err = search(&opts).unwrap_err();
        assert!(matches!(err, SearchError::Invalid(_)));
    }

    #[test]
    fn search_files_mode_mirrors_grep() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("a.md"), "feast.ID is used here");
        touch(&root.join("b.md"), "no match here");
        let mut opts = default_opts("feast.ID", root);
        opts.mode = SearchMode::Files;
        let text = search(&opts).unwrap();
        assert!(text.contains("a.md"), "{text}");
        assert!(!text.contains("b.md"), "{text}");
    }

    #[test]
    fn search_summary_mode_returns_wiki_link_per_page() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            &root.join("entities").join("feast.md"),
            "# feast\n\nfeast.ID is the type used throughout.\nMore lines.\n",
        );
        let text = search(&default_opts("feast.ID", root)).unwrap();
        assert!(text.contains("[[entities/feast]]"), "{text}");
        assert!(text.contains("Found in"), "{text}");
    }

    #[test]
    fn search_summary_prioritizes_notes_and_labels_standard_areas() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            &root.join("ideas").join("approach.md"),
            "# Approach\n\nUse the shared world.\n",
        );
        touch(
            &root.join("notes").join("architecture").join("world.md"),
            "# World\n\nUse the shared world.\n",
        );
        touch(
            &root.join("projects").join("world-repair").join("plan.md"),
            "# Plan\n\nUse the shared world.\n",
        );

        let text = search(&default_opts("shared world", root)).unwrap();
        let notes_at = text.find("[[notes/architecture/world]]").unwrap();
        let ideas_at = text.find("[[ideas/approach]]").unwrap();
        let projects_at = text.find("[[projects/world-repair/plan]]").unwrap();
        assert!(notes_at < ideas_at && ideas_at < projects_at, "{text}");
        assert!(text.contains("Notes / source of truth"), "{text}");
        assert!(text.contains("Ideas / original reasoning"), "{text}");
        assert!(text.contains("Projects / active work"), "{text}");
    }

    #[test]
    fn search_category_accepts_a_standard_area_and_excludes_other_areas() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            &root.join("notes").join("fact.md"),
            "# Fact\n\nshared term\n",
        );
        touch(
            &root.join("ideas").join("hypothesis.md"),
            "# Hypothesis\n\nshared term\n",
        );

        let mut opts = default_opts("shared term", root);
        opts.category = Some("notes".to_string());
        let text = search(&opts).unwrap();
        assert!(text.contains("[[notes/fact]]"), "{text}");
        assert!(!text.contains("[[ideas/hypothesis]]"), "{text}");
    }

    #[test]
    fn search_summary_indexes_deeply_nested_pages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let deep = [
            "projects",
            "alpha",
            "planning",
            "phase-one",
            "backend",
            "migration",
            "contracts",
            "review",
            "follow-up",
            "evidence",
        ]
        .iter()
        .fold(PathBuf::new(), |mut path, segment| {
            path.push(segment);
            path
        })
        .join("result.md");
        touch(&root.join(deep), "# Result\n\ndeep sentinel\n");

        let text = search(&default_opts("deep sentinel", root)).unwrap();
        assert!(text.contains(
            "[[projects/alpha/planning/phase-one/backend/migration/contracts/review/follow-up/evidence/result]]"
        ), "{text}");
    }

    #[test]
    fn search_content_mode_includes_line_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            &root.join("a.md"),
            "first line\nsecond line has target\nthird\n",
        );
        let mut opts = default_opts("target", root);
        opts.mode = SearchMode::Content;
        let text = search(&opts).unwrap();
        assert!(text.contains("2: second line has target"), "{text}");
    }

    #[test]
    fn search_content_mode_includes_requested_context() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("a.md"), "before\ntarget line\nafter\nfar away\n");
        let mut opts = default_opts("target", root);
        opts.mode = SearchMode::Content;
        opts.context = 1;
        let text = search(&opts).unwrap();
        assert!(text.contains("1: before"), "{text}");
        assert!(text.contains("2: target line"), "{text}");
        assert!(text.contains("3: after"), "{text}");
        assert!(!text.contains("4: far away"), "{text}");
    }

    #[test]
    fn search_summary_ranks_exact_phrase_before_separate_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            &root.join("notes").join("separate.md"),
            "# Separate\n\nRole appears here.\nS2S appears later.\n",
        );
        touch(
            &root.join("notes").join("phrase.md"),
            "# Phrase\n\nRole S2S is the exact phrase.\n",
        );
        let text = search(&default_opts("Role S2S", root)).unwrap();
        let phrase_at = text.find("[[notes/phrase]]").unwrap();
        let separate_at = text.find("[[notes/separate]]").unwrap();
        assert!(phrase_at < separate_at, "{text}");
    }
}
