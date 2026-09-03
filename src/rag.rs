//! Offline graph-aware retrieval over the local SQLite/FTS index.
//!
//! This is the first RAG layer: FTS5 or explicit Rust regex supplies the lexical seed set, then
//! explicit page links and evidence-backed rule-extracted entity relations
//! add nearby chunks. The result is deterministic and inspectable; it does
//! not call an embedding service or an LLM.

use crate::embedding::{self, EmbeddingMode, EmbeddingProvider};
use crate::shared::DEFAULT_WIKI_ROOT;
use regex::RegexBuilder;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const INDEX_DIR: &str = ".wiki";
const INDEX_DB: &str = "index.sqlite";
const MAX_LIMIT: usize = 100;
const MAX_RECALL: usize = 500;
const MAX_QUERY_FALLBACK_TERMS: usize = 8;
const MAX_PAGE_DEPTH: usize = 8;
const MAX_PAGE_CHUNKS: usize = 4;
const MAX_ENTITY_SCOPES: usize = 64;
const MAX_ENTITY_CHUNKS: usize = 8;
const MAX_VECTOR_SCAN: usize = 5_000;
const EXCERPT_CHARS: usize = 360;
pub const DEFAULT_CONTEXT_CHARS: usize = 8_000;
const MAX_CONTEXT_CHARS: usize = 50_000;

#[derive(Clone, Debug)]
pub struct RagSearchOptions {
    pub path: Option<String>,
    pub query: String,
    pub regex: bool,
    pub limit: usize,
    pub depth: usize,
    pub embedding_mode: EmbeddingMode,
    pub weights: RagWeights,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RagWeights {
    pub lexical: f64,
    pub graph: f64,
    pub vector: f64,
}

impl Default for RagWeights {
    fn default() -> Self {
        Self {
            lexical: 1.0,
            graph: 1.0,
            vector: 1.0,
        }
    }
}

impl RagWeights {
    fn sanitized(self) -> Self {
        Self {
            lexical: finite_non_negative(self.lexical),
            graph: finite_non_negative(self.graph),
            vector: finite_non_negative(self.vector),
        }
    }

    fn is_zero(self) -> bool {
        self.lexical == 0.0 && self.graph == 0.0 && self.vector == 0.0
    }
}

#[derive(Debug)]
pub enum RagError {
    Invalid(String),
    Database(String),
}

impl std::fmt::Display for RagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Database(message) => f.write_str(message),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RagHit {
    pub chunk_id: String,
    pub path: String,
    pub title: String,
    pub heading_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub lexical_score: f64,
    pub graph_score: f64,
    pub vector_score: f64,
    pub score: f64,
    pub reasons: Vec<String>,
    pub provenance: Vec<RagProvenance>,
    pub excerpt: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RagProvenance {
    pub kind: String,
    pub source: String,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RagContext {
    pub query: String,
    pub text: String,
    pub hits: Vec<RagHit>,
    pub truncated: bool,
}

#[derive(Clone, Debug)]
struct ChunkRecord {
    id: String,
    document_id: String,
    path: String,
    title: String,
    heading_path: String,
    start_line: usize,
    end_line: usize,
    text: String,
    text_hash: String,
}

#[derive(Clone, Copy, Debug)]
enum ScoreKind {
    Lexical,
    Graph,
    Vector,
}

impl ScoreKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Graph => "graph",
            Self::Vector => "vector",
        }
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    chunk: ChunkRecord,
    lexical_score: f64,
    graph_score: f64,
    vector_score: f64,
    provenance: BTreeMap<String, (ScoreKind, f64)>,
}

#[derive(Default)]
struct PageGraph {
    outgoing: BTreeMap<String, Vec<String>>,
    incoming: BTreeMap<String, Vec<String>>,
}

/// Search the indexed wiki with lexical retrieval plus graph expansion.
pub fn search(options: &RagSearchOptions) -> Result<Vec<RagHit>, RagError> {
    let query = options.query.trim();
    if query.is_empty() {
        return Err(RagError::Invalid(
            "wiki rag search query must be a non-empty string.".to_string(),
        ));
    }
    let limit = options.limit.clamp(1, MAX_LIMIT);
    let depth = options.depth.min(MAX_PAGE_DEPTH);
    let weights = options.weights.sanitized();
    if weights.is_zero() {
        return Err(RagError::Invalid(
            "wiki rag search requires at least one positive score weight.".to_string(),
        ));
    }
    let connection = open_database(options.path.as_deref())?;
    let recall_limit = options.limit.saturating_mul(8).clamp(20, MAX_RECALL) as i64;

    let direct_chunks = load_direct_chunks(&connection, query, options.regex, recall_limit)?;

    let mut candidates: BTreeMap<String, Candidate> = BTreeMap::new();
    let mut direct_chunk_ids = BTreeSet::new();
    let mut seed_document_ids = BTreeSet::new();
    for (chunk, boost, reason) in direct_chunks {
        direct_chunk_ids.insert(chunk.id.clone());
        seed_document_ids.insert(chunk.document_id.clone());
        add_candidate(&mut candidates, &chunk, boost, reason, ScoreKind::Lexical);
    }

    if !seed_document_ids.is_empty() && depth > 0 {
        let page_graph = load_page_graph(&connection)?;
        let page_distances = page_neighborhood(&page_graph, &seed_document_ids, depth);
        for (document_id, distance) in page_distances {
            if distance == 0 {
                continue;
            }
            let chunks = load_document_chunks(&connection, &document_id, MAX_PAGE_CHUNKS)?;
            for chunk in chunks {
                add_candidate(
                    &mut candidates,
                    &chunk,
                    0.36 / distance as f64,
                    format!("page-link depth {distance}"),
                    ScoreKind::Graph,
                );
            }
        }
    }

    let seed_entity_ids = load_entity_ids(&connection, &direct_chunk_ids)?;
    for (entity_id, (reason, boost)) in load_entity_scopes(&connection, &seed_entity_ids)?
        .into_iter()
        .take(MAX_ENTITY_SCOPES)
    {
        let chunks = load_entity_chunks(&connection, &entity_id, MAX_ENTITY_CHUNKS)?;
        for chunk in chunks {
            add_candidate(
                &mut candidates,
                &chunk,
                boost,
                reason.clone(),
                ScoreKind::Graph,
            );
        }
    }

    if let Some(provider) = embedding::provider(options.embedding_mode) {
        ensure_embedding_cache_schema(&connection)?;
        let vector_limit = options.limit.saturating_mul(8).clamp(20, MAX_RECALL);
        let provider_label = format!(
            "{}:{}",
            provider.descriptor().provider,
            provider.descriptor().model
        );
        for (chunk, similarity) in
            load_vector_chunks(&connection, provider.as_ref(), query, vector_limit)?
        {
            add_candidate(
                &mut candidates,
                &chunk,
                similarity,
                provider_label.clone(),
                ScoreKind::Vector,
            );
        }
    }

    let mut hits: Vec<RagHit> = candidates
        .into_values()
        .map(|candidate| {
            let score = candidate.lexical_score * weights.lexical
                + candidate.graph_score * weights.graph
                + candidate.vector_score * weights.vector;
            let provenance = candidate
                .provenance
                .iter()
                .map(|(source, (kind, component_score))| RagProvenance {
                    kind: kind.as_str().to_string(),
                    source: source.clone(),
                    score: *component_score,
                })
                .collect::<Vec<_>>();
            RagHit {
                chunk_id: candidate.chunk.id,
                path: candidate.chunk.path,
                title: candidate.chunk.title,
                heading_path: candidate.chunk.heading_path,
                start_line: candidate.chunk.start_line,
                end_line: candidate.chunk.end_line,
                lexical_score: candidate.lexical_score * weights.lexical,
                graph_score: candidate.graph_score * weights.graph,
                vector_score: candidate.vector_score * weights.vector,
                score,
                reasons: provenance.iter().map(|item| item.source.clone()).collect(),
                provenance,
                excerpt: make_excerpt(&candidate.chunk.text),
            }
        })
        .collect();
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.start_line.cmp(&right.start_line))
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    hits.truncate(limit);
    Ok(hits)
}

