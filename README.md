# wiki

> Evidence-first knowledge infrastructure for Markdown, Graph-RAG, and AI agents.

`wiki` is a standalone Rust CLI that turns an ordinary Markdown directory into a searchable, link-aware, agent-ready knowledge system.

It is designed around one uncompromising rule:

> **Markdown is the source of truth. Everything else is a rebuildable view.**

The CLI gives people and automation a coherent workflow for creating knowledge bases, reading project context, searching with literal or regex queries, building a local graph and full-text index, retrieving bounded evidence, asking source-aware questions, and running evidence-first Spec-Driven Development.

No daemon is required. No hosted database is required. No LLM is required for the default retrieval and extractive answer flows.

## Why wiki exists

Knowledge becomes fragmented across notes, project plans, decisions, logs, code references, and AI prompts. `wiki` provides a durable filesystem contract for that knowledge and a deterministic runtime for consuming it.

| Layer | Responsibility | Durable source |
| --- | --- | --- |
| Markdown | Human-authored knowledge, decisions, specifications, and history | `.md` files |
| Index | Incremental metadata, FTS5 chunks, links, entities, and cached vectors | `.wiki/index.sqlite` |
| Graph | Navigation through explicit `[[wiki-link]]` edges and extracted technical entities | Markdown + index |
| Graph-RAG | Bounded retrieval with lexical, graph, and optional local-vector signals | Markdown + index |
| Harness | One stable context package for agents and automation | CLI output |
| SDD | Evidence-first change lifecycle and verification gates | Markdown under `sdd/` |

The result is a small local knowledge runtime that remains inspectable by humans, scriptable by machines, and safe to rebuild when the index is deleted.

## Capability map

```text
                    ┌──────────────────────────────┐
                    │         Markdown tree         │
                    │  notes / ideas / projects /   │
                    │  sdd / index.md / log.md      │
                    └──────────────┬───────────────┘
                                   │
                         wiki index│ incremental scan
                                   ▼
                    ┌──────────────────────────────┐
                    │      Local SQLite + FTS5      │
                    │ chunks · links · entities     │
                    │ mentions · relations         │
                    └──────┬───────────────┬───────┘
                           │               │
                    wiki graph       wiki rag search
                           │               │
                           │       ┌───────▼────────┐
                           │       │ bounded evidence│
                           │       │ context / ask   │
                           │       └───────┬────────┘
                           │               │
                           └───────┬───────┘
                                   ▼
                    ┌──────────────────────────────┐
                    │       wiki harness           │
                    │ text · JSON · downstream     │
                    │ prompt with provenance       │
                    └──────────────────────────────┘
```

## Quick start

### 1. Build the binary

```bash
cargo build --release
```

The release binary is written to `target/release/wiki` (`wiki.exe` on Windows).

### 2. Seed a knowledge base

```bash
wiki init --dir docs
```

This creates the canonical layout without network access. Existing knowledge-area directories and legacy content are preserved. Use `--dry-run` to preview the plan, or `--force` when the generated root documents should be replaced.

### 3. Add Markdown and build the local index

```bash
wiki index --path docs
```

The index is incremental. Markdown remains authoritative; SQLite is only a rebuildable cache.

### 4. Give an agent one bounded context package

```bash
wiki harness \
  --path docs \
  --query "how does authentication work" \
  --format json
```

For a downstream answerer:

```bash
wiki harness \
  --path docs \
  --query "how does authentication work" \
  --format prompt
```

The JSON contract is `wiki.harness/v1`. It includes the index receipt, query-matched navigation, bounded task evidence, source line ranges, truncation state, and uncertainty state.

## Canonical knowledge-base layout

`wiki init` seeds this structure:

