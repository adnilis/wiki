//! `wiki init` — seed a knowledge-base wiki at a target directory.
//!
//! This module owns the canonical seed-file write path. It produces a
//! byte-stable seed for a fresh directory and a `Keep`/`Create`/`Overwrite` plan that
//! can be inspected before commit (or shown in `--dry-run` mode).
//!
//! Layout committed by `commit`:
//!
//! - `<dir>/AGENTS.md` — global context: owner goals and non-negotiable rules.
//! - `<dir>/index.md`  — catalog and navigation aid.
//! - `<dir>/SDD.md`    — SDD workflow, artifact contract, and agent checklist.
//! - `<dir>/log.md`    — append-only chronology; entries begin with
//!   `## [YYYY-MM-DD]`, the same prefix `read` and `search` parse.
//! - `<dir>/sdd/`      — spec-driven changes and archives.
//! - `<dir>/notes/`    — verified, reusable source-of-truth knowledge.
//! - `<dir>/ideas/`    — original reasoning, preferences, decision rationale.
//! - `<dir>/projects/` — active work, plans, implementation, verification.

use crate::shared::{
    display_path, knowledge_area_names, AGENTS_FILENAME, INDEX_FILENAME, LOG_FILENAME, SDD_FILENAME,
};
use std::fs;
use std::path::{Path, PathBuf};
use time::macros::format_description;
use time::OffsetDateTime;

/// User choices for the knowledge-base seed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnowledgeOptions {
    /// Target directory. Created if absent. Relative paths resolve against
    /// the session working directory.
    pub dir: PathBuf,
    /// Optional source URL recorded in the seed. Stored verbatim; never
    /// fetched. When `None`, the seed documents omit the `Source:` line.
    pub url: Option<String>,
    /// Overwrite existing seed files instead of leaving them untouched.
    pub force: bool,
}

/// One file or directory the command will touch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnowledgeItem {
    /// Absolute target path.
    pub path: PathBuf,
    /// Stable English verb shown in the preview/receipt.
    pub action: KnowledgeAction,
}

/// Stable action semantics for the preview and the receipt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KnowledgeAction {
    /// The target did not exist before; the command writes it now.
    Create,
    /// The target already exists; the command leaves it untouched.
    Keep,
    /// The target already exists and the user passed `--force`; the
    /// command replaces its contents.
    Overwrite,
}

impl KnowledgeAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "Create",
            Self::Keep => "Keep",
            Self::Overwrite => "Overwrite",
        }
    }

    #[allow(dead_code)]
    fn is_written(self) -> bool {
        matches!(self, Self::Create | Self::Overwrite)
    }
}

/// Immutable plan shared by the preview and the commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KnowledgePlan {
    items: Vec<KnowledgeItem>,
    options: KnowledgeOptions,
    resolved_dir: PathBuf,
}

#[allow(dead_code)]
impl KnowledgePlan {
    /// Every effective item in the order they will be processed.
    pub fn preview(&self) -> &[KnowledgeItem] {
        &self.items
    }

    /// Resolved target directory.
    pub fn resolved_dir(&self) -> &Path {
        &self.resolved_dir
    }

    /// Source URL recorded in the seed, if any.
    pub fn url(&self) -> Option<&str> {
        self.options.url.as_deref()
    }

    /// True when no file would be written (every item is `Keep`).
    pub fn is_empty(&self) -> bool {
        self.items.iter().all(|item| !item.action.is_written())
    }
}

/// Outcome of a successful commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationReceipt {
    /// Number of files actually written (Create or Overwrite).
    pub changed_targets: usize,
    /// User-facing follow-up notes.
    pub notes: Vec<String>,
}

