//! wiki index — build a local, incremental SQLite index for Markdown pages.
//!
//! This is the storage foundation for the later graph and RAG layers. It keeps
//! the source files authoritative: the .wiki/index.sqlite database is a
//! rebuildable cache containing document metadata, heading-aware chunks,
//! explicit [[wiki-links]], and an FTS5 table for future retrieval.

use crate::shared::{display_path, DEFAULT_WIKI_ROOT};
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

const INDEX_DIR: &str = ".wiki";
const INDEX_DB: &str = "index.sqlite";
const SCHEMA_VERSION: i32 = 3;
const INDEXER_VERSION: &str = "wiki-index-v5";
pub const DEFAULT_CHUNK_CHARS: usize = 1_200;
const MAX_CHUNK_CHARS: usize = 8_000;
const MAX_ENTITIES_PER_CHUNK: usize = 32;
const IGNORED_INLINE_IDENTIFIERS: &[&str] = &[
    "chunks_fts",
    "co_occurs_in_chunk",
    "entities",
    "entity_mentions",
    "relation_evidence",
    "relations",
    "wiki_links",
];

/// Options for an incremental index build.
#[derive(Clone, Debug)]
pub struct IndexOptions {
    /// Wiki root. Defaults to docs/.
    pub path: Option<String>,
    /// Delete indexed rows before scanning. The database itself remains in
    /// place so the operation is recoverable and does not affect Markdown.
    pub rebuild: bool,
    /// Maximum approximate character count per chunk.
    pub chunk_chars: usize,
}

/// Summary returned after a successful index build.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexStats {
    pub database_path: PathBuf,
    pub scanned: usize,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub chunks: usize,
    pub links: usize,
    pub entities: usize,
    pub mentions: usize,
    pub relations: usize,
}

#[derive(Debug)]
pub enum IndexError {
    Invalid(String),
    Io(String),
    Database(String),
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Io(message) | Self::Database(message) => {
                f.write_str(message)
            }
        }
    }
}

#[derive(Clone, Debug)]
struct MarkdownPage {
    relative_path: String,
    id: String,
    sha256: String,
    title: String,
    category: Option<String>,
    modified_at: i64,
    text: String,
}

#[derive(Clone, Debug)]
struct Chunk {
    ordinal: usize,
    heading_path: String,
    start_line: usize,
    end_line: usize,
    text: String,
    text_hash: String,
}

#[derive(Clone, Debug)]
struct WikiLink {
    target_path: String,
    link_text: Option<String>,
}

#[derive(Clone, Debug)]
struct EntityCandidate {
    canonical_name: String,
    entity_type: &'static str,
    surface: String,
    start_pos: usize,
    end_pos: usize,
    start_line: usize,
    end_line: usize,
    confidence: f64,
    extractor: &'static str,
}

struct EntityPatterns {
    rust_function: Regex,
    go_function: Regex,
    type_declaration: Regex,
    module_declaration: Regex,
    import: Regex,
    path: Regex,
    inline: Regex,
    config: Regex,
}

fn entity_patterns() -> &'static EntityPatterns {
    static PATTERNS: OnceLock<EntityPatterns> = OnceLock::new();
    PATTERNS.get_or_init(|| EntityPatterns {
        rust_function: Regex::new(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")
            .expect("valid Rust function regex"),
        go_function: Regex::new(r"\bfunc\s+(?:\([^)]*\)\s*)?([A-Za-z_][A-Za-z0-9_]*)")
            .expect("valid Go function regex"),
        type_declaration: Regex::new(
            r"\b(?:struct|enum|trait|class|interface|type|impl)\s+([A-Za-z_][A-Za-z0-9_]*)",
        )
        .expect("valid type regex"),
        module_declaration: Regex::new(r"\b(?:package|mod)\s+([A-Za-z_][A-Za-z0-9_]*)")
            .expect("valid module regex"),
        import: Regex::new(r"\b(?:use|import)\s+([A-Za-z_][A-Za-z0-9_:/.-]*)")
            .expect("valid import regex"),
        path: Regex::new(
            r"(?i)(?:[A-Za-z0-9_.-]+/)+[A-Za-z0-9_.-]+\.(?:rs|go|toml|yaml|yml|json|md|csv)\b",
        )
        .expect("valid path regex"),
        inline: Regex::new(r"\x60([^\x60\n]+)\x60").expect("valid inline code regex"),
        config: Regex::new(r"[A-Za-z_][A-Za-z0-9_-]*\.[A-Za-z_][A-Za-z0-9_.-]*")
            .expect("valid config regex"),
    })
}

fn wiki_link_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"\[\[([^\]|]+)(?:\|([^\]]+))?\]\]").expect("valid wiki-link regex")
    })
}

