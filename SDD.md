# SDD — Spec-Driven Development

知识库 SDD 规范，`wiki init` 写入新库的 canonical 模板。参考 E:/skills/sdd-driven-dev 与
OpenSpec Explore：探索优先、证据优先、人工确认。

**规格与任务由 Agent 手写 Markdown；CLI 只建 change、只读校验、记证据、归档。**

## 1. CLI 边界

~~~text
new → 写 spec → 拆 tasks → verify → archive
~~~

~~~bash
wiki sdd new "实现订单取消功能"                     # 同 --json
wiki sdd list                                      # 只列出进行中的变更
wiki sdd list --all                                # 同时列出进行中与已归档变更
wiki sdd verify --change <id>                       # 结构校验
wiki sdd verify --change <id> --input verify.yaml   # 证据校验
wiki sdd archive --change <id>
~~~

- CLI 从不生成或修改 `spec/`、`tasks/` 下文件。
- 无 spec / design / plan / task / build 子命令；不接收这些内容的 YAML/JSON 输入。任务与
  规格同为 Agent 手写的 Markdown，CLI 仅只读校验。
- `verify --input` 的 YAML 只是证据快照，不持久化、不生成规格或任务。
- 默认 YAML；`--yaml` 显式声明，`--json` 输出 JSON，互斥。

## 2. 生命周期与目录

~~~text
draft ── 写 spec / 拆 tasks ──> draft ──structure pass──> verified ──archive──> archived
                                   └─fail─> needs_fix ─────┘  evidence fail ──┘
~~~

- `verified` 后**任何 `spec/`、`tasks/` 改动都使结论失效，必须重跑 verify**。
- 任务状态：`todo → doing → done`；`*` → `blocked`（必填理由与解除条件）→ `doing`/`dropped`；
  `done` 重开须在 Log 记因；→ `dropped` 须填理由并先转移其独占 AC。

~~~text
<KB_ROOT>/sdd/
├── changes/<change-id>/
│   ├── change.md                    # CLI：标题、状态、时间、原始需求
│   ├── spec/index.md                # Agent：总览、探索、决策、影响、测试策略
│   ├── spec/requirements/REQ-001.md
│   ├── tasks/index.md               # Agent：看板、执行顺序、覆盖矩阵
│   ├── tasks/TASK-001.md
│   └── verify.md                    # CLI：校验时间、模式、结论、问题清单
└── archives/<change-id>/            # 只读
~~~

> `tasks/` 与 `spec/` 平级：规格是事实契约，任务是执行计划，分层避免任务抖动污染规格。
> Markdown 是唯一事实来源，不写旁路 `spec.yaml` / `tasks.json`。

ID 在 change 内**唯一稳定，创建后不复用**：`REQ-*` `AC-*`/`ACC-*` `E-*` `TASK-*` `VERIFY-*`。

| 字段 | 取值 |
|---|---|
| Priority（需求/任务） | `must` / `should` / `may` |
| 需求 Status | `proposed`（原"待实现"）/ `implemented` / `verified` / `dropped` |
| 任务 Status | `todo` / `doing` / `blocked` / `done` / `dropped` |
| 任务 Type | `code` / `test` / `config` / `data` / `docs` / `review` / `release` |
| 风险等级 | 低 / 中 / 高 |

## 3. Agent 工作方法

1. **读上下文**：读 `AGENTS.md` 与本文件；用 `wiki read --path <KB_ROOT>`、`wiki search`、
   `wiki rag` 定位 notes、projects、历史 change、代码入口、测试与接口文档。
2. **探索取证**：沿入口、状态机、调用链、DB/KV、缓存、消息、配置、重试、并发、最终生效条件
   探查。事实必绑 `E-*`；无证据的只能进"推测"或"开放问题"。
3. **直接写文件**：`wiki sdd new` 后按 `actionRequired.path` 写 `spec/index.md`、
   `spec/requirements/REQ-*.md`、`tasks/index.md`、`tasks/TASK-*.md`。不回传 YAML。