/// Computes the immutable plan without writing anything to disk.
pub fn plan(options: KnowledgeOptions) -> Result<KnowledgePlan, String> {
    if let Some(url) = options.url.as_deref() {
        if url.trim().is_empty() {
            return Err("--url must be a non-empty source URL.".to_string());
        }
    }
    let resolved_dir = resolve(&options.dir)?;
    if resolved_dir.exists() {
        let metadata = fs::metadata(&resolved_dir).map_err(|error| {
            format!(
                "Cannot inspect the knowledge-base directory {}: {error}",
                display_path(&resolved_dir)
            )
        })?;
        if !metadata.is_dir() {
            return Err(format!(
                "The knowledge-base path {} exists but is not a directory.",
                display_path(&resolved_dir)
            ));
        }
    }

    let agents_path = resolved_dir.join(AGENTS_FILENAME);
    let index_path = resolved_dir.join(INDEX_FILENAME);
    let log_path = resolved_dir.join(LOG_FILENAME);
    let sdd_path = resolved_dir.join(SDD_FILENAME);
    let mut items = vec![
        classify_file(&agents_path, options.force),
        classify_file(&index_path, options.force),
        classify_file(&log_path, options.force),
        classify_file(&sdd_path, options.force),
    ];
    items.extend(
        knowledge_area_names()
            .iter()
            .map(|name| classify_directory(&resolved_dir.join(name))),
    );

    Ok(KnowledgePlan {
        items,
        options,
        resolved_dir,
    })
}

fn resolve(dir: &Path) -> Result<PathBuf, String> {
    if dir.is_absolute() {
        return Ok(dir.to_path_buf());
    }
    let cwd = std::env::current_dir()
        .map_err(|error| format!("Cannot determine the session working directory: {error}"))?;
    Ok(cwd.join(dir))
}

fn classify_file(path: &Path, force: bool) -> KnowledgeItem {
    if path.exists() {
        if force {
            KnowledgeItem {
                path: path.to_path_buf(),
                action: KnowledgeAction::Overwrite,
            }
        } else {
            KnowledgeItem {
                path: path.to_path_buf(),
                action: KnowledgeAction::Keep,
            }
        }
    } else {
        KnowledgeItem {
            path: path.to_path_buf(),
            action: KnowledgeAction::Create,
        }
    }
}

fn classify_directory(path: &Path) -> KnowledgeItem {
    if path.exists() {
        // Knowledge-area directories are always user-owned once present.
        KnowledgeItem {
            path: path.to_path_buf(),
            action: KnowledgeAction::Keep,
        }
    } else {
        KnowledgeItem {
            path: path.to_path_buf(),
            action: KnowledgeAction::Create,
        }
    }
}

