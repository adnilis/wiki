//! Standalone wiki CLI.
//!
//! Subcommands mirror the wiki tools re-implemented from `fastctx`:
//! wiki init (seed a knowledge-base directory), wiki read (overview
//! of a markdown tree), wiki search (literal/regex search with three
//! output modes), and wiki harness (agent-ready project context).

mod ask;
mod config;
mod embedding;
mod graph;
mod harness;
mod index;
mod init;
mod rag;
mod read;
mod sdd;
mod sdd_render;
mod sdd_types;
mod search;
mod shared;

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;

/// Standalone wiki CLI — seed, read, and search a markdown knowledge base.
#[derive(Debug, Parser)]
#[command(
    name = "wiki",
    version,
    about = "Standalone wiki CLI — seed, read, and search a markdown knowledge base.",
    long_about = "A small, dependency-light CLI for working with markdown knowledge bases. init seeds a canonical layout; read and search inspect any markdown tree; harness builds one bounded, source-aware context package for agents and automation."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Seed a knowledge base at a target directory (AGENTS.md, index.md,
    /// log.md, and notes/ideas/projects/sdd/).
    Init {
        /// Target directory; created if absent. Defaults to `docs/`.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        dir: PathBuf,

        /// Optional source URL recorded in the seed (no network I/O is
        /// performed). Omit to seed an unlabeled knowledge base.
        #[arg(long, value_name = "URL")]
        url: Option<String>,

        /// Overwrite existing AGENTS.md, index.md, and log.md instead of
        /// leaving them. Existing knowledge-area directories and legacy
        /// content are always kept.
        #[arg(long)]
        force: bool,

        /// Plan and report what would change without writing anything.
        #[arg(long)]
        dry_run: bool,
    },

    /// Build or update the local SQLite/FTS index for Markdown pages.
    Index {
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,

        /// Clear indexed rows before scanning; Markdown files are untouched.
        #[arg(long)]
        rebuild: bool,

        /// Approximate maximum characters per indexed chunk.
        #[arg(
            long,
            value_name = "CHARS",
            default_value_t = index::DEFAULT_CHUNK_CHARS
        )]
        chunk_chars: usize,
    },

    /// Query or export the explicit wiki-link knowledge graph.
    Graph {
        #[command(subcommand)]
        command: GraphCommand,
    },

    /// Build one bounded, source-aware project context package for agents.
    Harness {
        /// Task or question used for evidence retrieval.
        #[arg(short = 'q', long = "query", value_name = "QUERY")]
        query: String,
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,
        /// Treat the query as a case-insensitive Rust regex.
        #[arg(long)]
        regex: bool,
        /// Maximum result chunks to retrieve.
        #[arg(long, value_name = "N", default_value_t = 8)]
        limit: usize,
        /// Page-link expansion depth. Values above 8 are capped.
        #[arg(long, value_name = "N", default_value_t = 1)]
        depth: usize,
        /// Embedding provider. hash is deterministic and fully offline.
        #[arg(long, value_enum, default_value_t = EmbeddingModeArg::None)]
        embedding: EmbeddingModeArg,
        /// Weight applied to lexical scores.
        #[arg(long, value_name = "WEIGHT", default_value_t = 1.0)]
        lexical_weight: f64,
        /// Weight applied to page/entity graph scores.
        #[arg(long, value_name = "WEIGHT", default_value_t = 1.0)]
        graph_weight: f64,
        /// Weight applied to embedding similarity scores.
        #[arg(long, value_name = "WEIGHT", default_value_t = 1.0)]
        vector_weight: f64,
        /// Maximum characters for the combined orientation and evidence.
        #[arg(long, value_name = "N", default_value_t = harness::DEFAULT_MAX_CHARS)]
        max_chars: usize,
        /// Output format for humans, automation, or a downstream answerer.
        #[arg(long, value_enum, default_value_t = HarnessOutputFormatArg::Text)]
        format: HarnessOutputFormatArg,
    },

    /// Search the indexed wiki with FTS5 plus page/entity graph expansion.
    Rag {
        #[command(subcommand)]
        command: RagCommand,
    },

    /// Run the YAML-driven Spec-Driven Development workflow.
    Sdd {
        #[command(subcommand)]
        command: SddCommand,
    },

    /// Read the structure of a wiki: page counts, knowledge areas,
    /// AGENTS.md, index.md, and recent log entries.
    Read {
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,

        /// Omit the AGENTS.md body.
        #[arg(long)]
        no_agents: bool,

        /// Omit the index.md body.
        #[arg(long)]
        no_index: bool,

        /// Number of most-recent log.md entries to include. 0 = omit the
        /// log section entirely.
        #[arg(long, value_name = "N", default_value_t = 0)]
        log_last: usize,

        /// Hard cap on the lines of index.md returned.
        #[arg(long, value_name = "LINES", default_value_t = 200)]
        index_head_limit: usize,

        /// Hard cap on lines of root AGENTS.md returned.
        #[arg(long, value_name = "LINES", default_value_t = 200)]
        agents_head_limit: usize,

        /// Skip the "plain markdown tree" hint when the directory has
        /// neither AGENTS.md nor index.md.
        #[arg(long)]
        no_strict: bool,
    },

    /// Search a wiki directory for a term or regex.
    Search {
        /// Search term (literal substring by default; Rust regex when
        /// `--regex` is passed).
        #[arg(short = 'q', long = "query", value_name = "QUERY")]
        query: String,

        /// Wiki root. Defaults to `docs/`.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,

        /// Output mode.
        #[arg(long, value_enum, default_value_t = SearchModeArg::Summary)]
        mode: SearchModeArg,

        /// Treat the query as a Rust regex instead of a literal substring.
        #[arg(long)]
        regex: bool,
        /// Case-sensitive matching. The default is case-insensitive.
        #[arg(long)]
        case_sensitive: bool,

        /// Restrict to a standard knowledge area (`notes`, `ideas`,
        /// `projects`, `sdd`) or any nested relative directory.
        #[arg(long, value_name = "AREA")]
        category: Option<String>,

        /// Limit matches per file in summary mode.
        #[arg(long, value_name = "N", default_value_t = 5)]
        per_file_limit: usize,

        /// Hard ceiling on the number of files listed.
        #[arg(long, value_name = "N", default_value_t = 50)]
        head_limit: usize,

        /// Lines of context before/after each match in content mode.
        #[arg(long, value_name = "N", default_value_t = 2)]
        context: usize,
    },
}