/// Build or update the local index. Markdown files remain the source of truth.
pub fn index(options: &IndexOptions) -> Result<IndexStats, IndexError> {
    let root = resolve_root(options.path.as_deref())?;
    let chunk_chars = options.chunk_chars.clamp(1, MAX_CHUNK_CHARS);
    let database_path = root.join(INDEX_DIR).join(INDEX_DB);
    fs::create_dir_all(database_path.parent().expect("index database has a parent"))
        .map_err(|error| IndexError::Io(format!("cannot create index directory: {error}")))?;

    let pages = scan_pages(&root)?;
    let connection = Connection::open(&database_path).map_err(|error| {
        IndexError::Database(format!("cannot open {}: {error}", database_path.display()))
    })?;
    let indexer_changed = initialize_schema(&connection)?;
    let force_rebuild = options.rebuild || indexer_changed;

    let mut existing = load_existing_documents(&connection)?;
    let old_count = existing.len();
    let transaction = connection.unchecked_transaction().map_err(database_error)?;

    if force_rebuild {
        clear_index(&transaction)?;
        existing.clear();
    }

    let mut stats = IndexStats {
        database_path,
        scanned: pages.len(),
        added: 0,
        updated: 0,
        unchanged: 0,
        removed: if force_rebuild { old_count } else { 0 },
        chunks: 0,
        links: 0,
        entities: 0,
        mentions: 0,
        relations: 0,
    };
    let mut seen = BTreeSet::new();

    for page in &pages {
        seen.insert(page.relative_path.clone());
        match existing.get(&page.relative_path) {
            Some((_, old_hash)) if old_hash == &page.sha256 && !force_rebuild => {
                stats.unchanged += 1;
                stats.chunks += count_chunks(&transaction, &page.id)?;
                stats.links += count_links(&transaction, &page.id)?;
            }
            Some(_) => {
                replace_page(&transaction, page, chunk_chars, true)?;
                stats.updated += 1;
                stats.chunks += count_chunks(&transaction, &page.id)?;
                stats.links += count_links(&transaction, &page.id)?;
            }
            None => {
                replace_page(&transaction, page, chunk_chars, false)?;
                stats.added += 1;
                stats.chunks += count_chunks(&transaction, &page.id)?;
                stats.links += count_links(&transaction, &page.id)?;
            }
        }
    }

    for (relative_path, (document_id, _)) in existing {
        if !seen.contains(&relative_path) {
            delete_page(&transaction, &document_id)?;
            if !force_rebuild {
                stats.removed += 1;
            }
        }
    }

    cleanup_orphaned_entities(&transaction)?;
    stats.entities = count_all(&transaction, "entities")?;
    stats.mentions = count_all(&transaction, "entity_mentions")?;
    stats.relations = count_all(&transaction, "relations")?;
    transaction
        .execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params!["indexer", INDEXER_VERSION],
        )
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)?;
    Ok(stats)
}