/// Writes the seed for a plan. Every action is committed in order; the
/// operation is per-file best-effort, but aborts the whole batch on the
/// first error.
pub fn commit(plan: KnowledgePlan) -> Result<OperationReceipt, String> {
    fs::create_dir_all(&plan.resolved_dir).map_err(|error| {
        format!(
            "Cannot create the knowledge-base directory {}: {error}",
            display_path(&plan.resolved_dir)
        )
    })?;

    for name in knowledge_area_names() {
        let area_path = plan.resolved_dir.join(name);
        if !area_path.exists() {
            fs::create_dir_all(&area_path).map_err(|error| {
                format!(
                    "Cannot create the knowledge-base area directory {}: {error}",
                    display_path(&area_path)
                )
            })?;
        }
    }
    let sdd_path = plan.resolved_dir.join("sdd");
    for child in ["changes", "archives"] {
        fs::create_dir_all(sdd_path.join(child)).map_err(|error| {
            format!(
                "Cannot create the SDD directory {}: {error}",
                display_path(&sdd_path.join(child))
            )
        })?;
    }

    let mut written = 0usize;
    let mut notes = Vec::new();
    for item in &plan.items {
        match item.action {
            KnowledgeAction::Create | KnowledgeAction::Overwrite => {
                let name = match item.path.file_name().and_then(|n| n.to_str()) {
                    Some(name) => name,
                    None => {
                        return Err(format!(
                            "Cannot render the knowledge-base seed file {}: unknown target.",
                            display_path(&item.path)
                        ));
                    }
                };
                let bytes = if name == AGENTS_FILENAME {
                    render_agents(plan.url(), &plan.resolved_dir)
                } else if name == INDEX_FILENAME {
                    render_index(plan.url(), &plan.resolved_dir)
                } else if name == LOG_FILENAME {
                    render_log(plan.url())
                } else if name == SDD_FILENAME {
                    render_sdd()
                } else if knowledge_area_names().contains(&name) {
                    continue;
                } else {
                    return Err(format!(
                        "Cannot render the knowledge-base seed file {}: unknown target.",
                        display_path(&item.path)
                    ));
                };
                fs::write(&item.path, bytes).map_err(|error| {
                    format!(
                        "Cannot write the knowledge-base file {}: {error}",
                        display_path(&item.path)
                    )
                })?;
                written += 1;
            }
            KnowledgeAction::Keep => {
                if let Some(name) = item.path.file_name().and_then(|n| n.to_str()) {
                    if [AGENTS_FILENAME, INDEX_FILENAME, LOG_FILENAME, SDD_FILENAME].contains(&name)
                    {
                        notes.push(format!(
                            "Kept existing {}; pass --force to overwrite.",
                            display_path(&item.path)
                        ));
                    }
                }
            }
        }
    }

    if written == 0 {
        notes.push("No changes were needed.".to_string());
    } else {
        notes.push(format!(
            "Knowledge base ready at {}.",
            display_path(&plan.resolved_dir)
        ));
    }

    Ok(OperationReceipt {
        changed_targets: written,
        notes,
    })
}

const GRAPH_RAG_GUIDANCE: &str = r#"
## Knowledge graph and Graph-RAG

- Run wiki index --path <PATH> after adding or changing Markdown; it builds the rebuildable local graph and FTS index under .wiki/.
- Use wiki harness --path <PATH> --query "..." --format json as the integration entry point; it refreshes the index and combines orientation with task evidence.
- Use wiki harness --path <PATH> --query "..." --format prompt when passing source-aware context to a downstream answerer.
- Inspect graph structure with wiki graph stats, wiki graph entities, wiki graph neighbors, wiki graph path, and wiki graph export.
- Search graph-aware evidence with wiki rag search --path <PATH> --query "..."; use wiki rag context to produce a bounded evidence package.
- Ask a source-aware question with wiki rag ask --path <PATH> --query "..."; use --format json for automation or --format prompt for a downstream answerer.
- Treat Markdown as authoritative. Extracted entities and co-occurrence relations are retrieval hints, not automatically verified semantic facts.

"#;

fn render_agents(url: Option<&str>, root: &Path) -> Vec<u8> {
    let root_display = display_path(root);
    let (header, gap) = match url {
        Some(value) => (
            format!("Knowledge base: `{root_display}`\nSource: <{value}>"),
            "\n\n",
        ),
        None => (format!("Knowledge base: `{root_display}`"), "\n\n"),
    };
    let body = format!(
        "# AGENTS.md — Global context\n\n\
         > Read this file before answering, advising, planning, or changing work in this knowledge base.\n\n\
         {header}{gap}\
         ## Who this is for\n\n\
         - Owner / team: _Add the people, domain, and working preferences that should shape every response._\n\
         - 2026 outcomes: _Add the concrete goals that matter this year._\n\n\
         ## Non-negotiable rules\n\n\
         - Before giving advice or starting implementation, search and read the relevant material in `notes/`.\n\
         - Do not substitute generic boilerplate for this knowledge base's recorded facts, preferences, or decisions.\n\
         - Treat verified material in `notes/` as the source of truth; clearly label uncertainty and hypotheses.\n\
         - Preserve the user's original reasoning in `ideas/`; do not flatten it into generic recommendations.\n\
         - After meaningful work, update the relevant project record and append a concise verified entry to `log.md`.\n\n\
         ## Knowledge areas\n\n\
         - `sdd/` — spec-driven changes under `sdd/changes/` and completed changes under `sdd/archives/`.\n\
         - `notes/` — processed, linked facts, technical references, and research; the reusable source of truth.\n\
         - `ideas/` — original thinking, taste, decision rationale, and working heuristics.\n\
         - `projects/` — active work: briefs, plans, implementation notes, decisions, and verification records.\n\n\
         ## Working method\n\n\
         1. Read this file, then use `wiki read` to orient yourself.\n\
         2. Search `notes/` before making recommendations or implementation choices.\n\
         3. Use `ideas/` to preserve the owner's reasoning and `projects/` to continue active work with context.\n\
         4. Keep SDD change specifications under `sdd/changes/`; archive verified changes under `sdd/archives/`.\n"
    );
    let body = body.replace(
        "## Knowledge areas\n\n",
        "## Knowledge areas\n\n- Read [[SDD]] before creating or advancing a spec-driven change; it defines the required artifacts, state transitions, and host handoff protocol.\n\n",
    );
    format!("{body}{GRAPH_RAG_GUIDANCE}").into_bytes()
}

