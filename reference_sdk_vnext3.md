# vNext.3 可执行参考说明

本目录的参考代码用于把设计中的关键边界变成可运行测试。它不是远端生产迁移结果。

## 1. 语言边界

| 代码 | 负责什么 | 不负责什么 |
|---|---|---|
| Rust `reference-control` | 计划解析、组件锁定、镜像解析、预算、后端生命周期、模型和工具入口、轨迹、每个已进入评分的 attempt 至多一次评分、清理、EpisodeResult | 数据集评分规则、Agent 推理循环、Environment 业务 |
| Python `reference/sdk/src/uenv/sdk/` | Adapter、Environment、AgentRunner、Scorer、ToolExecutor、UEnvModel 与包 schema 生成 | 调度、后端、权威预算、轨迹封存、结果提交 |
| Python `reference/datasets/` | 九个数据集各自的 Adapter、Environment、Scorer | 选择 Agent、Backend、模型或本次工具表 |
| Python `build_*.py` 与 `reference/fixture_plan.py` | 当前本地参考的过渡生成器，生成文档、schema 和稳定示例 | 生产协议定义或运行时决策 |

系统控制逻辑不再通过 Python `EpisodeRuntime` 模拟。`reference/fixture_plan.py` 只生成静态示例；每个示例随后由 Rust `PlanResolver` 重建并比较，防止生成器成为第二套运行时规则。

## 2. Rust 参考文件

| 文件 | 责任 |
|---|---|
| `contracts.rs` | 当前参考读取生成的 `uenv.schema.json` 的类型与字段集合，并执行控制链需要的浅层形状检查；目标生产实现改为使用统一 proto 生成的 Rust 类型和完整校验器 |
| `plan.rs` | `PlanResolver`、计划摘要、重试、工具与实际路由核验 |
| `ports.rs` | Backend、ToolHost、AgentHost、EnvironmentHost、ScorerHost、ModelProvider、ArtifactStore 的 Rust 边界 |
| `runtime.rs` | `BudgetEnforcer`、`AgentRuntime`、`TrajectoryWriter` |
| `scoring.rs` | 每个进入评分的 attempt 至多调用一次 Python Scorer，拒绝其填写系统字段，补全正式 `ScoreResult` |
| `supervisor.rs` | `EpisodeSupervisor.execute(dispatch, ports...)` 统一 attempt 状态机和清理；配置只读 `dispatch.plan` |

Rust 不重复声明 TaskSpec、ExecutionPlan、ScoreResult 等公开结构。当前本地参考暂以 `contracts/uenv.schema.json` 驱动 Python validator，Rust 只读取类型与字段集合并追加跨字段检查。目标生产协议以 `contracts/proto/uenv/v1/*.proto` 为唯一可编辑来源；Rust/Python 类型、RPC stub、核心 JSON schema 和字段字典全部生成。数据集新增业务字段只在包内 models.py 定义并生成包 schema。生产 Server/Worker 的每个外部边界必须使用生成类型或完整校验器，不能把本地 `validate_shape` 当成完整 JSON Schema 校验。

## 3. Python 参考文件

`reference/sdk/src/uenv/sdk/` 提供以下用户接口：

- `DatasetAdapter.normalize(row) -> PreparedSample`
- `Environment.reset/step/finalize`
- `AgentRunner.run(context: AgentContext) -> Outcome`
- `Scorer.score(ScoreInput, ScoringContext) -> ScoreResult`
- `ToolExecutor.execute(arguments, context)`

`AgentContext` 只暴露 `task`、初始 `observation`、只读 `tools`、`seed` 以及异步 `generate/call_tool/step`。AgentRunner 对这三个操作统一使用 `await`；具体 Context 由 Worker SDK 适配器提供，它只是能力入口，不拥有预算或第二份工具配置。

ComponentHost 先按所选角色的 package config schema 完整校验 `ComponentSpec.config`，再只把 `config.data` 这一个 dict 传给 `Environment(config)`、`AgentRunner(config)`、`Scorer(config)` 或 `ToolExecutor(config)`；基类统一保存为 `self.config`。空配置也显式传 `{}`。组件不能同时读取整个 TypedConfig、环境变量或包默认值来形成第二套运行配置。

