# 本地验证结果与边界

验证日期：2026-09-10。验证对象仅为 `architecture-review-0905/design`。

## 批次一次提交复验（2026-09-10）

- BatchRequest 统一为 batch_id、run_spec、episodes；配置随任务一次提交，没有前置注册接口。EpisodeRequest 移除 run_id 和任何配置覆盖，ExecutionPlan.run_id 只从批次 RunSpec 派生。
- 九个数据集新增由生成器维护的 batch_request.json。独立 run_spec.json、episode_request.json 仅作为阅读视图；对照清理前的九份 ExecutionPlan，所有字段值与 plan_digest 均保持不变。
- Rust validate_batch_submission 核验批次非空、成员 batch_id/sample_index、重复身份及同 run_id 的完整配置一致性；stored_run 只用于比较。每条任务仍由同一 PlanResolver 使用批次 run_spec 生成计划。
- 全部 33 项验证测试通过，其中包含 Rust 控制测试入口。新增验证覆盖九个数据集的多成员共享配置、后续批次复用、配置冲突、重复身份、空批次和成员覆盖字段拒绝。
- 三个生成脚本运行成功；Rust fmt 与 clippy -D warnings 通过；74 个本地链接和标题锚点、26 个 Mermaid 代码块边界检查通过，未逐图渲染。git diff --check 通过。
- 本地校验函数不实现数据库。配置比较与首次保存、任务持久化和并发幂等必须在生产 Server 事务中完成；生产 Bridge/RPC、部署、真实模型与后端均未在本次实现或验收。修改仅在 design，未新增临时脚本。

## run.yaml 配置清理复验（2026-09-10）

- 删除 ContainerBackendConfig.runtime_profile，以及 OpenHandsAgentConfig.history_policy、sdk_iteration_limit；同步九份运行示例、生成契约与计划。Process 的 runtime_profile 继续专指本机运行环境。
- 公共提交默认值只在核心契约或组件模型声明。expand_run 使用 SchemaRegistry.apply_defaults 补值后校验；显式参数保持不变，非法 null/类型、旧字段和已知同义入口被拒绝。Server/Worker 使用的普通 validator 仍要求完整字段，不自动补默认值。
- 九份公开示例省略默认资源、模型生成、重试、保留期和默认组件参数。逐份比较解析后的 RunSpec/ExecutionPlan：除明确删除的组件字段及其 plan_digest 外，实际配置值与清理前相同。
- 自定义角色配置可从 models.py 的默认值生成 schema，并通过同一提交函数补齐；文件读取后的配置与程序直接提供的等价完整配置得到相同 RunSpec，输入对象不被修改。
- 三个生成脚本成功。全部 32 项验证测试通过，含 21 项 Rust 控制测试的执行入口；新增检查覆盖默认值、非法参数和完整传输对象的严格校验。
- 用户指南第 2 节集中解释文件/SDK/Bridge、字段分组、默认值、组件扩展和后端切换。72 个本地链接及标题锚点、26 个 Mermaid 代码块边界检查通过；未逐图渲染。git diff --check 通过。
- 修改仅限 design 参考实现、契约、示例和文档；未修改生产源码。真实 Bridge/CLI、OpenHands 迭代接入、模型、后端和 Hub 服务仍未验收；本次未重复 wheel 构建。构建缓存位于仓库外，未新增临时脚本。

## 文档分组与合并复验（2026-09-10）

- docs 从 12 篇合并为 9 篇：主方案留在根目录，用户指南和数据集模板归入 guides，规则、迁移、参考实现和验证归入 development，字段字典与模块清单归入 generated。
- 能力清单合入重构计划，原有 39 项功能处置均保留且只在总表出现一次；历史问题中的独有风险合入迁移检查。配置审计的规则与实现证据分别归入字段规范和参考说明，原问答文档退出维护。
- 用户指南缩为操作说明；数据转换、稳定样本身份和各数据集原始字段映射归入数据集模板。参考说明的配置章节统一记录消费者及实现边界。
- 修正字段规范中的评分超时路径为 private_data.data.evaluation_plan.timeout_ms，与生成契约和 Python evaluate_harness 的读取一致；harness 字段只选择执行组件。
- 三个生成脚本运行成功；输出路径已更新。公共契约、Python SDK、Rust 控制参考和九个数据集示例的内容均未变化。
- 全部 29 项验证测试通过，其中一项运行 Rust 控制测试。68 个本地文件链接及标题锚点全部有效；主方案十章编号连续；25 个 Mermaid 代码块的边界与图类型入口检查通过，未进行逐图渲染验收。
- git diff --check 通过。依赖和 Rust 构建缓存使用仓库外已有目录，没有新增临时脚本、归档目录或本地构建产物。本次未执行 wheel 构建、真实后端、官方 benchmark 或远端源码验证。

下列条目按此前验证时的布局和范围保留，用于说明历史证据；当前路径与最新统计以上述记录和仓库 README 为准。

## 主文档重组复验（2026-09-10）