fn render_index(url: Option<&str>, root: &Path) -> Vec<u8> {
    let root_display = display_path(root);
    let (header, gap) = match url {
        Some(value) => (format!("Root: `{root_display}`\nSource: <{value}>"), "\n\n"),
        None => (format!("Root: `{root_display}`"), "\n\n"),
    };
    let body = format!(
        "# Knowledge base\n\n\
         {header}{gap}\
         ## Start here\n\n\
         - Read [[AGENTS]] first. It defines the owner context, goals, and non-negotiable working rules.\n\
         - Use `wiki read` for a concise overview and `wiki search` to find connected material.\n\n\
         ## Knowledge areas\n\n\
         - `sdd/` — active changes in `sdd/changes/` and archived changes in `sdd/archives/`.\n\
         - `notes/` — verified, reusable source-of-truth knowledge.\n\
         - `ideas/` — original thinking and decision rationale.\n\
         - `projects/` — active work and its plans, decisions, and verification.\n\n\
         ## Catalog\n\n\
         - _No pages yet. Add markdown files to the appropriate area and list durable entry points here._\n\n\
         ## Conventions\n\n\
         - Search the relevant material in `notes/` before advice, design, or implementation.\n\
         - Keep original thinking in `ideas/`, active work records in `projects/`, and SDD changes in `sdd/`.\n\
         - Record meaningful work as a `## [YYYY-MM-DD] action | summary | [[link]]` entry in `log.md`.\n\
         - Keep `index.md` in sync with durable entry points.\n"
    );
    body.replace(
        "- Read [[AGENTS]] first. It defines the owner context, goals, and non-negotiable working rules.\n",
        "- Read [[AGENTS]] first. It defines the owner context, goals, and non-negotiable working rules.\n- Read [[SDD]] for the spec-driven development workflow and artifact contract.\n",
    )
    .replace(
        "## Knowledge areas",
        r#"- Use wiki harness --path docs --query "..." --format json as the single context entry point for agents and automation.
## Knowledge areas"#,
    )
    .into_bytes()
}

fn render_sdd() -> Vec<u8> {
    include_bytes!("../SDD.md").to_vec()
}