包模型由 dataset.yaml.models 显式登记，不扫描方法体猜测类型。input 必填；private_data、environment_config、scorer_config、action、observation、state 按需登记。未声明的角色配置使用 EmptyConfig；已声明的配置 schema 用于校验同一份 config.data，构造函数仍接收 dict。嵌套 UEnvModel 自动生成并恢复；ArtifactRef、ContentPart、EvaluationPlan 复用公共 schema。参考生成器对递归模型和有歧义的联合类型明确报错。

参考发布加载器不要求 tests/，但 build_examples 仍读取九个包保留的合成测试数据。build_manifest 尚未构建或上传 wheel、发布工具函数、执行 Hub 准入；生成 manifest 中 artifacts/provided_tools 为空是本地夹具边界。SDK 的 __init__.py 暂集中提供小型参考接口；生产目录仍按 module_map 拆分并仅在 __init__.py 导出。

每个数据集都声明自己明确命名的三个直接子类。例如 GSM8K 使用 `Gsm8kAdapter`、`Gsm8kEnvironment`、`Gsm8kScorer`；SWE Verified 使用 `SweVerifiedAdapter`、`SweVerifiedEnvironment`、`SweVerifiedScorer`。共享逻辑放在普通函数中复用，不再增加 `QuestionAnswerEnvironment`、`TextScorer` 或 `HarnessScorer` 中间层。

Python `ScoreResult` 是同一个结果对象。Scorer 只填写四个业务字段：

```text
success, metrics, reward, evidence
```

下面三个字段由 Rust Worker 填写：

```text
status, scorer, error
```

这样没有 `ScoreFacts`、`ScorerOutput`、`FrozenOutcome` 等同义转发类型。

## 4. 完整调用顺序

```mermaid
sequenceDiagram
    participant S as Rust Server / PlanResolver
    participant W as Rust Worker / EpisodeSupervisor
    participant B as Rust Backend
    participant T as Rust ToolGateway / ToolHost
    participant A as Python AgentHost
    participant E as Python EnvironmentHost
    participant M as ModelProvider
    participant C as Python ScorerHost

    S->>S: EpisodeRequest + RunSpec + registered component catalog
    S->>S: resolve one ExecutionPlan; collect required capabilities
    S->>W: execute(DispatchRequest)
    W->>B: open(plan.backend, plan.runtime, plan.internet_access)
    W->>E: prepare(plan.environment, session, remaining_ms)
    W->>T: prepare(plan.tools, session, remaining_ms)
    T-->>W: actual routable tools; verify against plan.tools
    W->>A: prepare(plan.agent, plan.tools, remaining_ms)
    A-->>W: actual model-visible tools; verify against plan.tools
    W->>E: reset(plan.task)
    W->>A: run_agent(task, observation, remaining_ms)
    A->>W: generate(messages) / call_tool(call) / step(action)
    W->>M: generate using plan.model
    W->>T: call selected tool
    T->>T: route by execution_scope
    W->>E: gated step(action) when requested
    A->>W: Outcome
    W->>E: finalize(Outcome)
    W->>T: freeze(remaining_ms) and reject later tool calls
    W->>B: freeze(remaining_ms) and expose a read-only scoring view
    W->>W: write pre-score trajectory manifest
    W->>C: score(ScoreInput) once in this attempt
    C->>W: ScoreResult business fields
    W->>W: complete and record ScoreResult
    W->>C: close()
    W->>A: close()
    W->>T: close()
    W->>E: close()
    W->>B: close()
    W->>W: record cleanup/terminal and seal final trajectory
    W-->>S: EpisodeResult
```

`generate(messages)` 和 Environment 的 `step(action)` 都是 AgentRunner 在循环中可调用的能力；它们不是同一层的业务实现。Rust `AgentRuntime` 在副作用发生前拦截模型、工具和环境动作，统一计算预算并写轨迹。多轮循环仍属于 AgentRunner；Worker 负责限制 generation、工具、环境动作和总时间。生产进程使用同一 ComponentHost 协议的 Agent 与 Environment 两个角色实例；Rust 以 AgentHost、EnvironmentHost 两个权限端口防止调用串线，本地同步参考用两个 mock 端口代替真实 IPC。