- 主设计按目标、用户输入输出、部署、完整流程、数据对象、扩展接口、后端、评分轨迹、Hub、异常恢复组织为 10 章。GSM8K 示例使用已有参考样本与配置，模型回答及结果片段明确为示意。
- 源码语言依据移入能力清单；迁移、旧字段与验收细节移入源码重构计划；进程接线图与实现缺口移入参考说明；九个数据集继承图移入数据集模板。
- 主文档标题编号连续且层级一致；检查所有 Markdown 的代码块边界，以及 60 个本地文件链接和标题锚点，全部通过。Mermaid 检查覆盖代码块边界与图类型入口，不代表逐图渲染验收。
- `python -B -X utf8 scripts/validate_design.py` 通过全部 29 项测试，包含 Rust 控制测试入口。引用的 GSM8K 字段和 SWE 类名已与当前参考代码核对。
- 本次仅修改 Markdown，参考代码、契约和示例文件内容未改；未重新构建 wheel、执行真实后端或访问远端生产源码。

## 目录整理复验（2026-09-10）

- 文档集中到 `docs/`，维护脚本和 Python 依赖清单集中到 `scripts/`；Rust workspace 配置保留在根目录。
- 从仓库根目录运行三个 `scripts/build_*.py` 生成器成功；生成的协议和数据集示例内容未改变，字段字典与模块清单写入 `docs/`。
- `python -B -X utf8 scripts/validate_design.py` 通过全部 29 项测试，其中一项调用整套 Rust 控制测试。
- 检查 README 和所有设计文档的 49 个本地文件链接，目标全部存在。
- 本次复验使用仓库外的依赖缓存和 Rust 编译目录，未在设计目录新增 `.dependencies`、`target` 或 `__pycache__`。本次未重复 wheel 构建；下文 wheel、格式与 clippy 记录来自同日整理前的验证。

## 已执行

从 design 目录执行；wheel 检查使用带 setuptools/wheel 的本机 Python，在临时源码副本中构建。

```text
python -B -X utf8 scripts/build_contracts.py
python -B -X utf8 scripts/build_examples.py
python -B -X utf8 scripts/build_module_map.py
python -B -X utf8 scripts/validate_design.py
cargo fmt --all --manifest-path Cargo.toml -- --check
cargo clippy --offline --locked --manifest-path Cargo.toml --all-targets -- -D warnings
python -m pip wheel --no-cache-dir --no-index --no-deps --no-build-isolation <reference package>
```

结果：

- 生成 60 个公共类型和 11 个内置组件扩展 schema；另从九个数据集包的 `models.py` 生成 18 个包 schema。PackageMetadata 已删除；PackageManifest 不含 metadata 或作者 schema_version。
- 九个数据集都使用 `dataset.yaml + pyproject.toml + src/<package> + tests` 结构；公开运行配置位于 `reference/runs`，生成的 manifest、内部请求与九个 `execution_plan.json` 位于 `reference/generated`。
- 生成 138 个目标模块职责条目，其中 11 个是统一核心 proto 文件职责；补齐 AgentRuntime 与统一样本准备函数的文件职责。
- 28 个 Python 契约、包结构、模型生成、扩展与评分测试通过。
- 21 个 Rust 控制链测试通过。
- Rust 格式检查和 `clippy -D warnings` 通过。
- SDK、共享评分规则和九个数据集包共 11 个 Python wheel 离线构建成功；在系统临时目录的源码副本中构建，确认 dataset_adapter.py 进入 wheel、本地 tests 不进入 wheel。此项只验证打包，不代表安装后的生产运行通过。
- `scripts/validate_design.py` 的第 29 个测试运行整套 Rust 测试，最终输出 `Ran 29 tests ... OK`。

## 已验证的关键约束

- 无 tests/ 的作者包仍可加载并生成 manifest；Python 构建版本与包声明不一致时拒绝。
- 模型登记支持可选角色配置、动作、观测与状态；配置 schema 被实际用于参数校验，嵌套模型生成、校验和恢复通过回归检查。
- 字符串布尔值、未知模型角色、重复能力名、嵌套重复控制字段及不匹配的角色 manifest 均明确拒绝。
- Agent 交互时间用完后，finalize 使用评分预留时间，仍能完成一次评分；评分截止时间保持原值，不重新延长。

- 数据集 YAML、PackageManifest 和 ExecutionPlan 不含额外 UEnv `dependencies` 列表；Python 校验拒绝重新提交该字段，Rust 参考不再递归解析组件依赖。Python/Cargo 安装依赖保留。

