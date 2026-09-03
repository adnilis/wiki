# wiki

> 面向 Markdown、Graph-RAG 与 AI Agent 的证据优先知识基础设施。

`wiki` 是一个独立的 Rust CLI：把普通 Markdown 目录升级为可搜索、可链接、可审计、可供 Agent 直接消费的知识系统。

它坚持一条核心原则：

> **Markdown 是唯一事实源，其余一切都是可重建视图。**

`wiki` 覆盖从知识库初始化、上下文读取、全文与正则搜索，到本地索引、知识图谱、确定性 Graph-RAG、证据约束问答、Agent Harness，以及证据优先 SDD 变更治理的完整链路。

不需要常驻服务，不需要托管数据库；默认检索与摘录式回答也不需要 LLM。

## 为什么需要 wiki

项目知识往往散落在笔记、方案、决策、日志、代码引用、规格和 AI Prompt 中。`wiki` 用一套稳定的文件系统契约承载这些知识，再用确定性的本地运行时把它们交给人、脚本和 Agent 使用。

| 层次 | 职责 | 持久化事实 |
| --- | --- | --- |
| Markdown | 知识、决策、规格、历史记录 | `.md` 文件 |
| Index | 元数据、FTS5 分块、链接、实体与向量缓存 | `.wiki/index.sqlite` |
| Graph | 显式 `[[wiki-link]]` 与技术实体之间的导航关系 | Markdown + 索引 |
| Graph-RAG | 结合词法、图谱和可选本地向量的有界检索 | Markdown + 索引 |
| Harness | 面向 Agent 与自动化的一体化上下文协议 | CLI 输出 |
| SDD | 证据优先的变更生命周期与验证门禁 | `sdd/` 下的 Markdown |

最终得到的是一个本地、可检查、可脚本化、可随时重建的知识运行时。

## 能力全景

```text
                    ┌──────────────────────────────┐
                    │          Markdown 目录        │
                    │  notes / ideas / projects /   │
                    │  sdd / index.md / log.md      │
                    └──────────────┬───────────────┘
                                   │
                         wiki index│ 增量扫描
                                   ▼
                    ┌──────────────────────────────┐
                    │        本地 SQLite + FTS5     │
                    │ 分块 · 链接 · 实体 · 关系      │
                    └──────┬───────────────┬───────┘
                           │               │
                    wiki graph       wiki rag search
                           │               │
                           │       ┌───────▼────────┐
                           │       │ 有界证据检索    │
                           │       │ context / ask   │
                           │       └───────┬────────┘
                           │               │
                           └───────┬───────┘
                                   ▼
                    ┌──────────────────────────────┐
                    │        wiki harness           │
                    │ text · JSON · 约束型 Prompt   │
                    │ 来源与行号可追溯               │
                    └──────────────────────────────┘
```

## 五分钟上手

### 1. 构建二进制

```bash
cargo build --release
```

发布构建产物位于 `target/release/wiki`（Windows 下为 `wiki.exe`）。

### 2. 初始化知识库

```bash
wiki init --dir docs
```

该命令按规范生成目录结构，且不会访问网络。已有知识区目录、旧内容和遗留文件会被保留。使用 `--dry-run` 预览计划；需要覆盖生成的根文档时使用 `--force`。

### 3. 写入 Markdown 并建立索引

```bash
wiki index --path docs
```

索引是增量构建的。Markdown 永远是事实源，SQLite 只是可重建缓存。

### 4. 给 Agent 一个有边界的上下文包

```bash
wiki harness \
  --path docs \
  --query "认证流程如何工作" \
  --format json
```

如果要交给下游回答器：

```bash
wiki harness \
  --path docs \
  --query "认证流程如何工作" \
  --format prompt
```

JSON 协议版本为 `wiki.harness/v1`，包含索引回执、按查询命中的导航信息、有界任务证据、来源行号、截断状态与不确定性状态。

## 标准知识库结构

`wiki init` 生成如下布局：