pub fn format_results(query: &str, hits: &[RagHit]) -> String {
    let mut output = format!(
        "== Graph-RAG search ==\nQuery: {query}\nFound {} result(s).\n",
        hits.len()
    );
    if hits.is_empty() {
        output.push_str("(No indexed chunks matched the query or its graph neighborhood.)\n");
        return output;
    }
    output.push('\n');
    for hit in hits {
        let heading = if hit.heading_path.is_empty() {
            hit.title.as_str()
        } else {
            hit.heading_path.as_str()
        };
        output.push_str(&format!(
            "- score: {:.3} (lexical {:.3}, graph {:.3}, vector {:.3}) | {}:{}-{} | {}\n",
            hit.score,
            hit.lexical_score,
            hit.graph_score,
            hit.vector_score,
            hit.path,
            hit.start_line,
            hit.end_line,
            heading
        ));
        output.push_str(&format!("  reasons: {}\n", hit.reasons.join(", ")));
        output.push_str(&format!("  excerpt: {}\n", hit.excerpt));
    }
    output
}

#[derive(Serialize)]
struct RagSearchPayload<'a> {
    query: &'a str,
    hits: &'a [RagHit],
}

pub fn format_results_json(query: &str, hits: &[RagHit]) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&RagSearchPayload { query, hits })
}

pub fn build_context(query: &str, hits: &[RagHit], max_chars: usize) -> RagContext {
    let max_chars = max_chars.clamp(1, MAX_CONTEXT_CHARS);
    let mut text = String::new();
    let mut selected = Vec::new();
    let mut truncated = false;

    for (index, hit) in hits.iter().enumerate() {
        let block = context_block(index + 1, hit);
        let current_chars = text.chars().count();
        if current_chars + block.chars().count() <= max_chars {
            text.push_str(&block);
            selected.push(hit.clone());
            continue;
        }

        let remaining = max_chars.saturating_sub(current_chars);
        if remaining > 0 {
            text.push_str(&truncate_to_budget(&block, remaining));
            selected.push(hit.clone());
        }
        truncated = true;
        break;
    }

    if text.chars().count() > max_chars {
        text = truncate_to_budget(&text, max_chars);
    }
    RagContext {
        query: query.to_string(),
        text,
        hits: selected,
        truncated,
    }
}

pub fn format_context(query: &str, hits: &[RagHit], max_chars: usize) -> String {
    let context = build_context(query, hits, max_chars);
    let mut output = format!(
        "== Graph-RAG context ==\nQuery: {}\nSources: {}\nTruncated: {}\n\n",
        context.query,
        context.hits.len(),
        context.truncated
    );
    output.push_str(&context.text);
    output
}

pub fn format_context_json(
    query: &str,
    hits: &[RagHit],
    max_chars: usize,
) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&build_context(query, hits, max_chars))
}