- 九个数据集各自声明 Adapter、Environment、Scorer，共 27 个直接子类。
- 九个数据集业务字段只定义在各包 `models.py`；中央 `scripts/build_contracts.py` 和通用 `scripts/build_examples.py` 不包含数据集模型或名称。生成 schema 与模型类型标注逐字段一致。
- 九份公开 `run.yaml` 都不含 `schema_ref`、episode_id、attempt_id、lease 或 digest；组件目录在提交边界把公开 config 自动封装成内部 RunSpec。
- 数据集作者目录不保存 manifest、TaskSpec、EpisodeRequest 或 ExecutionPlan；这些对象只出现在独立生成目录。
- 九个包声明与 PackageManifest 均拒绝 metadata、schema_version、dependencies；公开 run.yaml 的 schema_version 由提交边界生成。
- 文档规定字段查询按作者任务过滤，已有测试检查这些文档约束；uenv describe CLI 尚未实现，不能把文档检查当成查询功能验收。
- 所有 Scorer 直接继承 `Scorer`，所有 Environment 直接继承 `Environment`。
- 数据集 manifest 只有 Adapter、Environment、Scorer 三个入口；Scorer 只处理单条 episode，系统查询层只从已保存结果生成通用运行统计。
- Python 参考中不再存在 `EpisodeRuntime`、Python 计划解析器、Python 权威工具路由或 Python Backend 基类。
- 九个示例计划均可由同一个 Rust `PlanResolver` 重建为完全相同的 JSON。
- Worker 的参考执行入口只有 `EpisodeSupervisor.execute(dispatch, ports...)`；逐 episode 配置只从 `dispatch.plan` 读取，lease、remaining_timeout_ms、consumed_usage 只提供派发授权与累计运行状态。
- `ScoreInput` 不复制 remaining_timeout_ms；Python Scorer 只从 `ScoringContext` 读取 Rust Supervisor 的当前剩余预算。
- `purpose=training` 必须且只能搭配 training 配置，`purpose=evaluation` 禁止携带 training。
- `RunSpec.scorer` 解析为 `ExecutionPlan.scorer`，正式结果只写 `EpisodeResult.score`。
- Python Scorer 不能填写 `status`、`scorer`、`error`；Rust 负责补全和错误转换。
- `ScoreInput.trajectory_ref` 与最终 `trajectory_ref` 都引用同一 `TrajectoryManifest` 根结构。
- harness 只在 `private_data.data.evaluation_plan.harness` 选择，并由 Rust 解析器按组件规则锁定。
- 预算预留、模型调用计数、工具调用计数、环境步数和输出 token 计数由 Rust 执行；DispatchRequest.consumed_usage 会延续跨 attempt 的累计计数。
- Agent、Backend 和本次启用的工具列表不进入数据集 manifest；数据集包只能用 `provided_tools` 声明可提供项。本次选择来自 RunSpec，解析后只有 ExecutionPlan 生效。
- AgentHost、EnvironmentHost、ScorerHost 和 ToolHost 是独立权限端口；失败路径仍关闭三类 Python host、ToolHost 与 Backend。
- 取消发生在资源创建后时仍进入同一清理路径。
- Environment.step 只能通过 AgentRuntime 的单一入口调用，Rust 在动作副作用前检查并预占预算，再校验和记录 Transition。
- Backend 只创建任务 session；ToolHost 从唯一 `ExecutionPlan.tools` 绑定实际路由。工具执行失败时仍写入与 ToolCall 同 id 的 ToolResult(error)。
- ToolHost 的实际可路由表和 AgentHost 的实际模型可见表都必须精确等于 `ExecutionPlan.tools`；隐藏原生工具和缺失适配器都会在 reset 前失败并清理。
- 评分器异常后，冻结 Outcome 与 `status=error` 的同一个 ScoreResult 仍保存在 EpisodeResult。
- 模型、工具、环境错误保留明确的 phase、operation_id 和 retryable；评分超时不会被改写为笼统的评分失败。
- 轨迹序号由 Rust 连续分配，私有材料和覆盖它的 plan_digest 不进入公开轨迹。
- 轨迹事件按 UTF-8 JSONL 序列化后的字节大小分片，单片上限为系统内部固定的 4 MiB；只在完整事件之间切分，并标记 `application/x-ndjson`。
- `TrajectoryManifest.trajectory_status` 用 scoring_checkpoint、final_complete、final_partial 分开表达评分快照和最终记录完整性，不再复用 complete 布尔值。
- 任一事件或 checkpoint 写入失败都会使最终轨迹标记为 final_partial；已形成的 Outcome/ScoreResult 不因 score 事件写入失败而丢失。
- Outcome.termination_reason 是唯一交互结束原因；EpisodeResult/TerminalEvent 不再复制。WorkerRegistration.components 是包括 Backend 在内的唯一安装清单。
- WorkerRegistration.capacity 表示总并发槽位，Heartbeat.available_slots 表示当前剩余槽位，resource_capacity 表示总资源，BackendSpec.resources 表示单 episode 申请。
- Agent 池字段、`env_type` 和操作系统底层配置字段均被公共 schema 拒绝。

## 没有验证

- 远端 Rust Server/Worker 已迁移到该设计。
- 真实 gRPC、lease、事务/outbox、持久 attempt 账本与崩溃恢复。
- 真实 Process、Docker、Podman 的隔离、取消和资源回收。
- OpenHands 原生工具、MCP 转换和真实模型服务。
- 官方 SWE/DSCodeBench harness 与文本评分器的官方差分。
- Hub 发布、数据分片、鉴权、缓存和生产部署。
- 大规模并发、性能或安全隔离。

因此这次结果证明的是“设计契约和本地控制参考一致”，不能表述为“生产迁移完成”或“benchmark 通过”。