#[derive(Debug, Subcommand)]
enum SddCommand {
    /// List active SDD changes; use --all to include archived changes.
    List {
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH")]
        path: Option<String>,
        /// Include archived changes in addition to active changes.
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        format: SddFormatArgs,
    },
    /// Create a new SDD change and tell the Agent where to write its Markdown specification.
    New {
        /// Human-readable change title.
        title: String,
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH")]
        path: Option<String>,
        #[command(flatten)]
        format: SddFormatArgs,
    },
    /// Verify a change structurally or with host-provided YAML evidence.
    Verify {
        #[arg(long, value_name = "CHANGE-ID")]
        change: String,
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH")]
        path: Option<String>,
        /// YAML or JSON verification evidence file, or - for stdin.
        #[arg(long, value_name = "PATH|-")]
        input: Option<String>,
        #[command(flatten)]
        format: SddFormatArgs,
    },
    /// Move a verified change from sdd/changes to sdd/archives.
    Archive {
        #[arg(long, value_name = "CHANGE-ID")]
        change: String,
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH")]
        path: Option<String>,
        #[command(flatten)]
        format: SddFormatArgs,
    },
}

#[derive(Args, Debug, Default)]
struct SddFormatArgs {
    /// Emit YAML (the default).
    #[arg(long, conflicts_with = "json")]
    yaml: bool,
    /// Emit JSON instead of the default YAML.
    #[arg(long, conflicts_with = "yaml")]
    json: bool,
}