fn context_block(index: usize, hit: &RagHit) -> String {
    let heading = if hit.heading_path.is_empty() {
        hit.title.as_str()
    } else {
        hit.heading_path.as_str()
    };
    format!(
        "[{index}] {}:{}-{}\nHeading: {heading}\nScore: {:.3} (lexical {:.3}, graph {:.3}, vector {:.3})\nReasons: {}\n{}\n\n",
        hit.path,
        hit.start_line,
        hit.end_line,
        hit.score,
        hit.lexical_score,
        hit.graph_score,
        hit.vector_score,
        hit.reasons.join(", "),
        hit.excerpt
    )
}

fn open_database(path: Option<&str>) -> Result<Connection, RagError> {
    let root = resolve_root(path)?;
    let database_path = root.join(INDEX_DIR).join(INDEX_DB);
    if !database_path.is_file() {
        return Err(RagError::Invalid(format!(
            "wiki rag search requires an index at {}; run wiki index --path {} first.",
            database_path.display(),
            root.display()
        )));
    }
    Connection::open(&database_path).map_err(database_error)
}

fn ensure_embedding_cache_schema(connection: &Connection) -> Result<(), RagError> {
    let exists: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'chunk_embeddings'",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if exists == 0 {
        return Err(RagError::Invalid(
            "hash embedding retrieval requires an index with embedding cache support; run wiki index for this wiki first.".to_string(),
        ));
    }
    Ok(())
}