```text
docs/
├── AGENTS.md              # 全局工作上下文与项目规则
├── index.md               # 目录索引与导航入口
├── SDD.md                 # 证据优先的 SDD 工作流
├── log.md                 # 只能追加的项目时间线
├── notes/                 # 已验证、可复用的权威知识
├── ideas/                 # 假设、偏好与决策理由
├── projects/              # 进行中的计划、实现记录与验证
├── sdd/
│   ├── changes/           # 活跃规格
│   └── archives/          # 已验证、已完成的规格
└── .wiki/
    └── index.sqlite       # 可重建的本地索引缓存
```

初始化是纯文件系统操作。`--url` 只会把源 URL 原样记录到 `AGENTS.md` 与 `index.md`，不会在初始化阶段抓取该地址。

## 命令地图

| 命令 | 作用 | 典型使用者 |
| --- | --- | --- |
| `wiki init` | 生成标准知识库布局 | 项目负责人 |
| `wiki read` | 读取结构、根上下文、索引和可选日志 | 人或 Agent |
| `wiki search` | 以摘要、文件或内容模式搜索 Markdown | 人或脚本 |
| `wiki index` | 构建或增量刷新 SQLite/FTS5 数据 | 本地工作流 |
| `wiki graph` | 查看、遍历、导出知识图谱 | 分析工具 |
| `wiki rag search` | 检索带词法/图谱扩展的排序分块 | 检索层 |
| `wiki rag context` | 组装带来源的有界证据包 | Prompt 构建器 |
| `wiki rag ask` | 使用摘录式或 OpenAI-compatible Provider 回答 | 助手 |
| `wiki harness` | 输出一体化 Agent 上下文包 | Agent Harness |
| `wiki sdd` | 创建、验证、列出、归档证据优先变更 | 工程工作流 |

所有可选路径默认指向 `docs/`。

## 直接读取与搜索 Markdown

### `wiki read`

读取知识库结构与根上下文：

```text
wiki read [--path <PATH>] [--no-agents] [--no-index] [--log-last N]
          [--index-head-limit LINES] [--agents-head-limit LINES] [--no-strict]
```

输出顺序固定为：

```
标题 → 全局上下文 → 索引 → 最近日志 → 完整性标记
```

常用参数：

- `--no-agents`、`--no-index`：省略对应根文档正文。
- `--log-last N`：输出最近的 `## [YYYY-MM-DD]` 日志条目；`0` 表示省略，硬上限为 50。
- `--index-head-limit`、`--agents-head-limit`：限制返回行数，硬上限均为 2,000。
- `--no-strict`：当目录没有 `AGENTS.md` 或 `index.md` 时，不输出普通 Markdown 树提示。

### `wiki search`

不依赖索引，直接搜索任意 Markdown 目录：

```text
wiki search --query <QUERY> [--path <PATH>] [--mode summary|files|content]
            [--regex] [--case-sensitive] [--category <AREA>]
            [--per-file-limit N] [--head-limit N] [--context N]
```

默认是大小写不敏感的字面匹配。包含多个词的字面查询按无序 AND 处理：每个词都必须出现在页面中，但不要求同一行或输入顺序一致；完整短语命中会优先排序。

输出模式：

- `summary`：页面标题、摘要、命中数量与 `[[wiki-link]]` 引用。
- `files`：只输出命中文件路径。
- `content`：输出带行号的命中行及合并后的上下文。

使用 `--regex` 可启用大小写不敏感的 Rust 正则；使用 `--category` 可限制到 `notes`、`ideas`、`projects`、`sdd` 或任意嵌套相对目录。

## 构建本地知识索引

```text
wiki index [--path <PATH>] [--rebuild] [--chunk-chars N]
```

索引包含：文档元数据、按标题感知的 Markdown Chunk、显式 `[[wiki-link]]` 关系、FTS5 全文检索数据、保守抽取的技术实体提及、带 Chunk 与行号证据的候选共现关系，以及可选的确定性 Hash Embedding。

重复运行时，只处理新增或修改的页面，并清理已删除页面的索引项。`--rebuild` 会在扫描前清理索引行，但绝不会修改 Markdown 目录。