#[derive(Debug, Subcommand)]
enum GraphCommand {
    /// Show node and link counts for the indexed wiki.
    Stats {
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,
    },

    /// List rule-extracted technical entities.
    Entities {
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,
        /// Optional substring filter for canonical entity names.
        #[arg(long, value_name = "QUERY")]
        query: Option<String>,
        /// Optional exact entity type filter, such as function or type.
        #[arg(long = "type", value_name = "TYPE")]
        entity_type: Option<String>,
        /// Maximum number of entities to return.
        #[arg(long, value_name = "N", default_value_t = 50)]
        limit: usize,
    },

    /// Show co-occurrence neighbors and evidence for one extracted entity.
    EntityNeighbors {
        /// Exact canonical entity name or entity id.
        #[arg(long, value_name = "ENTITY")]
        entity: String,
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,
        /// Maximum number of neighboring relations to return.
        #[arg(long, value_name = "N", default_value_t = 50)]
        limit: usize,
    },

    /// Show incoming and outgoing neighbors for a page path or title.
    Neighbors {
        /// Page path without .md, or an exact page title.
        #[arg(long, value_name = "ENTITY")]
        entity: String,
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,
        /// Maximum traversal depth. Values above 8 are capped.
        #[arg(long, value_name = "N", default_value_t = 1)]
        depth: usize,
        /// Which edge directions to traverse.
        #[arg(long, value_enum, default_value_t = GraphDirectionArg::Both)]
        direction: GraphDirectionArg,
    },

    /// Find a directed path following wiki-links from one page to another.
    Path {
        #[arg(long, value_name = "ENTITY")]
        from: String,
        #[arg(long, value_name = "ENTITY")]
        to: String,
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,
        /// Maximum number of links in the path. Values above 8 are capped.
        #[arg(long, value_name = "N", default_value_t = 4)]
        max_depth: usize,
    },

    /// Export all indexed nodes and wiki-links.
    Export {
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = GraphFormatArg::Json)]
        format: GraphFormatArg,
    },
}

#[derive(Debug, Subcommand)]
enum RagCommand {
    /// Retrieve direct lexical matches and nearby graph context.
    Search {
        /// Search query. Whitespace-separated terms use AND semantics in FTS5;
        /// --regex treats it as a case-insensitive Rust regex.
        #[arg(short = 'q', long = "query", value_name = "QUERY")]
        query: String,
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,
        /// Treat the query as a case-insensitive Rust regex.
        #[arg(long)]
        regex: bool,
        /// Maximum result chunks to return.
        #[arg(long, value_name = "N")]
        limit: usize,
        /// Page-link expansion depth. Values above 8 are capped.
        #[arg(long, value_name = "N", default_value_t = 1)]
        depth: usize,
        /// Embedding provider. `hash` is deterministic and fully offline.
        #[arg(long, value_enum, default_value_t = EmbeddingModeArg::None)]
        embedding: EmbeddingModeArg,
        /// Weight applied to lexical scores.
        #[arg(long, value_name = "WEIGHT", default_value_t = 1.0)]
        lexical_weight: f64,
        /// Weight applied to page/entity graph scores.
        #[arg(long, value_name = "WEIGHT", default_value_t = 1.0)]
        graph_weight: f64,
        /// Weight applied to embedding similarity scores.
        #[arg(long, value_name = "WEIGHT", default_value_t = 1.0)]
        vector_weight: f64,
        /// Output format.
        #[arg(long, value_enum, default_value_t = RagOutputFormatArg::Text)]
        format: RagOutputFormatArg,
    },