## 5. 唯一配置来源

Server 解析前，运行用户只在严格分组的 `RunSpec` 选择 Environment、Agent、Backend、模型、工具、Scorer 和 limits；数据集作者只在 Python `models.py` 中用类型标注声明业务字段。默认用户接口不展示 EpisodeRequest、ExecutionPlan、episode_id、attempt_id、lease、digest 或 TypedConfig.schema_ref。数据集 `PackageManifest` 是发布时登记到组件目录的元数据：同一个包可以同时导出 Environment 和 Scorer 入口，但 manifest 不构成运行时的第二个选择器。`RunSpec.environment` 与 `RunSpec.scorer` 是两个角色的唯一选择；它们可以引用同一数据集包，也可以引用分别兼容的包。`PlanResolver` 只读取这些已选组件的执行元数据；包 metadata 及其展示子字段已删除，包 schema_version 也不再要求作者提供。

Server 将组件版本、digest、镜像与截止时间锁定到 `ExecutionPlan`，并汇总 `required_capabilities`；随后 Placement 才用这些要求选择 Worker，PlanResolver 不接收或挑选某个 Worker。Worker 以 DispatchRequest 启动 attempt，所有执行配置只读取其中的 plan；同级 lease、remaining_timeout_ms 和 consumed_usage 是派发授权与累计运行状态，不是逐项覆盖参数。

后端、ToolHost、模型客户端、组件 host、评分 host 和产物存储作为 Worker 进程依赖注入。它们决定“通过哪个已启动驱动执行”，不携带本次任务的第二份配置。实际 backend、model、tools 和 scorer 仍只来自 plan。ToolGateway 实现 ToolHost，并把 plan.tools 绑定到 Backend 创建的 session；Backend 本身不再接收或选择工具。

工具只存在一张权威表 `ExecutionPlan.tools`，但必须核验两个实际结果。AgentManifest 只声明有序 supported_interfaces 和不可缺少的 required_tool_names；ToolSpec 声明 entrypoint、config_schema 和 interfaces。PlanResolver 校验工具配置，选择第一个共同接口并锁定 interface/adapter，不维护 Agent×具体工具的配对表。`ToolHost.prepare` 返回当前 session 中真正可路由的工具；`AgentHost.prepare` 返回经过 MCP 或原生适配后模型真正能看到的工具。Supervisor 分别要求二者与同一张计划表完全一致。它们是运行时核验结果，不是可修改的配置，也不落成 `actual_tools` 第二字段。

镜像只在 `ExecutionPlan.runtime.image` 生效。容器解析优先级是 RunSpec 覆盖、TaskSpec 样本镜像、包默认镜像；解析后 Worker 不再查看候选来源。Process backend 只在 RunSpec 显式要求 image 时拒绝；TaskSpec/PackageManifest 镜像是可用的容器方案，本次 Process 执行不消费。容器 backend 缺镜像直接拒绝。

harness 只在 `private_data.data.evaluation_plan.harness` 选择一次。Rust PlanResolver 在原字段上补全 digest，并将它纳入与其他组件相同的版本和能力检查，不再复制为第二个 plan 字段。

## 6. 轨迹与评分

`ScoreInput.trajectory_ref` 和 `EpisodeResult.trajectory_ref` 都指向 `TrajectoryManifest`。评分前 manifest 使用 trajectory_status=scoring_checkpoint；最终 manifest 使用 final_complete 或 final_partial，并包含适用的 score、cleaning 状态、错误和 terminal 事件。两处使用相同根结构，读取端无需根据调用位置猜格式。公开 manifest 不复制覆盖 private_data 的 plan_digest。参考 `TrajectoryWriter` 在任一事件或 checkpoint 写入失败后把最终清单标为 `final_partial`；score 事件写失败不会丢掉已经形成的 Outcome 或 ScoreResult。交互结束原因只保存在 Outcome.termination_reason；EpisodeResult 与 TerminalEvent 不再复制它，失败原因使用 ErrorRecord.code。