```text
docs/
├── AGENTS.md              # global operating context and project rules
├── index.md               # catalog and navigation entry point
├── SDD.md                 # evidence-first SDD workflow
├── log.md                 # append-only project chronology
├── notes/                 # verified, reusable source-of-truth knowledge
├── ideas/                 # hypotheses, preferences, and decision rationale
├── projects/              # active plans, implementation notes, and verification
├── sdd/
│   ├── changes/           # active specifications
│   └── archives/          # verified, completed specifications
└── .wiki/
    └── index.sqlite       # rebuildable local index cache
```

The generated seed is intentionally filesystem-first. `--url` records a source URL in `AGENTS.md` and `index.md`; initialization itself never fetches it.

## Command surface

| Command | Purpose | Typical consumer |
| --- | --- | --- |
| `wiki init` | Seed the canonical knowledge-base layout | Project owner |
| `wiki read` | Read structure, root context, index, and optional recent log entries | Human or agent |
| `wiki search` | Search any Markdown tree with summaries, files, or content | Human or script |
| `wiki index` | Build or incrementally refresh SQLite/FTS5 data | Local workflow |
| `wiki graph` | Inspect, traverse, and export the knowledge graph | Analyst or tool |
| `wiki rag search` | Retrieve ranked chunks with lexical and graph expansion | Retrieval layer |
| `wiki rag context` | Assemble bounded evidence with provenance | Prompt builder |
| `wiki rag ask` | Answer from evidence using extractive or OpenAI-compatible providers | Assistant |
| `wiki harness` | Combine index navigation and task evidence into one stable package | Agent harness |
| `wiki sdd` | Create, verify, list, and archive evidence-first changes | Engineering workflow |

All commands default to `docs/` where a path is optional.

## Read and search Markdown directly

### `wiki read`

Read the structure of a wiki and its root context:

```text
wiki read [--path <PATH>] [--no-agents] [--no-index] [--log-last N]
          [--index-head-limit LINES] [--agents-head-limit LINES] [--no-strict]
```

The output order is stable:

```
header → global context → index → recent log → completeness marker
```

Useful controls:

- `--no-agents` and `--no-index` remove the corresponding root document body.
- `--log-last N` includes the newest recognized `## [YYYY-MM-DD]` entries; `0` omits the log section and the hard ceiling is 50.
- `--index-head-limit` and `--agents-head-limit` cap returned lines, each with a hard ceiling of 2,000.
- `--no-strict` suppresses the hint shown for a plain Markdown tree without `AGENTS.md` or `index.md`.

### `wiki search`

Search without an index when a direct filesystem view is enough:

```text
wiki search --query <QUERY> [--path <PATH>] [--mode summary|files|content]
            [--regex] [--case-sensitive] [--category <AREA>]
            [--per-file-limit N] [--head-limit N] [--context N]
```

Default behavior is case-insensitive literal matching. A multi-word literal query is treated as an unordered AND: every token must appear in the page, but not necessarily on the same line or in the same order. Exact phrase hits rank first.

Output modes:

- `summary` — page heading, excerpt, match count, and `[[wiki-link]]` references.
- `files` — matching paths only.
- `content` — matching lines with line numbers and merged surrounding context.

Use `--regex` for a case-insensitive Rust regular expression, and `--category` to constrain the search to `notes`, `ideas`, `projects`, `sdd`, or any nested relative directory.

## Build the local knowledge index

```text
wiki index [--path <PATH>] [--rebuild] [--chunk-chars N]
```

The index stores document metadata, heading-aware Markdown chunks, explicit `[[wiki-link]]` records, FTS5 full-text search data, conservative technical-entity mentions, candidate co-occurrence relations with chunk and line evidence, and optional deterministic hash embeddings.

Re-running `wiki index` only processes new or changed pages and removes entries for deleted pages. `--rebuild` clears indexed rows before scanning but never modifies the Markdown tree.

The entity extractor recognizes declarations and inline code for functions, types, modules, commands, paths, configuration keys, and code-style identifiers. Co-occurrence relations are retrieval hints, not asserted semantic facts.

## Explore the knowledge graph

Graph commands operate on the explicit links and entities stored by `wiki index`:

```bash
wiki graph stats --path docs
wiki graph entities --path docs --query Role --type identifier
wiki graph entity-neighbors --path docs --entity RoleService --limit 20
wiki graph neighbors --path docs --entity notes/architecture --depth 2
wiki graph path --path docs --from notes/a --to notes/b --max-depth 4
wiki graph export --path docs --format json
wiki graph export --path docs --format dot
```

Page nodes can be selected by relative path without `.md` or by exact title. `neighbors` supports `incoming`, `outgoing`, and `both` directions. `path` follows resolved outgoing links. Dangling links remain visible in statistics and exports instead of disappearing silently.

## Deterministic Graph-RAG

`wiki rag` is the retrieval layer for evidence-grounded automation. It starts with FTS5 lexical matching, then expands through page links, entity mentions, and candidate entity relations.

### Search ranked evidence

```bash
wiki rag search \
  --path docs \
  --query "RoleService" \
  --limit 8 \
  --depth 1
```

Every result includes its score, source reason, page, heading path, line range, excerpt, and lexical/graph/vector score breakdown. Use `--format json` for automation.

The default retrieval path is fully offline. Add `--embedding hash` to enable the deterministic local token/ngram provider. Vectors are generated lazily and cached with the chunk text hash, so changed chunks are recomputed automatically. Lexical, graph, and vector weights can be adjusted independently.

### Assemble bounded context

```bash
wiki rag context \
  --path docs \
  --query "RoleService" \
  --limit 8 \
  --max-chars 8000 \
  --format json
```

The context package preserves source paths, headings, line ranges, provenance, graph reasons, score components, and the `truncated` state. The character budget is enforced before output is returned.

### Ask from evidence

```bash
wiki rag ask \
  --path docs \
  --query "What does RoleService do?" \
  --format json
```

The built-in `extractive` provider only quotes indexed evidence. It does not call an LLM and does not invent unsupported facts. If no direct lexical evidence exists, evidence is truncated, or no evidence is found, the result is explicitly marked `uncertain`.

For a separate answerer, emit a constrained prompt:

```bash
wiki rag ask \
  --path docs \
  --query "What does RoleService do?" \
  --format prompt
```

The prompt requires factual claims to cite numbered source markers and asks the downstream model to state uncertainty when the evidence is insufficient.

### Regex retrieval

`wiki rag search`, `wiki rag context`, and `wiki rag ask` support explicit regex retrieval:

```bash
wiki rag search \
  --path docs \
  --regex \
  --query 'EventStation|STATION_ING' \
  --format json
```

When `--regex` is enabled, the query is matched as a case-insensitive Rust regex against indexed chunks and is not split into ordinary fallback terms.

## One integration call for agents: `wiki harness`

`wiki harness` is the recommended integration boundary for projects that need to hand context to an agent or automation layer.

```text
wiki harness --path docs --query <TASK_OR_QUESTION>
wiki harness --path docs --query <TASK_OR_QUESTION> --format json
wiki harness --path docs --query <TASK_OR_QUESTION> --format prompt
```

One invocation incrementally refreshes `PATH/.wiki/index.sqlite`, builds query-matched search navigation, retrieves task-specific Graph-RAG evidence, enforces a combined character budget, and emits text, `wiki.harness/v1` JSON, or an evidence-constrained prompt.

The Harness package intentionally excludes `AGENTS.md` and `log.md` from task search evidence. Use `wiki read` when full root rules or recent chronology are needed. Markdown is never modified by Harness.

### Harness output modes

| Format | Designed for | Key property |
| --- | --- | --- |
| `text` | Humans | Easy inspection of index receipt and evidence |
| `json` | Scripts and agent runtimes | Stable `wiki.harness/v1` contract |
| `prompt` | A separate LLM or answerer | Search index is navigation-only; numbered Evidence is citable |

Natural-language retrieval first tries the complete query and then falls back to bounded technical terms or Chinese fragments when needed. Mixed ASCII/CJK queries are split at language boundaries. The `uncertain` flag communicates missing direct evidence or context truncation to downstream consumers.