    /// Assemble bounded, source-aware context for a downstream answerer.
    Context {
        /// Search query.
        #[arg(short = 'q', long = "query", value_name = "QUERY")]
        query: String,
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,
        /// Treat the query as a case-insensitive Rust regex.
        #[arg(long)]
        regex: bool,
        /// Maximum result chunks to retrieve.
        #[arg(long, value_name = "N", default_value_t = 8)]
        limit: usize,
        /// Page-link expansion depth. Values above 8 are capped.
        #[arg(long, value_name = "N", default_value_t = 1)]
        depth: usize,
        /// Embedding provider. `hash` is deterministic and fully offline.
        #[arg(long, value_enum, default_value_t = EmbeddingModeArg::None)]
        embedding: EmbeddingModeArg,
        /// Weight applied to lexical scores.
        #[arg(long, value_name = "WEIGHT", default_value_t = 1.0)]
        lexical_weight: f64,
        /// Weight applied to page/entity graph scores.
        #[arg(long, value_name = "WEIGHT", default_value_t = 1.0)]
        graph_weight: f64,
        /// Weight applied to embedding similarity scores.
        #[arg(long, value_name = "WEIGHT", default_value_t = 1.0)]
        vector_weight: f64,
        /// Maximum context characters.
        #[arg(
            long,
            value_name = "N",
            default_value_t = rag::DEFAULT_CONTEXT_CHARS
        )]
        max_chars: usize,
        /// Output format.
        #[arg(long, value_enum, default_value_t = RagOutputFormatArg::Text)]
        format: RagOutputFormatArg,
    },

    /// Answer a question from bounded, source-aware Graph-RAG evidence.
    Ask {
        /// Natural-language question. Technical identifiers are used as a
        /// fallback query when the full question has no lexical match.
        #[arg(short = 'q', long = "query", value_name = "QUESTION")]
        query: String,
        /// Wiki root. Defaults to docs/.
        #[arg(long, value_name = "PATH", default_value = shared::DEFAULT_WIKI_ROOT)]
        path: Option<String>,
        /// Treat the query as a case-insensitive Rust regex.
        #[arg(long)]
        regex: bool,
        /// Maximum result chunks to retrieve.
        #[arg(long, value_name = "N", default_value_t = 8)]
        limit: usize,
        /// Page-link expansion depth. Values above 8 are capped.
        #[arg(long, value_name = "N", default_value_t = 1)]
        depth: usize,
        /// Embedding provider. hash is deterministic and fully offline.
        #[arg(long, value_enum)]
        embedding: Option<EmbeddingModeArg>,
        /// Weight applied to lexical scores.
        #[arg(long, value_name = "WEIGHT")]
        lexical_weight: Option<f64>,
        /// Weight applied to page/entity graph scores.
        #[arg(long, value_name = "WEIGHT")]
        graph_weight: Option<f64>,
        /// Weight applied to embedding similarity scores.
        #[arg(long, value_name = "WEIGHT")]
        vector_weight: Option<f64>,
        /// Maximum characters used for evidence and the extractive answer.
        #[arg(long, value_name = "N")]
        max_chars: Option<usize>,
        /// Answer provider. The built-in provider only quotes indexed evidence.
        #[arg(long, value_enum)]
        provider: Option<AskProviderArg>,
        /// OpenAI-compatible chat completions endpoint. Overrides wiki.yaml.
        #[arg(long, value_name = "URL")]
        endpoint: Option<String>,
        /// Model name. Overrides wiki.yaml.
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
        /// HTTP timeout in seconds. Overrides wiki.yaml.
        #[arg(long, value_name = "N")]
        timeout_secs: Option<u64>,
        /// Output format.
        #[arg(long, value_enum, default_value_t = AskOutputFormatArg::Text)]
        format: AskOutputFormatArg,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
#[clap(rename_all = "lowercase")]
enum HarnessOutputFormatArg {
    #[default]
    Text,
    Json,
    Prompt,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
#[clap(rename_all = "lowercase")]
enum RagOutputFormatArg {
    #[default]
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
#[clap(rename_all = "lowercase")]
enum AskOutputFormatArg {
    #[default]
    Text,
    Json,
    Prompt,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
#[clap(rename_all = "lowercase")]
enum EmbeddingModeArg {
    #[default]
    None,
    Hash,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
#[clap(rename_all = "lowercase")]
enum AskProviderArg {
    #[default]
    Extractive,
    #[value(name = "openai-compatible")]
    OpenAiCompatible,
}

impl From<AskProviderArg> for ask::AnswerProviderMode {
    fn from(value: AskProviderArg) -> Self {
        match value {
            AskProviderArg::Extractive => Self::Extractive,
            AskProviderArg::OpenAiCompatible => Self::OpenAiCompatible,
        }
    }
}

impl From<EmbeddingModeArg> for embedding::EmbeddingMode {
    fn from(value: EmbeddingModeArg) -> Self {
        match value {
            EmbeddingModeArg::None => Self::None,
            EmbeddingModeArg::Hash => Self::Hash,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
#[clap(rename_all = "lowercase")]
enum GraphDirectionArg {
    Incoming,
    Outgoing,
    #[default]
    Both,
}

impl From<GraphDirectionArg> for graph::GraphDirection {
    fn from(value: GraphDirectionArg) -> Self {
        match value {
            GraphDirectionArg::Incoming => Self::Incoming,
            GraphDirectionArg::Outgoing => Self::Outgoing,
            GraphDirectionArg::Both => Self::Both,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[clap(rename_all = "lowercase")]
enum GraphFormatArg {
    Json,
    Dot,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
#[clap(rename_all = "lowercase")]
enum SearchModeArg {
    /// Per-page summary: heading + one excerpt + match count, with
    /// `[[wiki-link]]` references.
    #[default]
    Summary,
    /// File paths only.
    Files,
    /// Matching lines with surrounding context.
    Content,
}

impl From<SearchModeArg> for search::SearchMode {
    fn from(value: SearchModeArg) -> Self {
        match value {
            SearchModeArg::Summary => Self::Summary,
            SearchModeArg::Files => Self::Files,
            SearchModeArg::Content => Self::Content,
        }
    }
}

enum CliError {
    Human(String),
    SddOutput(String),
}

impl From<String> for CliError {
    fn from(value: String) -> Self {
        Self::Human(value)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Human(message)) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
        Err(CliError::SddOutput(payload)) => {
            print!("{payload}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Command::Init {
            dir,
            url,
            force,
            dry_run,
        } => Ok(run_init(dir, url, force, dry_run)?),
        Command::Index {
            path,
            rebuild,
            chunk_chars,
        } => Ok(run_index(index::IndexOptions {
            path,
            rebuild,
            chunk_chars,
        })?),
        Command::Graph { command } => Ok(run_graph(command)?),
        Command::Harness {
            query,
            path,
            regex,
            limit,
            depth,
            embedding,
            lexical_weight,
            graph_weight,
            vector_weight,
            max_chars,
            format,
        } => Ok(run_harness(
            harness::HarnessOptions {
                path,
                query,
                regex,
                limit,
                depth,
                embedding_mode: embedding.into(),
                weights: rag::RagWeights {
                    lexical: lexical_weight,
                    graph: graph_weight,
                    vector: vector_weight,
                },
                max_chars,
            },
            format,
        )?),
        Command::Rag { command } => Ok(run_rag(command)?),
        Command::Sdd { command } => run_sdd(command),
        Command::Read {
            path,
            no_agents,
            no_index,
            log_last,
            index_head_limit,
            agents_head_limit,
            no_strict,
        } => Ok(run_read(&read::ReadOptions {
            path,
            include_agents: !no_agents,
            include_index: !no_index,
            include_log_last: log_last,
            index_head_limit,
            agents_head_limit,
            strict: !no_strict,
        })?),
        Command::Search {
            query,
            path,
            mode,
            regex,
            case_sensitive,
            category,
            per_file_limit,
            head_limit,
            context,
        } => {
            let case_insensitive = !case_sensitive;
            Ok(run_search(&search::SearchOptions {
                query,
                path,
                mode: mode.into(),
                case_insensitive,
                regex,
                per_file_limit,
                head_limit,
                category,
                context,
            })?)
        }
    }
}

fn run_sdd(command: SddCommand) -> Result<(), CliError> {
    match command {
        SddCommand::List { path, all, format } => {
            emit_sdd("sdd.list", format.json, sdd::list(path.as_deref(), all))
        }
        SddCommand::New {
            title,
            path,
            format,
        } => emit_sdd("sdd.new", format.json, sdd::new(path.as_deref(), &title)),
        SddCommand::Verify {
            change,
            path,
            input,
            format,
        } => run_sdd_verify(change, path, input, format.json),
        SddCommand::Archive {
            change,
            path,
            format,
        } => emit_sdd(
            "sdd.archive",
            format.json,
            sdd::archive(path.as_deref(), &change),
        ),
    }
}

fn emit_sdd(
    command: &str,
    json: bool,
    result: Result<sdd::SddResponse, sdd::SddError>,
) -> Result<(), CliError> {
    match result {
        Ok(response) => {
            let output = if json {
                serde_json::to_string_pretty(&response)
                    .map_err(|error| CliError::Human(error.to_string()))?
            } else {
                serde_yaml::to_string(&response)
                    .map_err(|error| CliError::Human(error.to_string()))?
            };
            print!("{output}");
            Ok(())
        }
        Err(error) => Err(CliError::SddOutput(sdd::error_output(
            command, &error, json,
        ))),
    }
}

fn load_sdd_input_with_format(
    command: &str,
    path: Option<&str>,
    json: bool,
) -> Result<Option<String>, CliError> {
    sdd::load_input(path)
        .map_err(|error| CliError::SddOutput(sdd::error_output(command, &error, json)))
}

fn run_sdd_verify(
    change: String,
    path: Option<String>,
    input: Option<String>,
    json: bool,
) -> Result<(), CliError> {
    let input = load_sdd_input_with_format("sdd.verify", input.as_deref(), json)?;
    emit_sdd(
        "sdd.verify",
        json,
        sdd::verify(path.as_deref(), &change, input.as_deref()),
    )
}

fn run_init(dir: PathBuf, url: Option<String>, force: bool, dry_run: bool) -> Result<(), String> {
    let plan = init::plan(init::KnowledgeOptions {
        dir,
        url: url.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }),
        force,
    })?;
    print_preview(plan.preview());
    if let Some(url) = plan.url() {
        println!("Source: <{url}>");
    } else {
        println!("Source: (none — no --url supplied)");
    }
    println!("Mode: {}", if dry_run { "dry-run" } else { "commit" });
    println!();

    if dry_run {
        let (writes, keeps) =
            plan.preview()
                .iter()
                .fold((0usize, 0usize), |(writes, keeps), item| {
                    match item.action {
                        init::KnowledgeAction::Create | init::KnowledgeAction::Overwrite => {
                            (writes + 1, keeps)
                        }
                        init::KnowledgeAction::Keep => (writes, keeps + 1),
                    }
                });
        println!("(Dry run: {writes} write(s) and {keeps} keep(s) planned; nothing was written.)");
        return Ok(());
    }

    let receipt = init::commit(plan)?;
    println!("Wrote {} target(s).", receipt.changed_targets);
    for note in &receipt.notes {
        println!("{note}");
    }
    Ok(())
}

fn print_preview(items: &[init::KnowledgeItem]) {
    println!("Knowledge-base seed preview:");
    if items.is_empty() {
        println!("  No changes.");
        return;
    }
    for item in items {
        println!(
            "  {:<9} {}",
            item.action.as_str(),
            shared::display_path(&item.path)
        );
    }
}

fn run_read(opts: &read::ReadOptions) -> Result<(), String> {
    let text = read::read(opts).map_err(|e| e.to_string())?;
    print!("{text}");
    Ok(())
}

fn run_index(opts: index::IndexOptions) -> Result<(), String> {
    let stats = index::index(&opts).map_err(|error| error.to_string())?;
    println!("Indexed {} Markdown page(s).", stats.scanned);
    println!("Database: {}", shared::display_path(&stats.database_path));
    println!("Added: {}", stats.added);
    println!("Updated: {}", stats.updated);
    println!("Unchanged: {}", stats.unchanged);
    println!("Removed: {}", stats.removed);
    println!("Chunks: {}", stats.chunks);
    println!("Wiki links: {}", stats.links);
    println!("Entities: {}", stats.entities);
    println!("Mentions: {}", stats.mentions);
    println!("Relations: {}", stats.relations);
    Ok(())
}

fn run_graph(command: GraphCommand) -> Result<(), String> {
    match command {
        GraphCommand::Stats { path } => {
            let stats =
                graph::stats(&graph::GraphOptions { path }).map_err(|error| error.to_string())?;
            print!("{}", graph::format_stats(&stats));
        }
        GraphCommand::Entities {
            path,
            query,
            entity_type,
            limit,
        } => {
            let entities = graph::entities(
                &graph::GraphOptions { path },
                query.as_deref(),
                entity_type.as_deref(),
                limit,
            )
            .map_err(|error| error.to_string())?;
            print!("{}", graph::format_entities(&entities));
        }
        GraphCommand::EntityNeighbors {
            entity,
            path,
            limit,
        } => {
            let (root, neighbors) =
                graph::entity_neighbors(&graph::GraphOptions { path }, &entity, limit)
                    .map_err(|error| error.to_string())?;
            print!("{}", graph::format_entity_neighbors(&root, &neighbors));
        }
        GraphCommand::Neighbors {
            entity,
            path,
            depth,
            direction,
        } => {
            let result = graph::neighbors(
                &graph::GraphOptions { path },
                &entity,
                depth,
                direction.into(),
            )
            .map_err(|error| error.to_string())?;
            print!("{}", graph::format_neighbors(&result));
        }
        GraphCommand::Path {
            from,
            to,
            path,
            max_depth,
        } => {
            let result = graph::path(&graph::GraphOptions { path }, &from, &to, max_depth)
                .map_err(|error| error.to_string())?;
            print!("{}", graph::format_path(&result));
        }
        GraphCommand::Export { path, format } => {
            let export =
                graph::export(&graph::GraphOptions { path }).map_err(|error| error.to_string())?;
            match format {
                GraphFormatArg::Json => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&export).map_err(|error| error.to_string())?
                    );
                }
                GraphFormatArg::Dot => print!("{}", graph::format_dot(&export)),
            }
        }
    }
    Ok(())
}

fn run_search(opts: &search::SearchOptions) -> Result<(), String> {
    let text = search::search(opts).map_err(|e| e.to_string())?;
    print!("{text}");
    Ok(())
}

fn run_harness(
    opts: harness::HarnessOptions,
    format: HarnessOutputFormatArg,
) -> Result<(), String> {
    let context = harness::build(&opts).map_err(|error| error.to_string())?;
    match format {
        HarnessOutputFormatArg::Text => print!("{}", harness::format_text(&context)),
        HarnessOutputFormatArg::Json => println!(
            "{}",
            harness::format_json(&context).map_err(|error| error.to_string())?
        ),
        HarnessOutputFormatArg::Prompt => print!("{}", harness::format_prompt(&context)),
    }
    Ok(())
}

fn run_rag(command: RagCommand) -> Result<(), String> {
    match command {
        RagCommand::Search {
            query,
            path,
            regex,
            limit,
            depth,
            embedding,
            lexical_weight,
            graph_weight,
            vector_weight,
            format,
        } => {
            let hits = rag::search(&rag::RagSearchOptions {
                path,
                query: query.clone(),
                regex,
                limit,
                depth,
                embedding_mode: embedding.into(),
                weights: rag::RagWeights {
                    lexical: lexical_weight,
                    graph: graph_weight,
                    vector: vector_weight,
                },
            })
            .map_err(|error| error.to_string())?;
            match format {
                RagOutputFormatArg::Text => print!("{}", rag::format_results(&query, &hits)),
                RagOutputFormatArg::Json => println!(
                    "{}",
                    rag::format_results_json(&query, &hits).map_err(|error| error.to_string())?
                ),
            }
        }
        RagCommand::Context {
            query,
            path,
            regex,
            limit,
            depth,
            embedding,
            lexical_weight,
            graph_weight,
            vector_weight,
            max_chars,
            format,
        } => {
            let hits = rag::search(&rag::RagSearchOptions {
                path,
                query: query.clone(),
                regex,
                limit,
                depth,
                embedding_mode: embedding.into(),
                weights: rag::RagWeights {
                    lexical: lexical_weight,
                    graph: graph_weight,
                    vector: vector_weight,
                },
            })
            .map_err(|error| error.to_string())?;
            match format {
                RagOutputFormatArg::Text => {
                    print!("{}", rag::format_context(&query, &hits, max_chars));
                }
                RagOutputFormatArg::Json => println!(
                    "{}",
                    rag::format_context_json(&query, &hits, max_chars)
                        .map_err(|error| error.to_string())?
                ),
            }
        }
        RagCommand::Ask {
            query,
            path,
            regex,
            limit,
            depth,
            embedding,
            lexical_weight,
            graph_weight,
            vector_weight,
            max_chars,
            provider,
            endpoint,
            model,
            timeout_secs,
            format,
        } => {
            let (config, _) = config::load_default().map_err(|error| error.to_string())?;
            let configured = &config.rag;
            let provider = match provider {
                Some(provider) => provider.into(),
                None => configured
                    .provider
                    .as_deref()
                    .map(parse_configured_provider)
                    .transpose()?
                    .unwrap_or_default(),
            };
            let embedding_mode = match embedding {
                Some(embedding) => embedding.into(),
                None => configured
                    .embedding
                    .as_deref()
                    .map(parse_configured_embedding)
                    .transpose()?
                    .unwrap_or_default(),
            };
            let max_chars = max_chars
                .or(configured.max_chars)
                .unwrap_or(rag::DEFAULT_CONTEXT_CHARS);
            let weights = rag::RagWeights {
                lexical: lexical_weight.or(configured.lexical_weight).unwrap_or(1.0),
                graph: graph_weight.or(configured.graph_weight).unwrap_or(1.0),
                vector: vector_weight.or(configured.vector_weight).unwrap_or(1.0),
            };
            let answer = ask::answer(&ask::AskOptions {
                question: query.clone(),
                retrieval: rag::RagSearchOptions {
                    path,
                    query: query.clone(),
                    regex,
                    limit,
                    depth,
                    embedding_mode,
                    weights,
                },
                provider,
                max_chars,
                openai: ask::OpenAiCompatibleOptions {
                    endpoint: endpoint.or_else(|| configured.endpoint.clone()),
                    model: model.or_else(|| configured.model.clone()),
                    api_key: configured.api_key.clone(),
                    api_key_env: configured.api_key_env.clone(),
                    timeout_secs: timeout_secs.or(configured.timeout_secs),
                },
            })
            .map_err(|error| error.to_string())?;
            match format {
                AskOutputFormatArg::Text => print!("{}", ask::format_answer(&answer)),
                AskOutputFormatArg::Json => println!(
                    "{}",
                    ask::format_answer_json(&answer).map_err(|error| error.to_string())?
                ),
                AskOutputFormatArg::Prompt => print!("{}", ask::format_prompt(&answer)),
            }
        }
    }
    Ok(())
}

fn parse_configured_provider(value: &str) -> Result<ask::AnswerProviderMode, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "extractive" => Ok(ask::AnswerProviderMode::Extractive),
        "openai-compatible" | "openai_compatible" | "openai" => {
            Ok(ask::AnswerProviderMode::OpenAiCompatible)
        }
        value => Err(format!(
            "invalid rag.provider '{}' in wiki.yaml; expected 'extractive' or 'openai-compatible'.",
            value
        )),
    }
}

fn parse_configured_embedding(value: &str) -> Result<embedding::EmbeddingMode, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(embedding::EmbeddingMode::None),
        "hash" => Ok(embedding::EmbeddingMode::Hash),
        value => Err(format!(
            "invalid rag.embedding '{}' in wiki.yaml; expected 'none' or 'hash'.",
            value
        )),
    }
}