实体抽取器会识别函数、类型、模块、命令、路径、配置键和代码风格标识符的声明或行内代码。同一 Chunk 中共同出现的实体只会形成检索提示，不代表已经确认的语义事实。

## 探索知识图谱

图谱命令读取 `wiki index` 建立的显式链接与实体数据：

```bash
wiki graph stats --path docs
wiki graph entities --path docs --query Role --type identifier
wiki graph entity-neighbors --path docs --entity RoleService --limit 20
wiki graph neighbors --path docs --entity notes/architecture --depth 2
wiki graph path --path docs --from notes/a --to notes/b --max-depth 4
wiki graph export --path docs --format json
wiki graph export --path docs --format dot
```

页面节点可以用不带 `.md` 的相对路径或精确标题选择。`neighbors` 支持 `incoming`、`outgoing`、`both` 三种方向；`path` 沿已解析的出边查找路径。无法解析的 dangling link 会保留在统计和导出中，不会被静默丢弃。

## 确定性 Graph-RAG

`wiki rag` 是证据优先自动化的检索层。它先用 FTS5 做词法召回，再沿页面链接、实体提及和候选实体关系扩展上下文。

### 检索排序后的证据

```bash
wiki rag search \
  --path docs \
  --query "RoleService" \
  --limit 8 \
  --depth 1
```

每个结果都包含分数、来源理由、页面、标题路径、行号范围、摘要，以及 lexical/graph/vector 三类分数分解。自动化场景使用 `--format json`。

默认检索完全离线。增加 `--embedding hash` 后启用确定性的本地 token/ngram Provider。向量按需生成，并连同 Chunk 文本 Hash 缓存；Chunk 变化后会自动重新计算。词法、图谱、向量权重可以独立调整。

### 组装有界 Context

```bash
wiki rag context \
  --path docs \
  --query "RoleService" \
  --limit 8 \
  --max-chars 8000 \
  --format json
```

Context 会保留来源路径、标题、行号、provenance、图谱理由、分数分解和 `truncated` 状态，并在输出前严格执行字符预算。

### 基于证据提问

```bash
wiki rag ask \
  --path docs \
  --query "What does RoleService do?" \
  --format json
```

内置 `extractive` Provider 只摘录索引中的证据，不调用 LLM，也不补写没有证据支持的事实。没有直接词法证据、Context 被截断或完全没有证据时，结果会明确标记 `uncertain`。

如果由独立的回答器生成最终文本，可以输出受约束 Prompt：

```bash
wiki rag ask \
  --path docs \
  --query "What does RoleService do?" \
  --format prompt
```

Prompt 要求下游回答器为事实声明使用编号来源，并在证据不足时明确说明不确定。

### 正则检索

`wiki rag search`、`wiki rag context`、`wiki rag ask` 都支持显式正则检索：

```bash
wiki rag search \
  --path docs \
  --regex \
  --query 'EventStation|STATION_ING' \
  --format json
```

启用 `--regex` 后，query 会作为大小写不敏感的 Rust 正则直接扫描索引 Chunk，不再拆分为普通回退词项。

## Agent 的单一接入入口：`wiki harness`

需要把项目上下文交给 Agent 或自动化层时，推荐使用 `wiki harness`。

```text
wiki harness --path docs --query <TASK_OR_QUESTION>
wiki harness --path docs --query <TASK_OR_QUESTION> --format json
wiki harness --path docs --query <TASK_OR_QUESTION> --format prompt
```

一次调用完成：增量刷新 `PATH/.wiki/index.sqlite`，生成按 query 命中的 Search 导航，检索任务相关 Graph-RAG 证据，执行总字符预算，并输出 text、`wiki.harness/v1` JSON 或证据约束 Prompt。

Harness 会刻意将 `AGENTS.md` 与 `log.md` 排除在任务搜索证据之外。需要完整根规则或最近时间线时，使用 `wiki read`；Harness 本身不会修改 Markdown。

### Harness 输出模式

| 格式 | 面向对象 | 特点 |
| --- | --- | --- |
| `text` | 人 | 便于检查索引回执与证据 |
| `json` | 脚本与 Agent Runtime | 稳定的 `wiki.harness/v1` 协议 |
| `prompt` | 独立 LLM 或回答器 | Search 只做导航，编号 Evidence 才可引用 |