执行时私有评分材料原名传入 `EpisodeRequest.private_data`、`ExecutionPlan.private_data` 和 `ScoreInput.private_data`；准备阶段位于 PreparedSample 或标准化 JSONL 的同名字段。它不会进入 TaskSpec、Observation、AgentRuntime 或轨迹事件。Python Scorer 若需要运行正式测试，只能通过 `ScoringContext.run_harness()`；该回调由 Rust Worker 绑定到当前 Backend 的冻结评分视图。`ToolHost.freeze()` 先撤销 Agent 工具写入，`Backend.freeze()` 再固定 session 内容；完成这两步后才创建评分 checkpoint 和调用 Scorer。所有 freeze/close 都必须幂等。

Agent 交互结束后，finalize、freeze 和 Scorer 共用 score_reserve_ms 留出的总剩余时间；评分上下文沿用 BudgetEnforcer 的原始单调截止时间，不重新计算一个更晚的截止时间。

`ScoreInput` 不再携带 `remaining_timeout_ms`。Scorer 只能从 `ScoringContext` 查询 Supervisor 当前剩余预算；harness 的实际超时取这份剩余预算与 evaluation_plan 中超时上限的较小值，因此评分阶段没有第二个可覆盖的时间来源。

评测与后训练走同一评分路径，产生同一个 `EpisodeResult.score.reward`。进入评分前失败没有 score；已经调用 Scorer 后，status=ok 或 status=error 的同一个 ScoreResult 都保留。区别只在下游如何消费结果；Worker 不为 training 维护第二套评分函数。

每条 `GenerationEvent.output_token_count` 都必填，并且是预算统计的唯一来源。`output_token_ids`、`output_logprobs` 和 `loss_mask` 是可选的详细训练轨迹；存在时必须彼此对齐并与 count 一致。训练配置要求 token trace 时，缺少这些详细字段的结果不能进入训练，但仍可以作为普通评测结果保存。

## 7. 验证范围

Rust 测试覆盖九个计划的统一重建、唯一字段字典、重试配置冻结、统一 Supervisor 流程、评分字段所有权、预算预留、连续轨迹、失败工具调用成对记录和失败清理。Python 测试覆盖 schema、九组数据集类、Environment 接口和评分规则。

`EpisodeSupervisor.execute()` 的本地参考假定 DispatchRequest 已经过 Worker RPC 授权；它检查形状、摘要和预算，但没有实现真实 lease token/epoch fencing、活动 attempt 账本或重复派发连接。目标 `WorkerRpc.validate_dispatch` 必须先完成这些检查，才可调用 Supervisor。

Supervisor 以收到的 `remaining_timeout_ms` 建立本机单调截止时间；运行中不再用墙上时钟重新解释 `deadline_at_ms`。Server 负责从唯一绝对 deadline 派生该只减值，Worker RPC 负责拒绝被放大的值。这样 deadline 是 Server 的审计与派发依据，remaining timeout 是 Worker 的唯一运行时计时输入，并非两套可覆盖配置。

Backend.open、各 Host.prepare、AgentHost.run_agent、ToolHost/Backend.freeze，以及模型、工具、环境和评分调用都接收这一个预算派生的当前剩余毫秒。close 由 Worker 进程控制器使用平台固定清理超时，因为清理不能在 episode 预算耗尽时跳过。同步 trait 只表示参考边界；生产 host 必须真正中断超时进程或容器操作。

尚未实现真实网络 RPC、完整 Rust JSON Schema validator、lease/replay、持久 attempt 账本、持久轨迹 spool、durable outbox、启动恢复、进程强制中断、Docker/Podman/Process 驱动、真实 OpenHands/MCP 接入、官方 benchmark 差分和分布式恢复。参考代码通过 trait 和内存 mock 明确了部分端口，但 mock 通过不能当作生产验收。

公开运行示例不填写 schema_version，expand_run 在提交边界生成内部标记。该夹具辅助函数当前一次接收一份包 manifest，Environment/Scorer 选择不匹配时明确拒绝；生产提交端须分别按两个已选角色查询目录，Rust PlanResolver 已分别解析角色，不受夹具限制。