4. **规格 ≠ 实现计划**：规格写方案、边界、证据、验收；任务把需求翻译成可执行步骤。**规格
   驱动任务，任务不反向定义需求。** 发现规格缺口，先补 `REQ-*`/`AC-*`，再补任务，再重跑 verify。
5. **拆任务**：以"一次可独立验证的交付"为粒度；每个任务绑 `Requirement refs` +
   `Acceptance refs`，写可观察的完成定义；`Depends on` 表达无环顺序并标出可并行批次；测试、
   数据迁移、配置与灰度、文档、人工复核都要单独成任务。推进时只改状态与 Log。

**范围变化**：改需求/验收 → 同步覆盖矩阵 → 重跑 verify；调顺序/拆并任务 → ID 不复用、Log 记因
→ 重跑；废弃任务 → 填理由、转移独占 AC → 重跑；仅补测试输出 → 改 `verify.yaml` → 重跑 evidence。

## 4. spec/index.md 契约

按序组织，重要变更不得省略：

~~~text
# 规格
## 摘要  ## 问题  ## 变更类型  ## 目标  ## 非目标  ## 假设  ## 约束
## 风险  ## 开放问题  ## 证据链  ## 探索记录  ## 决策记录  ## 方案比较
## 影响分析矩阵  ## 接口契约与外部对齐  ## 外部接口依赖与版本依据
## 非代码改动与人工对齐  ## 高危操作及人工复核  ## 风险分级与发布护栏
## 测试策略  ## 需求  ## 任务
~~~

- **证据链**：仅三类——`[CODE]`（路径/符号/行号+结论）、`[SDD]`（文档路径/章节+结论）、
  `[CMD]`（可复现命令+输出摘要+结论）。任务只引用 `E-*`，不新增第四类；执行产生的命令输出
  先登记到证据链再被引用。
- **探索/决策/方案比较**：探索记现状、发现、未知项；决策记问题、决定、理由、证据；多方案
  逐项写收益/成本/风险并明确一个已选方案。不要把探索写成"已实现"。
- **影响矩阵**：覆盖调用链上下游、DB/KV/缓存/索引/消息、HTTP/RPC/SDK 契约、配置与灰度、
  监控告警、回滚触发、版本与存量数据兼容。
  `| 维度 | 对象 | 变更 | 兼容性 | 风险 | 缓解措施 | 证据 |`
- **接口**：类型、标识、变更、调用方、兼容性、外部对齐责任人。**外部依赖**：来源文档、版本、
  API、现状、冲突、解决方式、确认结论。**非代码改动**：配置/DB/KV/脚本/部署参数、执行方、
  时机、人工确认点。**高危操作**：重试、超时、熔断、降级、批量、锁、Lua/CAS、资源释放、
  并发触点及复核点。
- **高风险护栏**：灰度（范围/比例/时长）、回滚（开关/版本/数据处理/耗时）、监控阈值
  （指标/窗口/阈值）、告警（触发条件/责任人）、审批（谁在何时批准）。
- **测试策略**：写单元、集成、回归、命令与成功标准，描述"怎么验证"，不替代 verify 证据，
  也不替代任务里的测试项。
- **需求/任务章节**只放索引：需求表格（`ID | 标题 | 优先级 | 状态 | 验收项 | 文件`）、
  任务摘要（清单路径、总数、已完成、关键路径、覆盖状态）。

## 5. REQ-*.md 契约

~~~markdown
# REQ-001：用户可以取消可取消状态的订单

- Priority: must
- Status: proposed
- Source refs: E-001
- Dependencies: 无

## 边界场景
- 已支付且超过取消时限的订单不能取消。
- 重复取消请求必须幂等。

## 验收条件
### AC-001
- Given：订单处于待支付且未超时状态
- When：用户提交取消请求
- Then：订单进入已取消状态，并发布一次库存释放事件
- Test cases: cancel-pending-order
- Regression scope: order-detail, order-list
- Evidence refs: E-001
~~~