自然语言查询会先尝试完整问题；没有直接命中时，再回退到有限数量的技术词或中文片段。中英文混合查询会在语言边界处拆分。`uncertain` 会把直接证据缺失或上下文截断传递给下游。

PowerShell 接入示例：

```powershell
$context = wiki harness --path docs --query $task --format json
```

## 证据优先的 Spec-Driven Development

`wiki sdd` 提供基于文件系统的变更生命周期：

```
new → Agent 编写 Markdown 规格与任务 → verify → archive
```

命令：

```bash
wiki sdd new "实现订单取消功能"
wiki sdd list
wiki sdd list --all
wiki sdd verify --change <CHANGE-ID>
wiki sdd verify --change <CHANGE-ID> --input <PATH|->
wiki sdd archive --change <CHANGE-ID>
```

契约如下：

- 进行中的变更位于 `sdd/changes/<change-id>/`；
- 完成后的变更整体移动到 `sdd/archives/<change-id>/`；
- `wiki sdd new` 返回路径和写作说明，Agent 直接维护 `spec/index.md`、`spec/requirements/REQ-*.md` 与 `tasks/` 下的 Markdown；
- Markdown 是唯一持久化规格事实源，不要求额外的规格 JSON/YAML 文件；
- `wiki sdd verify` 做结构验证，也可通过 `--input` 接收宿主提供的 YAML/JSON 验证证据；
- `wiki sdd archive` 会再次检查验证指纹，并要求所有未废弃任务处于 `done` 或 `dropped`；
- 响应默认输出 YAML；使用 `--json` 输出 JSON，`--yaml` 可显式声明默认格式，且与 `--json` 互斥。

生成的 `SDD.md` 记录 evidence-first 契约：探索、证据链、决策、方案比较、影响分析、风险控制、发布护栏、测试策略、需求与验收条件。

## 配置

`wiki rag ask` 会尝试从可执行文件所在目录读取 `wiki.yaml`。命令行参数优先于配置文件；环境变量作为 Provider 配置兜底。

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

使用 `openai-compatible` 时，也可以通过 CLI 参数或 Provider 环境变量提供 endpoint 和 model。内置 extractive Provider 始终是零网络依赖的默认路径。

## 设计原则

- **事实源优先**：Markdown 始终权威，且可以被普通编辑器直接修改；
- **可重建**：删除本地索引不会造成知识丢失；
- **默认确定性**：初始化、索引、搜索、图谱和摘录式回答不要求远程服务；
- **证据高于自信**：输出保留路径、标题、行号、provenance、不确定性与截断状态；
- **组合优先**：人类可读文本、机器 JSON、下游 Prompt 都是一等输出；
- **增量处理**：未变化页面不会被无谓重复处理；
- **文件系统原生**：Git、代码评审、普通编辑器和既有 Markdown 流程仍是主要协作界面。

## 退出码

| 码 | 含义 |
| --- | --- |
| `0` | 成功 |
| `1` | 调用方或输入错误，例如路径不存在、查询为空或正则非法 |

## 开发与验证

```bash
cargo fmt --check
cargo check
cargo test
```

测试覆盖初始化、读取、字面与正则搜索、增量索引、Graph-RAG、来源约束回答、Harness 协议、SDD 响应与验证证据、生命周期流转和归档移动。

## 与 `fastctx` 的兼容关系

本 CLI 将 [`fastctx`](https://github.com/yc-duan/fastctx) 中的 wiki 能力重新实现为独立二进制：

| `fastctx` 能力 | `wiki` 命令 |
| --- | --- |
| `wiki_init` | `wiki init` |
| `wiki_read` | `wiki read` |
| `wiki_search` | `wiki search` |
| `fastctx init knowledge` | `wiki init --dir docs --force` |

种子布局、知识区约定、`## [YYYY-MM-DD]` 日志前缀和 `[[wiki-link]]` 摘要渲染保持兼容。

## License

MIT OR Apache-2.0