fn resolve_root(input: Option<&str>) -> Result<PathBuf, RagError> {
    let raw = input.unwrap_or(DEFAULT_WIKI_ROOT);
    if raw.trim().is_empty() {
        return Err(RagError::Invalid(
            "wiki rag search path must be a non-empty directory path.".to_string(),
        ));
    }
    let path = PathBuf::from(raw);
    if !path.exists() {
        return Err(RagError::Invalid(format!(
            "wiki rag search path does not exist: {}",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(RagError::Invalid(format!(
            "wiki rag search path is not a directory: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn load_fts_chunks(
    connection: &Connection,
    query: &str,
    limit: i64,
) -> Result<Vec<ChunkRecord>, RagError> {
    let fts_query = build_fts_query(query);
    let mut statement = connection
        .prepare(
            "SELECT c.id, c.document_id, d.path, d.title, c.heading_path,
                    c.start_line, c.end_line, c.text, c.text_hash
             FROM chunks_fts
             JOIN chunks c ON c.id = chunks_fts.chunk_id
             JOIN documents d ON d.id = c.document_id
             WHERE chunks_fts MATCH ?1
             ORDER BY bm25(chunks_fts), c.id
             LIMIT ?2",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(params![fts_query, limit], chunk_from_row)
        .map_err(database_error)?;
    rows.map(|row| row.map_err(database_error)).collect()
}

fn load_like_chunks(
    connection: &Connection,
    query: &str,
    limit: i64,
) -> Result<Vec<ChunkRecord>, RagError> {
    let pattern = format!("%{}%", query);
    let mut statement = connection
        .prepare(
            "SELECT c.id, c.document_id, d.path, d.title, c.heading_path,
                    c.start_line, c.end_line, c.text, c.text_hash
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE lower(c.text) LIKE lower(?1)
             ORDER BY c.document_id, c.ordinal
             LIMIT ?2",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(params![pattern, limit], chunk_from_row)
        .map_err(database_error)?;
    rows.map(|row| row.map_err(database_error)).collect()
}

fn load_direct_chunks(
    connection: &Connection,
    query: &str,
    regex_mode: bool,
    limit: i64,
) -> Result<Vec<(ChunkRecord, f64, String)>, RagError> {
    if regex_mode {
        return Ok(load_regex_chunks(connection, query, limit)?
            .into_iter()
            .map(|chunk| (chunk, 1.0, "regex".to_string()))
            .collect());
    }

    let fts_chunks = load_fts_chunks(connection, query, limit)?;
    if !fts_chunks.is_empty() {
        return Ok(fts_chunks
            .into_iter()
            .map(|chunk| (chunk, 1.0, "fts5".to_string()))
            .collect());
    }

    let like_chunks = load_like_chunks(connection, query, limit)?;
    if !like_chunks.is_empty() {
        return Ok(like_chunks
            .into_iter()
            .map(|chunk| (chunk, 0.82, "substring".to_string()))
            .collect());
    }

    let mut direct_chunks = Vec::new();
    let mut seen = BTreeSet::new();
    for term in query_terms(query) {
        let reason = format!("query-term: {term}");
        let term_chunks = load_fts_chunks(connection, &term, limit)?;
        let term_chunks = if term_chunks.is_empty() {
            load_like_chunks(connection, &term, limit)?
        } else {
            term_chunks
        };
        let boost = query_term_boost(&term);
        for chunk in term_chunks {
            if seen.insert((chunk.id.clone(), reason.clone())) {
                direct_chunks.push((chunk, boost, reason.clone()));
            }
        }
    }
    Ok(direct_chunks)
}

fn load_regex_chunks(
    connection: &Connection,
    query: &str,
    limit: i64,
) -> Result<Vec<ChunkRecord>, RagError> {
    let regex = RegexBuilder::new(query)
        .case_insensitive(true)
        .build()
        .map_err(|error| RagError::Invalid(format!("invalid regex query: {error}")))?;
    let mut statement = connection
        .prepare(
            "SELECT c.id, c.document_id, d.path, d.title, c.heading_path,
                    c.start_line, c.end_line, c.text, c.text_hash
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             ORDER BY c.document_id, c.ordinal",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([], chunk_from_row)
        .map_err(database_error)?;
    let mut matches = Vec::new();
    for row in rows {
        let chunk = row.map_err(database_error)?;
        if regex.is_match(&chunk.text) {
            matches.push(chunk);
            if matches.len() >= limit.max(0) as usize {
                break;
            }
        }
    }
    Ok(matches)
}

fn build_fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn query_runs(value: &str) -> Vec<String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum RunKind {
        Ascii,
        Cjk,
    }

    let mut runs = Vec::new();
    let mut current = String::new();
    let mut kind = None;

    for character in value.chars() {
        let next_kind = if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
            Some(RunKind::Ascii)
        } else if is_cjk(character) {
            Some(RunKind::Cjk)
        } else {
            None
        };

        match next_kind {
            Some(next_kind) if kind == Some(next_kind) => current.push(character),
            Some(next_kind) => {
                if !current.is_empty() {
                    runs.push(std::mem::take(&mut current));
                }
                current.push(character);
                kind = Some(next_kind);
            }
            None => {
                if !current.is_empty() {
                    runs.push(std::mem::take(&mut current));
                }
                kind = None;
            }
        }
    }

    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

fn query_terms(value: &str) -> Vec<String> {
    const ENGLISH_STOPWORDS: &[&str] = &[
        "a", "an", "and", "are", "about", "can", "could", "does", "for", "how", "i", "is", "me",
        "of", "on", "or", "please", "tell", "the", "to", "what", "when", "where", "which", "who",
        "why", "will", "would",
    ];
    const CJK_STOPWORDS: &[&str] = &[
        "请", "分析", "如何", "怎么", "怎样", "什么", "是否", "以及", "并且", "的", "了", "吗",
        "呢",
    ];

    let mut scored = Vec::<(String, i32)>::new();
    for raw in query_runs(value) {
        let lower = raw.to_lowercase();
        let characters: Vec<char> = raw.chars().collect();
        if characters.iter().all(|character| character.is_ascii()) {
            if ENGLISH_STOPWORDS.contains(&lower.as_str()) {
                continue;
            }
            let has_structure = characters
                .iter()
                .any(|character| character.is_ascii_digit() || matches!(character, '_' | '-'));
            if characters.len() >= 3 || has_structure || raw.chars().any(char::is_uppercase) {
                push_scored_term(&mut scored, lower, 1_000 + characters.len() as i32);
            }
            continue;
        }

        if characters.iter().all(|character| is_cjk(*character)) {
            if characters.len() >= 2 && characters.len() <= 8 {
                if !CJK_STOPWORDS.contains(&raw.as_str()) {
                    push_scored_term(
                        &mut scored,
                        raw.to_lowercase(),
                        220 + characters.len() as i32,
                    );
                }
            }
            for width in (2..=4).rev() {
                if characters.len() < width {
                    continue;
                }
                for start in 0..=characters.len() - width {
                    let term: String = characters[start..start + width].iter().collect();
                    if cjk_term_is_noise(&term, CJK_STOPWORDS) {
                        continue;
                    }
                    push_scored_term(&mut scored, term, 200 + width as i32);
                }
            }
        }
    }

    scored.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| right.0.chars().count().cmp(&left.0.chars().count()))
            .then_with(|| left.0.cmp(&right.0))
    });
    scored
        .into_iter()
        .take(MAX_QUERY_FALLBACK_TERMS)
        .map(|(term, _)| term)
        .collect()
}

fn push_scored_term(terms: &mut Vec<(String, i32)>, term: String, score: i32) {
    if let Some((_, existing_score)) = terms.iter_mut().find(|(existing, _)| existing == &term) {
        *existing_score = (*existing_score).max(score);
        return;
    }
    terms.push((term, score));
}

fn cjk_term_is_noise(term: &str, stopwords: &[&str]) -> bool {
    stopwords.contains(&term)
        || term.starts_with("请")
        || term.starts_with("如何")
        || term.starts_with("怎么")
        || term.starts_with("怎样")
        || term.starts_with("是否")
        || term.ends_with('的')
        || term.ends_with('了')
        || term.ends_with('吗')
        || term.ends_with('呢')
        || term.ends_with('并')
}

fn query_term_boost(term: &str) -> f64 {
    let length = term.chars().count() as f64;
    let base = if term.chars().any(is_cjk) { 0.44 } else { 0.52 };
    (base + (length - 2.0).max(0.0) * 0.035).min(0.90)
}

fn is_cjk(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{20000}'..='\u{2FA1F}'
    )
}

fn chunk_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChunkRecord> {
    Ok(ChunkRecord {
        id: row.get(0)?,
        document_id: row.get(1)?,
        path: row.get(2)?,
        title: row.get(3)?,
        heading_path: row.get(4)?,
        start_line: row.get::<_, i64>(5)?.max(0) as usize,
        end_line: row.get::<_, i64>(6)?.max(0) as usize,
        text: row.get(7)?,
        text_hash: row.get(8)?,
    })
}

fn add_candidate(
    candidates: &mut BTreeMap<String, Candidate>,
    chunk: &ChunkRecord,
    boost: f64,
    reason: String,
    kind: ScoreKind,
) {
    let candidate = candidates
        .entry(chunk.id.clone())
        .or_insert_with(|| Candidate {
            chunk: chunk.clone(),
            lexical_score: 0.0,
            graph_score: 0.0,
            vector_score: 0.0,
            provenance: BTreeMap::new(),
        });
    if candidate.provenance.insert(reason, (kind, boost)).is_none() {
        match kind {
            ScoreKind::Lexical => candidate.lexical_score = candidate.lexical_score.max(boost),
            ScoreKind::Graph => candidate.graph_score = (candidate.graph_score + boost).min(0.60),
            ScoreKind::Vector => candidate.vector_score = candidate.vector_score.max(boost),
        }
    }
}

fn load_page_graph(connection: &Connection) -> Result<PageGraph, RagError> {
    let mut document_paths = BTreeMap::new();
    let mut statement = connection
        .prepare("SELECT id, path FROM documents")
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_error)?;
    for row in rows {
        let (id, path) = row.map_err(database_error)?;
        document_paths.insert(canonical_path(&path), id);
    }
    drop(statement);

    let mut graph = PageGraph::default();
    let mut statement = connection
        .prepare("SELECT source_document_id, target_path FROM wiki_links")
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_error)?;
    for row in rows {
        let (source, target_path) = row.map_err(database_error)?;
        let Some(target) = document_paths.get(&canonical_path(&target_path)) else {
            continue;
        };
        if source == *target {
            continue;
        }
        graph
            .outgoing
            .entry(source.clone())
            .or_default()
            .push(target.clone());
        graph
            .incoming
            .entry(target.clone())
            .or_default()
            .push(source);
    }
    for neighbors in graph.outgoing.values_mut() {
        neighbors.sort();
        neighbors.dedup();
    }
    for neighbors in graph.incoming.values_mut() {
        neighbors.sort();
        neighbors.dedup();
    }
    Ok(graph)
}

fn page_neighborhood(
    graph: &PageGraph,
    seeds: &BTreeSet<String>,
    max_depth: usize,
) -> BTreeMap<String, usize> {
    let mut distances = BTreeMap::new();
    let mut queue = VecDeque::new();
    for seed in seeds {
        distances.insert(seed.clone(), 0);
        queue.push_back((seed.clone(), 0));
    }
    while let Some((current, distance)) = queue.pop_front() {
        if distance >= max_depth {
            continue;
        }
        let mut adjacent = Vec::new();
        adjacent.extend(graph.outgoing.get(&current).into_iter().flatten().cloned());
        adjacent.extend(graph.incoming.get(&current).into_iter().flatten().cloned());
        adjacent.sort();
        adjacent.dedup();
        for next in adjacent {
            if distances.contains_key(&next) {
                continue;
            }
            let next_distance = distance + 1;
            distances.insert(next.clone(), next_distance);
            queue.push_back((next, next_distance));
        }
    }
    distances
}

fn load_document_chunks(
    connection: &Connection,
    document_id: &str,
    limit: usize,
) -> Result<Vec<ChunkRecord>, RagError> {
    let mut statement = connection
        .prepare(
            "SELECT c.id, c.document_id, d.path, d.title, c.heading_path,
                    c.start_line, c.end_line, c.text, c.text_hash
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE c.document_id = ?1
             ORDER BY c.ordinal
             LIMIT ?2",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(params![document_id, limit as i64], chunk_from_row)
        .map_err(database_error)?;
    rows.map(|row| row.map_err(database_error)).collect()
}

fn load_entity_ids(
    connection: &Connection,
    chunk_ids: &BTreeSet<String>,
) -> Result<BTreeSet<String>, RagError> {
    let mut entity_ids = BTreeSet::new();
    let mut statement = connection
        .prepare("SELECT DISTINCT entity_id FROM entity_mentions WHERE chunk_id = ?1")
        .map_err(database_error)?;
    for chunk_id in chunk_ids {
        let rows = statement
            .query_map(params![chunk_id], |row| row.get::<_, String>(0))
            .map_err(database_error)?;
        for row in rows {
            entity_ids.insert(row.map_err(database_error)?);
        }
    }
    Ok(entity_ids)
}

fn load_entity_scopes(
    connection: &Connection,
    seed_entity_ids: &BTreeSet<String>,
) -> Result<BTreeMap<String, (String, f64)>, RagError> {
    let mut names = BTreeMap::new();
    let mut name_statement = connection
        .prepare("SELECT id, canonical_name FROM entities")
        .map_err(database_error)?;
    let name_rows = name_statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_error)?;
    for row in name_rows {
        let (id, name) = row.map_err(database_error)?;
        names.insert(id, name);
    }
    drop(name_statement);

    let mut scopes: BTreeMap<String, (String, f64)> = BTreeMap::new();
    for entity_id in seed_entity_ids {
        let name = names
            .get(entity_id)
            .cloned()
            .unwrap_or_else(|| entity_id.clone());
        scopes
            .entry(entity_id.clone())
            .or_insert_with(|| (format!("entity-mention: {name}"), 0.28));
    }

    let mut relation_statement = connection
        .prepare(
            "SELECT subject_entity_id, predicate, object_entity_id
             FROM relations
             ORDER BY subject_entity_id, predicate, object_entity_id",
        )
        .map_err(database_error)?;
    let relation_rows = relation_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(database_error)?;
    let relations: Vec<(String, String, String)> = relation_rows
        .map(|row| row.map_err(database_error))
        .collect::<Result<_, _>>()?;
    for (subject, predicate, object) in relations {
        let (seed, other) = if seed_entity_ids.contains(&subject) {
            (&subject, &object)
        } else if seed_entity_ids.contains(&object) {
            (&object, &subject)
        } else {
            continue;
        };
        if seed_entity_ids.contains(other) {
            continue;
        }
        let other_name = names.get(other).cloned().unwrap_or_else(|| other.clone());
        scopes.entry(other.clone()).or_insert_with(|| {
            (
                format!(
                    "candidate relation from {}: {predicate} -> {other_name}",
                    names.get(seed).map(String::as_str).unwrap_or(seed.as_str())
                ),
                0.20,
            )
        });
    }
    Ok(scopes)
}

fn load_entity_chunks(
    connection: &Connection,
    entity_id: &str,
    limit: usize,
) -> Result<Vec<ChunkRecord>, RagError> {
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT c.id, c.document_id, d.path, d.title, c.heading_path,
                    c.start_line, c.end_line, c.text, c.text_hash
             FROM entity_mentions m
             JOIN chunks c ON c.id = m.chunk_id
             JOIN documents d ON d.id = c.document_id
             WHERE m.entity_id = ?1
             ORDER BY c.document_id, c.ordinal
             LIMIT ?2",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(params![entity_id, limit as i64], chunk_from_row)
        .map_err(database_error)?;
    rows.map(|row| row.map_err(database_error)).collect()
}

fn load_vector_chunks(
    connection: &Connection,
    provider: &dyn EmbeddingProvider,
    query: &str,
    limit: usize,
) -> Result<Vec<(ChunkRecord, f64)>, RagError> {
    let descriptor = provider.descriptor();
    let query_vector = provider.embed(query).map_err(embedding_error)?;
    validate_vector(&query_vector, descriptor.dimensions)?;

    let mut statement = connection
        .prepare(
            "SELECT c.id, c.document_id, d.path, d.title, c.heading_path,
                    c.start_line, c.end_line, c.text, c.text_hash
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             ORDER BY c.id
             LIMIT ?1",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(params![MAX_VECTOR_SCAN as i64], chunk_from_row)
        .map_err(database_error)?;
    let chunks: Vec<ChunkRecord> = rows
        .map(|row| row.map_err(database_error))
        .collect::<Result<_, _>>()?;
    drop(statement);

    let mut resolved: Vec<Option<Vec<f32>>> = vec![None; chunks.len()];
    let mut missing_indices = Vec::new();
    let mut missing_inputs = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let cached: Option<(i64, String, String)> = connection
            .query_row(
                "SELECT dimensions, text_hash, vector_json
                 FROM chunk_embeddings
                 WHERE chunk_id = ?1 AND provider = ?2 AND model = ?3",
                params![
                    chunk.id,
                    descriptor.provider.as_str(),
                    descriptor.model.as_str()
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(database_error)?;
        if let Some((dimensions, text_hash, vector_json)) = cached {
            if dimensions == descriptor.dimensions as i64 && text_hash == chunk.text_hash {
                if let Some(vector) = parse_vector(&vector_json, descriptor.dimensions) {
                    resolved[index] = Some(vector);
                    continue;
                }
            }
        }
        missing_indices.push(index);
        missing_inputs.push(chunk.text.clone());
    }

    if !missing_inputs.is_empty() {
        let generated: Vec<Vec<f32>> = provider
            .embed_batch(&missing_inputs)
            .map_err(embedding_error)?;
        if generated.len() != missing_indices.len() {
            return Err(RagError::Invalid(format!(
                "embedding provider returned {} vectors for {} chunks.",
                generated.len(),
                missing_indices.len()
            )));
        }
        for (index, vector) in missing_indices.into_iter().zip(generated) {
            validate_vector(&vector, descriptor.dimensions)?;
            let vector_json = serde_json::to_string(&vector)
                .map_err(|error| RagError::Database(error.to_string()))?;
            connection
                .execute(
                    "INSERT INTO chunk_embeddings
                         (chunk_id, provider, model, dimensions, text_hash, vector_json, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(chunk_id, provider, model) DO UPDATE SET
                         dimensions = excluded.dimensions,
                         text_hash = excluded.text_hash,
                         vector_json = excluded.vector_json,
                         created_at = excluded.created_at",
                    params![
                        chunks[index].id,
                        descriptor.provider.as_str(),
                        descriptor.model.as_str(),
                        descriptor.dimensions as i64,
                        chunks[index].text_hash,
                        vector_json,
                        unix_now(),
                    ],
                )
                .map_err(database_error)?;
            resolved[index] = Some(vector);
        }
    }

    let mut scored = Vec::new();
    for (chunk, vector) in chunks.into_iter().zip(resolved) {
        let Some(vector) = vector else { continue };
        let Some(similarity) = embedding::cosine_similarity(&query_vector, &vector) else {
            continue;
        };
        scored.push((chunk, similarity.max(0.0)));
    }
    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.id.cmp(&right.0.id))
    });
    scored.truncate(limit);
    Ok(scored)
}

fn parse_vector(value: &str, dimensions: usize) -> Option<Vec<f32>> {
    let vector: Vec<f32> = serde_json::from_str(value).ok()?;
    if vector.len() != dimensions || vector.iter().any(|value| !value.is_finite()) {
        return None;
    }
    Some(vector)
}

fn validate_vector(vector: &[f32], dimensions: usize) -> Result<(), RagError> {
    if vector.len() != dimensions || vector.iter().any(|value| !value.is_finite()) {
        return Err(RagError::Invalid(format!(
            "embedding provider returned an invalid vector: expected {dimensions} finite values, got {}.",
            vector.len()
        )));
    }
    Ok(())
}

fn canonical_path(value: &str) -> String {
    let mut path = value.trim().replace('\\', "/");
    while let Some(stripped) = path.strip_prefix("./") {
        path = stripped.to_string();
    }
    if path.to_ascii_lowercase().ends_with(".md") {
        path.truncate(path.len().saturating_sub(3));
    }
    path
}

fn make_excerpt(text: &str) -> String {
    let one_line = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    truncate_text(&one_line, EXCERPT_CHARS)
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut result: String = text.chars().take(max_chars).collect();
    result.push_str("...");
    result
}

fn truncate_to_budget(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return text.chars().take(max_chars).collect();
    }
    let mut result: String = text.chars().take(max_chars - 3).collect();
    result.push_str("...");
    result
}

fn database_error(error: rusqlite::Error) -> RagError {
    RagError::Database(error.to_string())
}

fn embedding_error(error: embedding::EmbeddingError) -> RagError {
    RagError::Database(format!("embedding provider error: {error}"))
}

fn finite_non_negative(value: f64) -> f64 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{index, IndexOptions, DEFAULT_CHUNK_CHARS};
    use std::fs;
    use std::path::Path;

    fn options(root: &Path, query: &str, limit: usize, depth: usize) -> RagSearchOptions {
        RagSearchOptions {
            path: Some(root.to_string_lossy().into_owned()),
            query: query.to_string(),
            regex: false,
            limit,
            depth,
            embedding_mode: EmbeddingMode::None,
            weights: RagWeights::default(),
        }
    }

    #[test]
    fn search_combines_fts_page_links_and_entity_mentions() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("notes")).unwrap();
        let marker = '\x60';
        fs::write(
            root.join("notes").join("service.md"),
            format!(
                "# Service\n\n{marker}RootSignal{marker} uses {marker}SharedEntity{marker}. See [[notes/guide]].\n"
            ),
        )
        .unwrap();
        fs::write(
            root.join("notes").join("guide.md"),
            "# Guide\n\nThis page is linked from the service page.\n",
        )
        .unwrap();
        fs::write(
            root.join("notes").join("related.md"),
            format!(
                "# Related\n\n{marker}SharedEntity{marker} is described here without the query phrase.\n"
            ),
        )
        .unwrap();
        index(&IndexOptions {
            path: Some(root.to_string_lossy().into_owned()),
            rebuild: false,
            chunk_chars: DEFAULT_CHUNK_CHARS,
        })
        .unwrap();

        let hits = search(&options(root, "RootSignal", 10, 1)).unwrap();
        assert!(hits.iter().any(|hit| {
            hit.path == "notes/service.md" && hit.reasons.iter().any(|r| r == "fts5")
        }));
        assert!(hits.iter().any(|hit| {
            hit.path == "notes/guide.md"
                && hit
                    .reasons
                    .iter()
                    .any(|reason| reason == "page-link depth 1")
        }));
        assert!(hits.iter().any(|hit| {
            hit.path == "notes/related.md"
                && hit
                    .reasons
                    .iter()
                    .any(|reason| reason == "entity-mention: SharedEntity")
        }));
    }

    #[test]
    fn natural_language_query_falls_back_to_specific_terms() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(
            root.join("harness.md"),
            "# Harness\n\nThe wiki harness automatically updates the index and returns task evidence.\n",
        )
        .unwrap();
        index(&IndexOptions {
            path: Some(root.to_string_lossy().into_owned()),
            rebuild: false,
            chunk_chars: DEFAULT_CHUNK_CHARS,
        })
        .unwrap();

        let hits = search(&options(
            root,
            "请分析 wiki harness 如何自动更新索引并返回任务证据",
            8,
            0,
        ))
        .unwrap();
        let harness_hit = hits
            .iter()
            .find(|hit| hit.path == "harness.md")
            .expect("natural language query should retrieve the harness page");
        assert!(harness_hit
            .reasons
            .iter()
            .any(|reason| reason == "query-term: harness"));
        assert!(harness_hit.lexical_score > 0.0);
    }

    #[test]
    fn regex_query_retrieves_alternative_identifiers_with_regex_provenance() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(
            root.join("station.md"),
            "# Station\n\nEventStation transitions the role into STATION_ING.\n",
        )
        .unwrap();
        index(&IndexOptions {
            path: Some(root.to_string_lossy().into_owned()),
            rebuild: false,
            chunk_chars: DEFAULT_CHUNK_CHARS,
        })
        .unwrap();

        let mut search_options = options(root, r"EventStation|STATION_ING", 8, 0);
        search_options.regex = true;
        let hit = search(&search_options)
            .unwrap()
            .into_iter()
            .find(|hit| hit.path == "station.md")
            .expect("regex query should retrieve the station page");
        assert!(hit.reasons.iter().any(|reason| reason == "regex"));
        assert!(hit
            .provenance
            .iter()
            .any(|item| item.kind == "lexical" && item.source == "regex"));
    }

    #[test]
    fn regex_query_rejects_invalid_patterns() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("page.md"), "# Page\n\nA value.\n").unwrap();
        index(&IndexOptions {
            path: Some(root.to_string_lossy().into_owned()),
            rebuild: false,
            chunk_chars: DEFAULT_CHUNK_CHARS,
        })
        .unwrap();
        let error = search(&RagSearchOptions {
            path: Some(root.to_string_lossy().into_owned()),
            query: "(unclosed".to_string(),
            regex: true,
            limit: 8,
            depth: 0,
            embedding_mode: EmbeddingMode::None,
            weights: RagWeights::default(),
        })
        .unwrap_err();
        assert!(error.to_string().contains("invalid regex query"));
    }

    #[test]
    fn query_terms_prioritize_technical_identifiers_and_support_chinese() {
        let terms = query_terms("请分析 wiki harness 如何自动更新索引并返回任务证据");
        assert_eq!(terms.first().map(String::as_str), Some("harness"));
        assert!(terms.iter().any(|term| term == "wiki"));
        assert!(terms.iter().any(|term| term.chars().count() >= 2));
        assert!(terms.len() <= MAX_QUERY_FALLBACK_TERMS);
    }

    #[test]
    fn query_terms_split_mixed_ascii_and_cjk_runs() {
        let terms = query_terms("index没有命中");
        assert!(terms.iter().any(|term| term == "index"));
        assert!(terms.iter().any(|term| term == "没有命中"));
    }

    #[test]
    fn hash_embedding_cache_is_lazy_and_tracks_chunk_hash() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(
            root.join("page.md"),
            "# Page\n\nRoleService handles authorization.\n",
        )
        .unwrap();
        index(&IndexOptions {
            path: Some(root.to_string_lossy().into_owned()),
            rebuild: false,
            chunk_chars: DEFAULT_CHUNK_CHARS,
        })
        .unwrap();

        let mut search_options = options(root, "RoleService", 8, 0);
        search_options.embedding_mode = EmbeddingMode::Hash;
        let first_hits = search(&search_options).unwrap();
        assert!(!first_hits.is_empty());
        assert!(first_hits.iter().any(|hit| {
            hit.vector_score > 0.0
                && hit
                    .provenance
                    .iter()
                    .any(|item| item.kind == "vector" && item.source == "local-hash:token-ngram-v1")
        }));

        let database_path = root.join(INDEX_DIR).join(INDEX_DB);
        let connection = Connection::open(&database_path).unwrap();
        let initial_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(initial_count > 0);
        let initial_hashes: (String, String) = connection
            .query_row(
                "SELECT c.text_hash, e.text_hash
                 FROM chunks c
                 JOIN chunk_embeddings e ON e.chunk_id = c.id
                 WHERE e.provider = 'local-hash'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(initial_hashes.0, initial_hashes.1);
        drop(connection);

        let second_hits = search(&search_options).unwrap();
        assert_eq!(second_hits, first_hits);
        let connection = Connection::open(&database_path).unwrap();
        let second_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(second_count, initial_count);
        drop(connection);

        fs::write(
            root.join("page.md"),
            "# Page\n\nRoleService handles a changed authorization policy.\n",
        )
        .unwrap();
        index(&IndexOptions {
            path: Some(root.to_string_lossy().into_owned()),
            rebuild: false,
            chunk_chars: DEFAULT_CHUNK_CHARS,
        })
        .unwrap();
        let connection = Connection::open(&database_path).unwrap();
        let after_reindex_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM chunk_embeddings", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(after_reindex_count, 0);
        drop(connection);

        search(&search_options).unwrap();
        let connection = Connection::open(database_path).unwrap();
        let refreshed_hashes: (String, String) = connection
            .query_row(
                "SELECT c.text_hash, e.text_hash
                 FROM chunks c
                 JOIN chunk_embeddings e ON e.chunk_id = c.id
                 WHERE e.provider = 'local-hash'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(refreshed_hashes.0, refreshed_hashes.1);
        assert_ne!(initial_hashes.0, refreshed_hashes.0);
    }

    #[test]
    fn search_rejects_empty_query() {
        let error = search(&RagSearchOptions {
            path: None,
            query: "  ".to_string(),
            regex: false,
            limit: 8,
            depth: 1,
            embedding_mode: EmbeddingMode::None,
            weights: RagWeights::default(),
        })
        .unwrap_err();
        assert!(error.to_string().contains("non-empty"));
    }

    fn sample_hit(path: &str, excerpt: &str) -> RagHit {
        RagHit {
            chunk_id: format!("chunk:{path}"),
            path: path.to_string(),
            title: "Service".to_string(),
            heading_path: "Service > API".to_string(),
            start_line: 3,
            end_line: 8,
            lexical_score: 1.0,
            graph_score: 0.0,
            vector_score: 0.0,
            score: 1.0,
            reasons: vec!["fts5".to_string()],
            provenance: vec![RagProvenance {
                kind: "lexical".to_string(),
                source: "fts5".to_string(),
                score: 1.0,
            }],
            excerpt: excerpt.to_string(),
        }
    }

    #[test]
    fn context_is_bounded_and_keeps_source_metadata() {
        let hits = vec![
            sample_hit("notes/a.md", "RoleService handles requests."),
            sample_hit("notes/b.md", "EquipBagSize is checked here."),
        ];
        let context = build_context("RoleService", &hits, 180);
        assert!(context.text.chars().count() <= 180);
        assert!(context.truncated);
        assert!(context.text.contains("notes/a.md"));
        assert_eq!(context.query, "RoleService");
        assert!(!context.hits.is_empty());
    }

    #[test]
    fn structured_json_contains_query_and_hits() {
        let hits = vec![sample_hit("notes/a.md", "RoleService handles requests.")];
        let json = format_results_json("RoleService", &hits).unwrap();
        assert!(json.contains("\"query\": \"RoleService\""));
        assert!(json.contains("\"path\": \"notes/a.md\""));
        let context_json = format_context_json("RoleService", &hits, 2_000).unwrap();
        assert!(context_json.contains("\"truncated\": false"));
    }
}