规则：`REQ-*`、`AC-*`/`ACC-*` 不重复；`must` 必须有验收；依赖只指向本 change 的 `REQ-*`，
无自引用与循环；验收须可执行、可观察、能映射到测试或证据；废弃验收标 `dropped`，不复用编号。

## 6. tasks/ 契约

### 6.1 tasks/index.md

~~~markdown
# 任务
- Has tasks: true  ｜ 总数 7（must 5 / should 2）｜ 已完成 3/7

## 看板
| 状态 | 任务 | 覆盖 | 阻塞/备注 |
|---|---|---|---|
| doing | TASK-003 | REQ-001 / AC-002 | 等 TASK-001 合入 |

## 执行顺序
- 批次 1（可并行）：TASK-001, TASK-002 ｜ 批次 2：TASK-003 ｜ 批次 3：TASK-005, TASK-006
- 关键路径：TASK-001 → TASK-003 → TASK-005

## 覆盖矩阵
| REQ | 验收 | 任务 | 状态 | 证据 |
|---|---|---|---|---|
| REQ-001 | AC-001 | TASK-001 | done | E-003 |
| REQ-001 | AC-002 | TASK-003 | doing | - |
~~~

- 覆盖矩阵必须覆盖**每个 must 需求的每个 AC**，有空洞即校验失败。
- `Has tasks: false` 适用于单提交即可完成的微小 change，此时可不建 `TASK-*.md`，但 must
  验收仍须在 verify 证据中体现。

### 6.2 tasks/TASK-*.md

~~~markdown
# TASK-003：实现取消接口的幂等分支

- Priority: must ｜ Status: doing ｜ Type: code
- Requirement refs: REQ-001
- Acceptance refs: AC-002
- Evidence refs: E-001, E-002
- Depends on: TASK-001
- Blocked reason: 无

## 范围
在 cancelOrder 入口增加幂等键分支，重复请求直接返回首次结果，不重复发布库存事件。

## 完成定义
- [ ] cancelOrder 增加幂等键读写，命中时跳过状态机与事件发布
- [ ] 新增单元测试 idempotent-cancel，断言事件仅发布一次
- [ ] 回归 order-detail、order-list 通过
- [ ] 证据命令输出已登记为 E-003

## 复核与回退
- Review: 幂等键过期时间与并发写入路径需人工复核（@owner）
- Rollback: 关闭开关 order.cancel.idempotent，回退上一版本，约 5 分钟

## Log
- 2026-09-02 todo → doing：开始实现，依赖 TASK-001 已合入
~~~

规则：

- `TASK-*` 唯一稳定，废弃不复用。
- `Requirement refs` / `Acceptance refs` 必须指向本 change 存在的标识符；无需求来源即越界
  任务，校验失败。
- 覆盖 must 需求或其 AC 的任务，`Priority` 必为 `must`。
- `Depends on` 只指向本 change 的 `TASK-*`，无自引用、无环。
- `blocked` 必填理由与解除条件；`dropped` 必填理由并先转移独占 AC。
- 每项任务必须有**完成定义**：可勾选、可观察、能映射到测试/命令/人工确认与 `AC-*`。

粒度：建议 0.5–2 天；>3 天必须拆分（warning）；must 任务 ≤12 个，超出说明 change 该拆。
反模式：`TASK-001 实现订单取消功能`（需求副本）、`TASK-00X 修复所有问题`（不可验证）、
只写代码不写测试/配置/复核。

### 6.3 双向追溯

- 正向：每个 `TASK-*` 可追溯到一个或多个 `REQ-*`/`AC-*`。
- 反向：每个 must 需求的每个 `AC-*` 至少被一个未废弃任务覆盖。
- 一致：任务声明的覆盖关系与覆盖矩阵必须一致。
- 每项交付（提交、PR、迁移、配置）都能在 `tasks/index.md` 找到对应任务。

## 7. 验证与归档

### 7.1 verify（structure）：只读校验，结果写入 verify.md