fn resolve_root(input: Option<&str>) -> Result<PathBuf, IndexError> {
    let raw = input.unwrap_or(DEFAULT_WIKI_ROOT);
    if raw.trim().is_empty() {
        return Err(IndexError::Invalid(
            "wiki index path must be a non-empty directory path.".to_string(),
        ));
    }
    let path = PathBuf::from(raw);
    if !path.exists() {
        return Err(IndexError::Invalid(format!(
            "wiki index path does not exist: {}",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(IndexError::Invalid(format!(
            "wiki index path is not a directory: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn scan_pages(root: &Path) -> Result<Vec<MarkdownPage>, IndexError> {
    let mut paths = Vec::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }
            let name = entry.file_name();
            name != INDEX_DIR
                && name != ".git"
                && name != "node_modules"
                && name != "target"
                && name != ".venv"
        });

    for entry in walker {
        let entry = entry.map_err(|error| IndexError::Io(format!("cannot scan wiki: {error}")))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let is_markdown = entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
        if is_markdown {
            paths.push(entry.path().to_path_buf());
        }
    }
    paths.sort();

    paths
        .into_iter()
        .map(|path| read_page(root, &path))
        .collect()
}

fn read_page(root: &Path, path: &Path) -> Result<MarkdownPage, IndexError> {
    let bytes = fs::read(path)
        .map_err(|error| IndexError::Io(format!("cannot read {}: {error}", path.display())))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let relative = path
        .strip_prefix(root)
        .map_err(|_| IndexError::Io(format!("cannot relativize {}", path.display())))?;
    let relative_path = display_path(relative);
    let id = format!("doc:{relative_path}");
    let metadata = fs::metadata(path)
        .map_err(|error| IndexError::Io(format!("cannot inspect {}: {error}", path.display())))?;
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0);
    let title = first_heading(&text).unwrap_or_else(|| {
        path.file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("untitled")
            .to_string()
    });
    let category = relative
        .components()
        .next()
        .and_then(|component| component.as_os_str().to_str())
        .map(ToOwned::to_owned);

    Ok(MarkdownPage {
        relative_path,
        id,
        sha256: hash_bytes(&bytes),
        title,
        category,
        modified_at,
        text,
    })
}

fn initialize_schema(connection: &Connection) -> Result<bool, IndexError> {
    let current: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(database_error)?;
    if current > SCHEMA_VERSION {
        return Err(IndexError::Database(format!(
            "index schema version {current} is newer than this binary supports ({}).",
            SCHEMA_VERSION
        )));
    }
    connection
        .execute_batch(
            "
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS documents (
                id TEXT PRIMARY KEY NOT NULL,
                path TEXT NOT NULL UNIQUE,
                sha256 TEXT NOT NULL,
                title TEXT NOT NULL,
                category TEXT,
                modified_at INTEGER NOT NULL,
                indexed_at INTEGER NOT NULL,
                status TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chunks (
                id TEXT PRIMARY KEY NOT NULL,
                document_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                heading_path TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                text TEXT NOT NULL,
                text_hash TEXT NOT NULL,
                FOREIGN KEY(document_id) REFERENCES documents(id) ON DELETE CASCADE,
                UNIQUE(document_id, ordinal)
            );
            CREATE TABLE IF NOT EXISTS wiki_links (
                source_document_id TEXT NOT NULL,
                target_path TEXT NOT NULL,
                link_text TEXT,
                link_kind TEXT NOT NULL,
                FOREIGN KEY(source_document_id) REFERENCES documents(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS entities (
                id TEXT PRIMARY KEY NOT NULL,
                canonical_name TEXT NOT NULL,
                entity_type TEXT NOT NULL,
                aliases_json TEXT NOT NULL,
                confidence REAL NOT NULL,
                status TEXT NOT NULL,
                UNIQUE(canonical_name, entity_type)
            );
            CREATE TABLE IF NOT EXISTS entity_mentions (
                id TEXT PRIMARY KEY NOT NULL,
                entity_id TEXT NOT NULL,
                chunk_id TEXT NOT NULL,
                surface TEXT NOT NULL,
                start_pos INTEGER NOT NULL,
                end_pos INTEGER NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                extractor TEXT NOT NULL,
                confidence REAL NOT NULL,
                FOREIGN KEY(entity_id) REFERENCES entities(id) ON DELETE CASCADE,
                FOREIGN KEY(chunk_id) REFERENCES chunks(id) ON DELETE CASCADE,
                UNIQUE(entity_id, chunk_id, start_pos, end_pos)
            );
            CREATE TABLE IF NOT EXISTS relations (
                id TEXT PRIMARY KEY NOT NULL,
                subject_entity_id TEXT NOT NULL,
                predicate TEXT NOT NULL,
                object_entity_id TEXT NOT NULL,
                confidence REAL NOT NULL,
                extractor TEXT NOT NULL,
                status TEXT NOT NULL,
                FOREIGN KEY(subject_entity_id) REFERENCES entities(id) ON DELETE CASCADE,
                FOREIGN KEY(object_entity_id) REFERENCES entities(id) ON DELETE CASCADE,
                UNIQUE(subject_entity_id, predicate, object_entity_id)
            );
            CREATE TABLE IF NOT EXISTS relation_evidence (
                relation_id TEXT NOT NULL,
                chunk_id TEXT NOT NULL,
                quote TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                PRIMARY KEY(relation_id, chunk_id),
                FOREIGN KEY(relation_id) REFERENCES relations(id) ON DELETE CASCADE,
                FOREIGN KEY(chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
            );
            CREATE TABLE IF NOT EXISTS chunk_embeddings (
                chunk_id TEXT NOT NULL,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                dimensions INTEGER NOT NULL,
                text_hash TEXT NOT NULL,
                vector_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY(chunk_id, provider, model),
                FOREIGN KEY(chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_chunks_document_id ON chunks(document_id);
            CREATE INDEX IF NOT EXISTS idx_wiki_links_source ON wiki_links(source_document_id);
            CREATE INDEX IF NOT EXISTS idx_wiki_links_target ON wiki_links(target_path);
            CREATE INDEX IF NOT EXISTS idx_entity_mentions_entity ON entity_mentions(entity_id);
            CREATE INDEX IF NOT EXISTS idx_entity_mentions_chunk ON entity_mentions(chunk_id);
            CREATE INDEX IF NOT EXISTS idx_relations_subject ON relations(subject_entity_id);
            CREATE INDEX IF NOT EXISTS idx_relations_object ON relations(object_entity_id);
            CREATE INDEX IF NOT EXISTS idx_relation_evidence_chunk ON relation_evidence(chunk_id);
            CREATE INDEX IF NOT EXISTS idx_chunk_embeddings_provider ON chunk_embeddings(provider, model);
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                chunk_id UNINDEXED,
                text,
                heading_path
            );
            ",
        )
        .map_err(database_error)?;
    let previous_indexer: Option<String> = connection
        .query_row("SELECT value FROM meta WHERE key = 'indexer'", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(database_error)?;
    connection
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(database_error)?;
    set_meta(connection, "schema_version", &SCHEMA_VERSION.to_string())?;
    Ok(previous_indexer.as_deref() != Some(INDEXER_VERSION))
}

fn set_meta(connection: &Connection, key: &str, value: &str) -> Result<(), IndexError> {
    connection
        .execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(database_error)?;
    Ok(())
}

fn load_existing_documents(
    connection: &Connection,
) -> Result<BTreeMap<String, (String, String)>, IndexError> {
    let mut statement = connection
        .prepare("SELECT path, id, sha256 FROM documents")
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(database_error)?;
    let mut documents = BTreeMap::new();
    for row in rows {
        let (path, id, sha256) = row.map_err(database_error)?;
        documents.insert(path, (id, sha256));
    }
    Ok(documents)
}

fn clear_index(transaction: &Transaction<'_>) -> Result<(), IndexError> {
    transaction
        .execute_batch(
            "DELETE FROM chunks_fts;
             DELETE FROM relation_evidence;
             DELETE FROM relations;
             DELETE FROM entity_mentions;
             DELETE FROM entities;
             DELETE FROM chunk_embeddings;
             DELETE FROM wiki_links;
             DELETE FROM chunks;
             DELETE FROM documents;",
        )
        .map_err(database_error)?;
    Ok(())
}

fn replace_page(
    transaction: &Transaction<'_>,
    page: &MarkdownPage,
    chunk_chars: usize,
    remove_existing: bool,
) -> Result<(), IndexError> {
    if remove_existing {
        delete_page(transaction, &page.id)?;
    }
    transaction
        .execute(
            "INSERT INTO documents
             (id, path, sha256, title, category, modified_at, indexed_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'indexed')",
            params![
                page.id,
                page.relative_path,
                page.sha256,
                page.title,
                page.category,
                page.modified_at,
                unix_now(),
            ],
        )
        .map_err(database_error)?;

    let chunks = chunk_markdown(&page.text, chunk_chars);
    for chunk in &chunks {
        let chunk_id = format!("{}:chunk:{}", page.id, chunk.ordinal);
        transaction
            .execute(
                "INSERT INTO chunks
                 (id, document_id, ordinal, heading_path, start_line, end_line, text, text_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    chunk_id,
                    page.id,
                    chunk.ordinal as i64,
                    chunk.heading_path,
                    chunk.start_line as i64,
                    chunk.end_line as i64,
                    chunk.text,
                    chunk.text_hash,
                ],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "INSERT INTO chunks_fts(chunk_id, text, heading_path) VALUES (?1, ?2, ?3)",
                params![chunk_id, chunk.text, chunk.heading_path],
            )
            .map_err(database_error)?;
        index_chunk_entities(transaction, &chunk_id, chunk)?;
    }

    for link in extract_wiki_links(&page.text) {
        transaction
            .execute(
                "INSERT INTO wiki_links(source_document_id, target_path, link_text, link_kind)
                 VALUES (?1, ?2, ?3, 'wiki')",
                params![page.id, link.target_path, link.link_text],
            )
            .map_err(database_error)?;
    }
    Ok(())
}

fn delete_page(transaction: &Transaction<'_>, document_id: &str) -> Result<(), IndexError> {
    let chunk_ids: Vec<String> = {
        let mut statement = transaction
            .prepare("SELECT id FROM chunks WHERE document_id = ?1")
            .map_err(database_error)?;
        let rows = statement
            .query_map(params![document_id], |row| row.get(0))
            .map_err(database_error)?;
        rows.map(|row| row.map_err(database_error))
            .collect::<Result<_, _>>()?
    };
    for chunk_id in chunk_ids {
        transaction
            .execute(
                "DELETE FROM chunks_fts WHERE chunk_id = ?1",
                params![chunk_id],
            )
            .map_err(database_error)?;
    }
    transaction
        .execute(
            "DELETE FROM wiki_links WHERE source_document_id = ?1",
            params![document_id],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM chunks WHERE document_id = ?1",
            params![document_id],
        )
        .map_err(database_error)?;
    transaction
        .execute("DELETE FROM documents WHERE id = ?1", params![document_id])
        .map_err(database_error)?;
    Ok(())
}

fn index_chunk_entities(
    transaction: &Transaction<'_>,
    chunk_id: &str,
    chunk: &Chunk,
) -> Result<(), IndexError> {
    let mut candidates = extract_entities(&chunk.text, chunk.start_line);
    candidates.truncate(MAX_ENTITIES_PER_CHUNK);
    let mut entity_ids: BTreeMap<String, f64> = BTreeMap::new();

    for candidate in &candidates {
        let entity_id = ensure_entity(transaction, candidate)?;
        let mention_id = format!(
            "mention:{}",
            hash_bytes(
                format!(
                    "{}:{}:{}:{}",
                    entity_id, chunk_id, candidate.start_pos, candidate.end_pos
                )
                .as_bytes(),
            )
        );
        transaction
            .execute(
                "INSERT OR IGNORE INTO entity_mentions
                 (id, entity_id, chunk_id, surface, start_pos, end_pos, start_line, end_line, extractor, confidence)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    mention_id,
                    entity_id,
                    chunk_id,
                    candidate.surface,
                    candidate.start_pos as i64,
                    candidate.end_pos as i64,
                    candidate.start_line as i64,
                    candidate.end_line as i64,
                    candidate.extractor,
                    candidate.confidence,
                ],
            )
            .map_err(database_error)?;
        entity_ids
            .entry(entity_id)
            .and_modify(|confidence| *confidence = confidence.max(candidate.confidence))
            .or_insert(candidate.confidence);
    }

    let unique_entities: Vec<(String, f64)> = entity_ids.into_iter().collect();
    for left in 0..unique_entities.len() {
        for right in (left + 1)..unique_entities.len() {
            let (subject, left_confidence) = &unique_entities[left];
            let (object, right_confidence) = &unique_entities[right];
            let predicate = "co_occurs_in_chunk";
            let confidence = (*left_confidence).min(*right_confidence);
            let relation_id = format!(
                "relation:{}",
                hash_bytes(format!("{subject}:{predicate}:{object}").as_bytes())
            );
            transaction
                .execute(
                    "INSERT INTO relations
                     (id, subject_entity_id, predicate, object_entity_id, confidence, extractor, status)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'rule:co-occurrence', 'candidate')
                     ON CONFLICT(subject_entity_id, predicate, object_entity_id)
                     DO UPDATE SET confidence = CASE
                         WHEN excluded.confidence > confidence THEN excluded.confidence
                         ELSE confidence
                     END",
                    params![relation_id, subject, predicate, object, confidence],
                )
                .map_err(database_error)?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO relation_evidence
                     (relation_id, chunk_id, quote, start_line, end_line)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        relation_id,
                        chunk_id,
                        truncate_text(&chunk.text, 1_000),
                        chunk.start_line as i64,
                        chunk.end_line as i64,
                    ],
                )
                .map_err(database_error)?;
        }
    }
    Ok(())
}

fn ensure_entity(
    transaction: &Transaction<'_>,
    candidate: &EntityCandidate,
) -> Result<String, IndexError> {
    let entity_id = format!(
        "entity:{}",
        hash_bytes(format!("{}:{}", candidate.entity_type, candidate.canonical_name).as_bytes())
    );
    transaction
        .execute(
            "INSERT INTO entities
             (id, canonical_name, entity_type, aliases_json, confidence, status)
             VALUES (?1, ?2, ?3, '[]', ?4, 'candidate')
             ON CONFLICT(canonical_name, entity_type)
             DO UPDATE SET confidence = CASE
                 WHEN excluded.confidence > confidence THEN excluded.confidence
                 ELSE confidence
             END",
            params![
                entity_id,
                candidate.canonical_name,
                candidate.entity_type,
                candidate.confidence,
            ],
        )
        .map_err(database_error)?;
    Ok(entity_id)
}

fn cleanup_orphaned_entities(transaction: &Transaction<'_>) -> Result<(), IndexError> {
    transaction
        .execute(
            "DELETE FROM relations
             WHERE NOT EXISTS (
                 SELECT 1 FROM relation_evidence e WHERE e.relation_id = relations.id
             )",
            [],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM entities
             WHERE NOT EXISTS (
                 SELECT 1 FROM entity_mentions m WHERE m.entity_id = entities.id
             )",
            [],
        )
        .map_err(database_error)?;
    Ok(())
}

fn count_all(transaction: &Transaction<'_>, table: &str) -> Result<usize, IndexError> {
    let sql = match table {
        "entities" | "entity_mentions" | "relations" => {
            format!("SELECT COUNT(*) FROM {table}")
        }
        _ => {
            return Err(IndexError::Database(format!(
                "unsupported count table: {table}"
            )))
        }
    };
    let count: i64 = transaction
        .query_row(&sql, [], |row| row.get(0))
        .map_err(database_error)?;
    Ok(count.max(0) as usize)
}

fn extract_entities(text: &str, base_line: usize) -> Vec<EntityCandidate> {
    let mut candidates = Vec::new();
    let patterns = entity_patterns();
    add_capture_entities(
        &mut candidates,
        text,
        &patterns.rust_function,
        1,
        "function",
        "rule:rust-function",
        0.95,
        base_line,
    );
    add_capture_entities(
        &mut candidates,
        text,
        &patterns.go_function,
        1,
        "function",
        "rule:go-function",
        0.95,
        base_line,
    );
    add_capture_entities(
        &mut candidates,
        text,
        &patterns.type_declaration,
        1,
        "type",
        "rule:type-declaration",
        0.95,
        base_line,
    );
    add_capture_entities(
        &mut candidates,
        text,
        &patterns.module_declaration,
        1,
        "module",
        "rule:module-declaration",
        0.9,
        base_line,
    );
    add_capture_entities(
        &mut candidates,
        text,
        &patterns.import,
        1,
        "module",
        "rule:import",
        0.85,
        base_line,
    );
    add_full_entities(
        &mut candidates,
        text,
        &patterns.path,
        "path",
        "rule:path",
        0.9,
        base_line,
        |surface| normalize_entity_name("path", surface),
    );

    for captures in patterns.inline.captures_iter(text) {
        let Some(inner) = captures.get(1) else {
            continue;
        };
        let token = inner.as_str().trim();
        if token.is_empty() {
            continue;
        }
        let leading = inner.as_str().len() - inner.as_str().trim_start().len();
        let start = inner.start() + leading;
        let end = start + token.len();
        if is_command_token(token) {
            let command = command_head(token);
            add_candidate(
                &mut candidates,
                &command,
                start,
                start + command.len(),
                "command",
                "rule:inline-command",
                0.9,
                text,
                base_line,
                command.clone(),
            );
        } else if looks_like_path(token) {
            add_candidate(
                &mut candidates,
                token,
                start,
                end,
                "path",
                "rule:inline-path",
                0.92,
                text,
                base_line,
                normalize_entity_name("path", token),
            );
        } else if looks_like_file_name(token) {
            add_candidate(
                &mut candidates,
                token,
                start,
                end,
                "path",
                "rule:inline-file",
                0.9,
                text,
                base_line,
                normalize_entity_name("path", token),
            );
        } else if patterns.config.is_match(token) && !token.starts_with("http") {
            add_candidate(
                &mut candidates,
                token,
                start,
                end,
                "config",
                "rule:inline-config",
                0.8,
                text,
                base_line,
                normalize_entity_name("config", token),
            );
        } else if looks_like_identifier(token) && !is_ignored_inline_identifier(token) {
            add_candidate(
                &mut candidates,
                token,
                start,
                end,
                "identifier",
                "rule:inline-identifier",
                0.72,
                text,
                base_line,
                normalize_entity_name("identifier", token),
            );
        }
    }

    let mut deduped: BTreeMap<(String, String, usize, usize), EntityCandidate> = BTreeMap::new();
    for candidate in candidates {
        let key = (
            candidate.entity_type.to_string(),
            candidate.canonical_name.clone(),
            candidate.start_pos,
            candidate.end_pos,
        );
        match deduped.get(&key) {
            Some(existing) if existing.confidence >= candidate.confidence => {}
            _ => {
                deduped.insert(key, candidate);
            }
        }
    }
    let mut result: Vec<EntityCandidate> = deduped.into_values().collect();
    result.sort_by(|left, right| {
        left.start_pos
            .cmp(&right.start_pos)
            .then_with(|| left.end_pos.cmp(&right.end_pos))
            .then_with(|| left.entity_type.cmp(right.entity_type))
    });
    result
}

fn add_capture_entities(
    candidates: &mut Vec<EntityCandidate>,
    text: &str,
    regex: &Regex,
    group: usize,
    entity_type: &'static str,
    extractor: &'static str,
    confidence: f64,
    base_line: usize,
) {
    for captures in regex.captures_iter(text) {
        let Some(found) = captures.get(group) else {
            continue;
        };
        let Some(full_match) = captures.get(0) else {
            continue;
        };
        if entity_type == "type" && is_cli_type_option_value(text, full_match.start()) {
            continue;
        }
        add_candidate(
            candidates,
            found.as_str(),
            found.start(),
            found.end(),
            entity_type,
            extractor,
            confidence,
            text,
            base_line,
            normalize_entity_name(entity_type, found.as_str()),
        );
    }
}

fn is_cli_type_option_value(text: &str, declaration_start: usize) -> bool {
    text[..declaration_start].trim_end().ends_with("--")
}

fn add_full_entities<F>(
    candidates: &mut Vec<EntityCandidate>,
    text: &str,
    regex: &Regex,
    entity_type: &'static str,
    extractor: &'static str,
    confidence: f64,
    base_line: usize,
    normalize: F,
) where
    F: Fn(&str) -> String,
{
    for found in regex.find_iter(text) {
        add_candidate(
            candidates,
            found.as_str(),
            found.start(),
            found.end(),
            entity_type,
            extractor,
            confidence,
            text,
            base_line,
            normalize(found.as_str()),
        );
    }
}

fn add_candidate(
    candidates: &mut Vec<EntityCandidate>,
    surface: &str,
    start_pos: usize,
    end_pos: usize,
    entity_type: &'static str,
    extractor: &'static str,
    confidence: f64,
    text: &str,
    base_line: usize,
    canonical_name: String,
) {
    let surface = surface.trim();
    if surface.is_empty() || canonical_name.is_empty() {
        return;
    }
    candidates.push(EntityCandidate {
        canonical_name,
        entity_type,
        surface: surface.to_string(),
        start_pos,
        end_pos,
        start_line: line_for_offset(text, start_pos, base_line),
        end_line: line_for_offset(text, end_pos, base_line),
        confidence,
        extractor,
    });
}

fn normalize_entity_name(entity_type: &str, value: &str) -> String {
    let mut value = value
        .trim()
        .trim_matches(|character: char| {
            matches!(character, '\x60' | ',' | ';' | ':' | ')' | '}' | ']')
        })
        .replace('\\', "/");
    while let Some(stripped) = value.strip_prefix("./") {
        value = stripped.to_string();
    }
    if entity_type == "command" {
        return command_head(&value);
    }
    value
}

fn command_head(value: &str) -> String {
    value
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_command_token(value: &str) -> bool {
    value
        .split_whitespace()
        .next()
        .is_some_and(|first| matches!(first, "wiki" | "cargo" | "git" | "go" | "rustc"))
}

fn looks_like_path(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let known_extension = [
        ".rs", ".go", ".toml", ".yaml", ".yml", ".json", ".md", ".csv",
    ]
    .iter()
    .any(|extension| lower.ends_with(extension));
    known_extension && (value.contains('/') || value.contains('\\'))
}

fn looks_like_file_name(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        ".rs", ".go", ".toml", ".yaml", ".yml", ".json", ".md", ".csv",
    ]
    .iter()
    .any(|extension| lower.ends_with(extension) && lower.len() > extension.len())
}

fn looks_like_identifier(value: &str) -> bool {
    if value.len() < 3
        || !value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return false;
    }
    value
        .chars()
        .any(|character| character.is_ascii_uppercase())
        || value.contains('_')
        || value.chars().any(|character| character.is_ascii_digit())
}

fn is_ignored_inline_identifier(value: &str) -> bool {
    IGNORED_INLINE_IDENTIFIERS
        .iter()
        .any(|ignored| value.eq_ignore_ascii_case(ignored))
}

fn line_for_offset(text: &str, offset: usize, base_line: usize) -> usize {
    let mut boundary = offset.min(text.len());
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    base_line
        + text[..boundary]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut result: String = text.chars().take(max_chars).collect();
    result.push_str("...");
    result
}

fn count_chunks(transaction: &Transaction<'_>, document_id: &str) -> Result<usize, IndexError> {
    let count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE document_id = ?1",
            params![document_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    Ok(count.max(0) as usize)
}

fn count_links(transaction: &Transaction<'_>, document_id: &str) -> Result<usize, IndexError> {
    let count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM wiki_links WHERE source_document_id = ?1",
            params![document_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    Ok(count.max(0) as usize)
}

fn first_heading(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let trimmed = line.trim();
        let hashes = trimmed
            .chars()
            .take_while(|character| *character == '#')
            .count();
        if hashes == 0 || hashes > 6 || trimmed.chars().nth(hashes) != Some(' ') {
            return None;
        }
        Some(
            trimmed
                .get(hashes + 1..)
                .unwrap_or_default()
                .trim()
                .trim_end_matches('#')
                .trim()
                .to_string(),
        )
    })
}

fn chunk_markdown(text: &str, max_chars: usize) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut heading_stack: Vec<(usize, String)> = Vec::new();
    let mut current_lines: Vec<(usize, String)> = Vec::new();
    let mut current_heading = String::new();
    let mut current_chars = 0usize;

    for (index, raw_line) in text.lines().enumerate() {
        let line_number = index + 1;
        if let Some((level, heading)) = parse_heading(raw_line) {
            flush_chunk(&mut chunks, &mut current_lines, &current_heading);
            current_chars = 0;
            while heading_stack
                .last()
                .is_some_and(|(existing_level, _)| *existing_level >= level)
            {
                heading_stack.pop();
            }
            heading_stack.push((level, heading));
            current_heading = heading_stack
                .iter()
                .map(|(_, value)| value.as_str())
                .collect::<Vec<_>>()
                .join(" > ");
        }

        let line = raw_line.to_string();
        let line_chars = line.chars().count() + 1;
        if current_chars > 0 && current_chars + line_chars > max_chars {
            flush_chunk(&mut chunks, &mut current_lines, &current_heading);
            current_chars = 0;
        }
        current_lines.push((line_number, line));
        current_chars += line_chars;

        if current_lines
            .last()
            .is_some_and(|(_, line)| line.trim().is_empty())
            && current_lines.len() > 1
        {
            if current_chars >= max_chars / 2 {
                flush_chunk(&mut chunks, &mut current_lines, &current_heading);
                current_chars = 0;
            }
        }
    }
    flush_chunk(&mut chunks, &mut current_lines, &current_heading);
    chunks
}

fn flush_chunk(chunks: &mut Vec<Chunk>, lines: &mut Vec<(usize, String)>, heading_path: &str) {
    if lines.is_empty() {
        return;
    }
    while lines.last().is_some_and(|(_, line)| line.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return;
    }
    let text = lines
        .iter()
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let ordinal = chunks.len();
    chunks.push(Chunk {
        ordinal,
        heading_path: heading_path.to_string(),
        start_line: lines.first().map(|(line, _)| *line).unwrap_or(0),
        end_line: lines.last().map(|(line, _)| *line).unwrap_or(0),
        text_hash: hash_bytes(text.as_bytes()),
        text,
    });
    lines.clear();
}

fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let level = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if level == 0 || level > 6 || trimmed.chars().nth(level) != Some(' ') {
        return None;
    }
    let heading = trimmed
        .get(level + 1..)
        .unwrap_or_default()
        .trim()
        .trim_end_matches('#')
        .trim()
        .to_string();
    Some((level, heading))
}

fn extract_wiki_links(text: &str) -> Vec<WikiLink> {
    let searchable = markdown_without_code(text);
    wiki_link_pattern()
        .captures_iter(&searchable)
        .filter_map(|captures| {
            let target = normalize_link_target(captures.get(1)?.as_str());
            if target.is_empty() {
                return None;
            }
            Some(WikiLink {
                target_path: target,
                link_text: captures
                    .get(2)
                    .map(|value| value.as_str().trim().to_string()),
            })
        })
        .collect()
}

fn markdown_without_code(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            output.push('\n');
            continue;
        }
        if in_fence {
            output.push('\n');
            continue;
        }
        output.push_str(&strip_inline_code(line));
        output.push('\n');
    }
    output
}