fn render_log(url: Option<&str>) -> Vec<u8> {
    let format = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
    let stamp = OffsetDateTime::now_utc()
        .format(&format)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string());
    let (header, gap) = match url {
        Some(value) => (format!("Source: <{value}>"), "\n\n"),
        None => (String::new(), "\n"),
    };
    let body = format!(
        "# Knowledge-base log\n\n\
         {header}{gap}\
         ## [{stamp}] init | seed knowledge base | [[AGENTS]]\n\
         - Created AGENTS.md, SDD.md, notes/, ideas/, projects/, and sdd/{{changes,archives}}/; update the relevant project record after meaningful work.\n"
    );
    body.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn options(dir: &Path, url: Option<&str>, force: bool) -> KnowledgeOptions {
        KnowledgeOptions {
            dir: dir.to_path_buf(),
            url: url.map(str::to_string),
            force,
        }
    }

    #[test]
    fn plan_marks_every_seed_target_as_create_on_an_empty_directory() {
        let dir = temp_dir();
        let plan = plan(options(
            dir.path(),
            Some("https://example.com/vault"),
            false,
        ))
        .unwrap();
        assert!(!plan.is_empty(), "an empty dir should plan writes");
        let actions: Vec<&'static str> = plan
            .preview()
            .iter()
            .map(|item| item.action.as_str())
            .collect();
        assert_eq!(actions, vec!["Create"; 8]);
        assert_eq!(plan.url(), Some("https://example.com/vault"));
    }

    #[test]
    fn plan_keeps_existing_files_without_force() {
        let dir = temp_dir();
        let root = dir.path();
        fs::write(root.join(AGENTS_FILENAME), "user agents\n").unwrap();
        fs::write(root.join(INDEX_FILENAME), "user index\n").unwrap();
        fs::write(root.join(LOG_FILENAME), "user log\n").unwrap();
        fs::write(root.join(SDD_FILENAME), "user sdd\n").unwrap();
        for name in knowledge_area_names() {
            fs::create_dir(root.join(name)).unwrap();
        }

        let plan = plan(options(root, Some("https://example.com/vault"), false)).unwrap();
        let actions: Vec<&'static str> = plan
            .preview()
            .iter()
            .map(|item| item.action.as_str())
            .collect();
        assert_eq!(actions, vec!["Keep"; 8]);
        assert!(plan.is_empty());
    }

    #[test]
    fn plan_overwrites_existing_files_with_force() {
        let dir = temp_dir();
        let root = dir.path();
        fs::write(root.join(INDEX_FILENAME), "user index\n").unwrap();
        fs::write(root.join(LOG_FILENAME), "user log\n").unwrap();

        let plan = plan(options(root, Some("https://example.com/vault"), true)).unwrap();
        let actions: Vec<&'static str> = plan
            .preview()
            .iter()
            .map(|item| item.action.as_str())
            .collect();
        // Existing seed files are forced; missing knowledge areas are created.
        assert_eq!(
            actions,
            vec![
                "Create",
                "Overwrite",
                "Overwrite",
                "Create",
                "Create",
                "Create",
                "Create",
                "Create"
            ]
        );
        assert!(!plan.is_empty());
    }

    #[test]
    fn commit_writes_the_seed_and_creates_missing_parent_directories() {
        let dir = temp_dir();
        let nested = dir.path().join("nested").join("inner");
        let plan = plan(options(&nested, Some("https://example.com/vault"), false)).unwrap();
        let receipt = commit(plan).unwrap();
        // Four seed documents are written; empty area directories do not
        // count as changed file targets.
        assert_eq!(receipt.changed_targets, 4);
        assert!(nested.is_dir());
        assert!(nested.join(AGENTS_FILENAME).is_file());
        assert!(nested.join(INDEX_FILENAME).is_file());
        assert!(nested.join(LOG_FILENAME).is_file());
        assert!(nested.join(SDD_FILENAME).is_file());
        for name in knowledge_area_names() {
            assert!(nested.join(name).is_dir(), "missing {name}/");
        }
        assert!(nested.join("sdd").join("changes").is_dir());
        assert!(nested.join("sdd").join("archives").is_dir());

        let agents = fs::read_to_string(nested.join(AGENTS_FILENAME)).unwrap();
        assert!(agents.contains("Read this file before"), "{agents}");
        assert!(
            agents.contains("## Knowledge graph and Graph-RAG"),
            "{agents}"
        );
        assert!(agents.contains("wiki index --path <PATH>"), "{agents}");
        assert!(agents.contains("wiki harness --path <PATH>"), "{agents}");
        assert!(agents.contains("wiki graph entities"), "{agents}");
        assert!(agents.contains("wiki rag ask --path <PATH>"), "{agents}");
        assert!(!agents.contains("wiki.yaml"), "{agents}");
        assert!(agents.contains("`notes/`"), "{agents}");
        assert!(agents.contains("[[SDD]]"), "{agents}");
        let sdd = fs::read_to_string(nested.join(SDD_FILENAME)).unwrap();
        assert!(sdd.contains("# SDD"), "{sdd}");
        assert!(sdd.contains("OpenSpec"), "{sdd}");
        assert!(sdd.contains("wiki sdd new"), "{sdd}");
        assert_eq!(sdd.as_bytes(), include_bytes!("../SDD.md"));
        let index = fs::read_to_string(nested.join(INDEX_FILENAME)).unwrap();
        assert!(
            index.contains("Source: <https://example.com/vault>"),
            "{index}"
        );
        assert!(index.contains("## Knowledge areas"), "{index}");

        let log = fs::read_to_string(nested.join(LOG_FILENAME)).unwrap();
        assert!(log.contains("## ["), "{log}");
        assert!(log.contains("init | seed knowledge base"), "{log}");

        assert!(
            receipt
                .notes
                .iter()
                .any(|note| note.contains("Knowledge base ready")),
            "{:?}",
            receipt.notes
        );
    }

    #[test]
    fn commit_is_idempotent_without_force_and_preserves_user_content() {
        let dir = temp_dir();
        let root = dir.path();
        let original_agents = "# My agent rules\n\nkeep me\n";
        let original_index = "# My custom index\n\ndo not touch\n";
        let original_log = "## [2026-01-01] custom | keep me | [[x]]\n";
        let original_sdd = "# My custom SDD\n";
        fs::write(root.join(AGENTS_FILENAME), original_agents).unwrap();
        fs::write(root.join(INDEX_FILENAME), original_index).unwrap();
        fs::write(root.join(LOG_FILENAME), original_log).unwrap();
        fs::write(root.join(SDD_FILENAME), original_sdd).unwrap();
        for name in knowledge_area_names() {
            fs::create_dir(root.join(name)).unwrap();
        }

        let plan = plan(options(root, Some("https://example.com/vault"), false)).unwrap();
        let receipt = commit(plan).unwrap();
        assert_eq!(receipt.changed_targets, 0);
        assert_eq!(
            fs::read_to_string(root.join(AGENTS_FILENAME)).unwrap(),
            original_agents
        );
        assert_eq!(
            fs::read_to_string(root.join(INDEX_FILENAME)).unwrap(),
            original_index
        );
        assert_eq!(
            fs::read_to_string(root.join(LOG_FILENAME)).unwrap(),
            original_log
        );
        assert_eq!(
            fs::read_to_string(root.join(SDD_FILENAME)).unwrap(),
            original_sdd
        );
    }

    #[test]
    fn commit_with_force_replaces_existing_seed_files() {
        let dir = temp_dir();
        let root = dir.path();
        fs::write(root.join(AGENTS_FILENAME), "user agents\n").unwrap();
        fs::write(root.join(INDEX_FILENAME), "user index\n").unwrap();
        fs::write(root.join(LOG_FILENAME), "user log\n").unwrap();
        fs::write(root.join(SDD_FILENAME), "user sdd\n").unwrap();

        let plan = plan(options(root, Some("https://example.com/vault"), true)).unwrap();
        let receipt = commit(plan).unwrap();
        assert_eq!(receipt.changed_targets, 4);
        let agents = fs::read_to_string(root.join(AGENTS_FILENAME)).unwrap();
        assert!(
            agents.contains("Source: <https://example.com/vault>"),
            "{agents}"
        );
        let index = fs::read_to_string(root.join(INDEX_FILENAME)).unwrap();
        assert!(
            index.contains("Source: <https://example.com/vault>"),
            "{index}"
        );
        let sdd = fs::read_to_string(root.join(SDD_FILENAME)).unwrap();
        assert!(sdd.contains("# SDD"), "{sdd}");
    }

    #[test]
    fn commit_without_a_url_omits_the_source_line() {
        let dir = temp_dir();
        let root = dir.path();
        let plan = plan(options(root, None, false)).unwrap();
        let receipt = commit(plan).unwrap();
        assert_eq!(receipt.changed_targets, 4);

        let agents = fs::read_to_string(root.join(AGENTS_FILENAME)).unwrap();
        assert!(!agents.contains("Source: <"), "{agents}");
        let index = fs::read_to_string(root.join(INDEX_FILENAME)).unwrap();
        assert!(!index.contains("Source: <"), "{index}");
        assert!(index.contains("Root:"), "{index}");
        assert!(index.contains("## Knowledge areas"), "{index}");
        assert!(root.join(SDD_FILENAME).is_file());

        let log = fs::read_to_string(root.join(LOG_FILENAME)).unwrap();
        assert!(!log.contains("Source: <"), "{log}");
        assert!(log.contains("## ["), "{log}");
        assert!(log.contains("init | seed knowledge base"), "{log}");
    }

    #[test]
    fn plan_rejects_an_explicitly_empty_source_url() {
        let dir = temp_dir();
        let error = plan(options(dir.path(), Some("  "), false)).unwrap_err();
        assert!(error.contains("--url"), "{error}");
    }

    #[test]
    fn plan_rejects_a_path_that_points_to_a_file() {
        let dir = temp_dir();
        let file = dir.path().join("not-a-dir");
        fs::write(&file, b"x").unwrap();
        let error = plan(options(&file, Some("https://example.com/vault"), false)).unwrap_err();
        assert!(error.contains("not a directory"), "{error}");
    }

    #[test]
    fn log_entry_matches_the_wiki_parser_prefix() {
        let bytes = render_log(Some("https://example.com/vault"));
        let text = std::str::from_utf8(&bytes).unwrap();
        let first_entry = text
            .lines()
            .find(|line| line.starts_with("## ["))
            .expect("a log entry");
        // The CLI's `read --log-last` and `search` parse this exact prefix.
        assert!(first_entry.starts_with("## [2"), "{first_entry}");
        assert!(first_entry.contains("init"), "{first_entry}");
        assert!(first_entry.contains("[[AGENTS]]"), "{first_entry}");
    }

    #[test]
    fn index_lists_the_supplied_url_and_the_knowledge_areas() {
        let bytes = render_index(Some("https://example.com/vault"), &PathBuf::from("docs"));
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(
            text.contains("Source: <https://example.com/vault>"),
            "{text}"
        );
        assert!(text.contains("[[AGENTS]]"), "{text}");
        assert!(text.contains("[[SDD]]"), "{text}");
        for name in knowledge_area_names() {
            assert!(text.contains(&format!("`{name}/`")), "{text}");
        }
        assert!(text.contains("## Catalog"), "{text}");
    }

    #[test]
    fn index_omits_the_source_line_when_no_url_is_provided() {
        let bytes = render_index(None, &PathBuf::from("docs"));
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(!text.contains("Source:"), "{text}");
        assert!(text.contains("Root:"), "{text}");
        assert!(text.contains("`notes/`"), "{text}");
    }

    #[test]
    fn commit_preserves_a_legacy_pages_directory() {
        let dir = temp_dir();
        let root = dir.path();
        let legacy = root.join("pages");
        fs::create_dir(&legacy).unwrap();
        fs::write(legacy.join("existing.md"), "# Existing page\n").unwrap();

        let receipt = commit(plan(options(root, None, false)).unwrap()).unwrap();

        assert_eq!(receipt.changed_targets, 4);
        assert_eq!(
            fs::read_to_string(legacy.join("existing.md")).unwrap(),
            "# Existing page\n"
        );
        for name in knowledge_area_names() {
            assert!(root.join(name).is_dir(), "missing {name}/");
        }
    }
}