**规格**：① `spec/index.md` 存在非空；② `spec/requirements/` 至少一个非空文件；③ 每个
must 需求含"验收条件"；④ `REQ-*`/`AC-*`/`E-*` 唯一，需求依赖无自引用与循环。
**任务**：⑤ `tasks/index.md` 存在（`Has tasks: false` 跳过 ⑥–⑨）；⑥ 至少一个 `TASK-*.md`
且编号唯一；⑦ 每个任务声明 `Requirement refs` 且指向存在的 `REQ-*`；⑧ 覆盖矩阵无 must
验收空洞；⑨ 任务依赖无自引用与环，`blocked`/`dropped` 均已填理由；⑩ 覆盖 must 的任务优先级
为 `must`，每项任务有完成定义；⑪ 矩阵与任务文件声明一致；⑫ 所有未废弃任务必须为
`done` 或 `dropped`，否则验证失败且不得归档。

全通过 → `verified`；任一失败 → `needs_fix`，`verify.md` 列出失败项并定位到文件与标识符。
粒度/规模超限记 warning，不阻断。

### 7.2 verify（evidence）

~~~yaml
schemaVersion: sdd.verification/v1
result: pass
tasks:                       # 状态快照，须与 TASK-*.md 当前状态一致
  - id: TASK-001
    status: done
    evidenceRefs: [E-003]
checks:
  - id: VERIFY-001
    name: 单元测试
    status: passed
    command: cargo test --quiet
    output: all tests passed
    evidenceRefs: [E-003]
    requirementRefs: [REQ-001]
    acceptanceRefs: [AC-001]
    taskRefs: [TASK-001]
~~~

- `result: pass` 要求所有 checks 为 `passed`。
- 所有未废弃任务须为 `done`/`dropped`；存在 `todo`/`doing`/`blocked` 的任务即失败，
  `verify.md` 列出未完成任务与未覆盖验收。
- `tasks` 段与 `tasks/TASK-*.md` 不一致即失败（先改 Markdown 再重跑）。
- `acceptanceRefs` 与 `taskRefs` 至少各一项。
- 失败写入 `verify.md` 并回 `needs_fix`；修复后重跑，不手工编辑 `verify.md`。

### 7.3 archive

仅最近一次校验为 `verified` 且所有未废弃任务均为 `done`/`dropped` 的 change 可归档；
CLI 将目录从 `sdd/changes/<change-id>/` 移到
`sdd/archives/<change-id>/`，此后只读。
Windows 若遇到临时文件占用，CLI 会短暂重试；重试仍失败时关闭编辑器、终端、索引器或杀毒软件对 change 的占用后再试。

## 8. Agent 执行清单

- [ ] 读 AGENTS.md、SDD.md 与相关 notes/projects，建立 `E-*` 证据链
- [ ] 区分事实、推测与开放问题
- [ ] `spec/index.md` + 每个 `REQ-*.md`：must 需求均有 Given/When/Then 验收
- [ ] 记录探索、决策、方案比较、影响矩阵、接口与外部依赖
- [ ] 非代码动作、高危复核、高风险护栏（灰度/回滚/阈值/告警/审批）已写
- [ ] 测试策略已写，`tasks/index.md` 覆盖矩阵覆盖全部 must 需求的每个 AC
- [ ] 每个 `TASK-*.md` 有需求来源、完成定义、依赖、复核与回退
- [ ] 测试/配置/数据/文档/发布类工作均已成任务，无越界、无孤儿任务
- [ ] 实现与测试完成，任务状态已更新，verify 证据齐备
- [ ] structure 与 evidence 均 `verified` 后才 archive，并更新 change 记录与 log.md

## 9. 命令

~~~text
wiki sdd -h ｜ wiki sdd new -h ｜ wiki sdd list -h ｜ wiki sdd verify -h ｜ wiki sdd archive -h
~~~

历史任务文档可保留审计，但不代表 CLI 提供任务推进、spec、design、plan 或 build 功能。任务的
创建、推进与调整始终由 Agent 编辑 `tasks/` 下 Markdown 完成，CLI 只做只读校验。