fn strip_inline_code(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut in_code = false;
    for character in line.chars() {
        if character == '\x60' {
            in_code = !in_code;
            output.push(' ');
        } else if in_code {
            output.push(' ');
        } else {
            output.push(character);
        }
    }
    output
}

fn normalize_link_target(value: &str) -> String {
    let mut target = value.trim().replace('\\', "/");
    while let Some(stripped) = target.strip_prefix("./") {
        target = stripped.to_string();
    }
    target.trim_end_matches(".md").to_string()
}

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn database_error(error: rusqlite::Error) -> IndexError {
    IndexError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(root: &Path) -> IndexOptions {
        IndexOptions {
            path: Some(root.to_string_lossy().into_owned()),
            rebuild: false,
            chunk_chars: DEFAULT_CHUNK_CHARS,
        }
    }

    #[test]
    fn chunk_markdown_tracks_heading_paths_and_lines() {
        let chunks = chunk_markdown(
            r#"# Root

intro

## Child

body
"#,
            1_200,
        );
        assert_eq!(chunks.len(), 2, "{chunks:?}");
        assert_eq!(chunks[0].heading_path, "Root");
        assert_eq!(chunks[0].start_line, 1);
        assert!(chunks[0].text.as_bytes().contains(&10));
        assert_eq!(chunks[1].heading_path, "Root > Child");
        assert_eq!(chunks[1].start_line, 5);
    }

    #[test]
    fn index_is_incremental_and_removes_deleted_pages() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::write(
            root.join("notes").join("one.md"),
            r#"# One

See [[notes/two|Two]].
"#,
        )
        .unwrap();
        fs::write(
            root.join("notes").join("two.md"),
            r#"# Two

body"#,
        )
        .unwrap();

        let first = index(&options(root)).unwrap();
        assert_eq!(first.scanned, 2);
        assert_eq!(first.added, 2);
        assert_eq!(first.unchanged, 0);
        assert_eq!(first.links, 1);
        assert!(first
            .database_path
            .ends_with(Path::new(".wiki/index.sqlite")));

        let second = index(&options(root)).unwrap();
        assert_eq!(second.added, 0);
        assert_eq!(second.updated, 0);
        assert_eq!(second.unchanged, 2);

        fs::write(
            root.join("notes").join("one.md"),
            r#"# One

changed"#,
        )
        .unwrap();
        fs::remove_file(root.join("notes").join("two.md")).unwrap();
        let third = index(&options(root)).unwrap();
        assert_eq!(third.updated, 1);
        assert_eq!(third.removed, 1);
        assert_eq!(third.scanned, 1);
    }

    #[test]
    fn index_skips_its_internal_directory() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join(INDEX_DIR)).unwrap();
        fs::write(root.join("page.md"), "page").unwrap();
        fs::write(root.join(INDEX_DIR).join("ignored.md"), "ignored").unwrap();
        let stats = index(&options(root)).unwrap();
        assert_eq!(stats.scanned, 1);
    }

    #[test]
    fn index_upgrades_older_schema() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join(INDEX_DIR)).unwrap();
        fs::write(root.join("page.md"), "# Page\n\nbody\n").unwrap();
        let database_path = root.join(INDEX_DIR).join(INDEX_DB);
        let connection = rusqlite::Connection::open(&database_path).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        drop(connection);

        index(&options(root)).unwrap();

        let connection = rusqlite::Connection::open(database_path).unwrap();
        let version: i32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table'
                   AND name IN ('entities', 'entity_mentions', 'relations', 'relation_evidence', 'chunk_embeddings')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 5);
    }

    #[test]
    fn index_rebuilds_when_indexer_version_changes() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let marker = '\x60';
        fs::write(
            root.join("page.md"),
            format!("# Page\n\n{marker}RoleService{marker}.\n"),
        )
        .unwrap();
        index(&options(root)).unwrap();

        let connection = rusqlite::Connection::open(root.join(INDEX_DIR).join(INDEX_DB)).unwrap();
        connection
            .execute(
                "UPDATE meta SET value = 'wiki-index-v2' WHERE key = 'indexer'",
                [],
            )
            .unwrap();
        drop(connection);

        let stats = index(&options(root)).unwrap();
        assert_eq!(stats.added, 1);
        assert_eq!(stats.updated, 0);
        assert_eq!(stats.unchanged, 0);
        assert_eq!(stats.removed, 1);
    }

    #[test]
    fn normalize_link_target_removes_relative_prefix_and_extension() {
        assert_eq!(normalize_link_target("./notes/foo.md"), "notes/foo");
    }

    #[test]
    fn extract_wiki_links_ignores_fenced_and_inline_code() {
        let marker = '\x60';
        let fence = marker.to_string().repeat(3);
        let text = format!(
            "Real [[notes/real]].\n\n{fence}text\nExample [[notes/fenced]]\n{fence}\n\nUse {marker}[[notes/inline]]{marker} as syntax.\n"
        );
        let links = extract_wiki_links(&text);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_path, "notes/real");
    }

    #[test]
    fn rule_entity_extractor_finds_technical_entities_without_prose_commands() {
        let marker = '\x60';
        let text = format!(
            "# Role\n\nfn HandleRequest() {{}}\ntype RoleData struct {{}}\nUse {marker}RoleService{marker}, {marker}server.port{marker}, {marker}src/role.go{marker}, {marker}co_occurs_in_chunk{marker}, and {marker}wiki index{marker}.\nThe wiki is useful. Use --type identifier when filtering.\n"
        );
        let entities = extract_entities(&text, 1);
        let names: BTreeSet<(&str, &str)> = entities
            .iter()
            .map(|entity| (entity.entity_type, entity.canonical_name.as_str()))
            .collect();
        assert!(names.contains(&("function", "HandleRequest")));
        assert!(names.contains(&("type", "RoleData")));
        assert!(names.contains(&("identifier", "RoleService")));
        assert!(names.contains(&("config", "server.port")));
        assert!(names.contains(&("path", "src/role.go")));
        assert!(names.contains(&("command", "wiki index")));
        assert!(!names.contains(&("identifier", "co_occurs_in_chunk")));
        assert!(!names.contains(&("type", "identifier")));
        assert!(!names.contains(&("command", "wiki is")));
        assert!(entities.iter().all(|entity| entity.start_line >= 1));
    }

    #[test]
    fn entity_extractor_handles_non_ascii_inline_command_text() {
        let marker = '\x60';
        let text = format!("Use {marker}wiki 返回{marker}.\n");

        let entities = extract_entities(&text, 1);
        let command = entities
            .iter()
            .find(|entity| entity.entity_type == "command")
            .expect("expected an inline command entity");

        assert_eq!(command.canonical_name, "wiki 返回");
        assert_eq!(command.start_line, 1);
        assert_eq!(command.end_line, 1);
    }

    #[test]
    fn index_writes_entity_mentions_and_cooccurrence_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let marker = '\x60';
        let text = format!(
            "# Page\n\n{marker}RoleService{marker} uses {marker}EquipBagSize{marker} and {marker}server.port{marker}.\n"
        );
        fs::write(root.join("page.md"), text).unwrap();
        let stats = index(&options(root)).unwrap();
        assert!(stats.entities >= 3, "{stats:?}");
        assert!(stats.mentions >= 3, "{stats:?}");
        assert!(stats.relations >= 1, "{stats:?}");

        let connection = rusqlite::Connection::open(root.join(".wiki/index.sqlite")).unwrap();
        let evidence: i64 = connection
            .query_row("SELECT COUNT(*) FROM relation_evidence", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(evidence >= 1);
        let role_mentions: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM entity_mentions m
                 JOIN entities e ON e.id = m.entity_id
                 WHERE e.canonical_name = 'RoleService'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(role_mentions, 1);
    }
}