PowerShell integration example:

```powershell
$context = wiki harness --path docs --query $task --format json
```

## Evidence-first Spec-Driven Development

`wiki sdd` provides a filesystem-native change lifecycle:

```
new → Agent writes Markdown spec and tasks → verify → archive
```

Commands:

```bash
wiki sdd new "Implement order cancellation"
wiki sdd list
wiki sdd list --all
wiki sdd verify --change <CHANGE-ID>
wiki sdd verify --change <CHANGE-ID> --input <PATH|->
wiki sdd archive --change <CHANGE-ID>
```

Behavioral contract:

- Active changes live under `sdd/changes/<change-id>/`.
- Completed changes move as a whole directory to `sdd/archives/<change-id>/`.
- `wiki sdd new` returns the path and instructions for the Agent to write `spec/index.md`, `spec/requirements/REQ-*.md`, and `tasks/` Markdown.
- Markdown is the persistent specification authority; no specification JSON or YAML file is required.
- `wiki sdd verify` validates structure and can consume host-provided YAML or JSON verification evidence through `--input`.
- `wiki sdd archive` rechecks that verification is current and that all non-deprecated tasks are `done` or `dropped` before moving the change.
- Responses are YAML by default. Add `--json` for JSON; `--yaml` is an explicit declaration of the default and conflicts with `--json`.

The generated `SDD.md` documents the evidence-first contract: exploration, evidence chain, decisions, alternatives, impact analysis, risk controls, release guardrails, test strategy, requirements, and acceptance criteria.

## Configuration

`wiki rag ask` optionally loads `wiki.yaml` from the directory containing the executable. CLI values override the file; environment variables are provider fallbacks.

```yaml
rag:
  provider: extractive
  # provider: openai-compatible
  endpoint: http://127.0.0.1:9000/v1/chat/completions
  model: local-model
  api_key_env: WIKI_RAG_LLM_API_KEY
  timeout_secs: 30
  embedding: hash
  lexical_weight: 1.0
  graph_weight: 0.8
  vector_weight: 0.7
  max_chars: 8000
```

For `openai-compatible`, the endpoint and model can also be supplied through CLI flags or provider environment variables. The built-in extractive provider remains the zero-network default.

## Design principles

- **Source-first** — Markdown is authoritative and always remains editable.
- **Rebuildable** — the local index can be deleted and recreated without data loss.
- **Deterministic by default** — initialization, indexing, search, graph queries, and extractive answers do not require a remote service.
- **Evidence over confidence** — outputs preserve paths, headings, line ranges, provenance, uncertainty, and truncation state.
- **Composable** — human-readable text, machine-readable JSON, and downstream prompts are first-class output forms.
- **Incremental** — unchanged pages are not needlessly reprocessed.
- **Filesystem-native** — Git, code review, ordinary editors, and existing Markdown workflows remain the primary collaboration surface.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Success |
| `1` | Caller or input error, such as a missing path, empty query, or invalid regex |

## Development

```bash
cargo fmt --check
cargo check
cargo test
```

The test suite covers initialization, reading, literal and regex search, incremental indexing, Graph-RAG retrieval, source-aware answering, Harness contracts, SDD responses and verification evidence, lifecycle transitions, and archive moves.

## Compatibility with `fastctx`

This CLI re-implements the wiki capabilities of [`fastctx`](https://github.com/yc-duan/fastctx) as a standalone binary:

| `fastctx` capability | `wiki` command |
| --- | --- |
| `wiki_init` | `wiki init` |
| `wiki_read` | `wiki read` |
| `wiki_search` | `wiki search` |
| `fastctx init knowledge` | `wiki init --dir docs --force` |

The seed layout, knowledge-area conventions, `## [YYYY-MM-DD]` log prefix, and `[[wiki-link]]` summary rendering are kept compatible with the original wiki workflow.

## License

MIT OR Apache-2.0
