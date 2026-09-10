# 字段与配置单一来源审计

审计目标有三个：一个含义只使用一个字段名；一次 attempt 的执行配置只有一个生效来源；每个字段都能回答谁填写、谁读取、读后产生什么具体作用。

## 0. 字段用途的审计要求与当前缺口

按 [字段规范第 0.1 节](field_conventions.md#01-字段必须有明确用途) 逐字段审查，包含嵌套子字段。以下记录区分已核实的消费者和当前缺口，不代表已经完成所有字段的用途审计。

| 字段 | 谁填写 | 谁读取 | 具体作用与当前状态 |
|---|---|---|---|
| 包 `id` 与 `version` | 数据集作者 | `package_loader.package_schema_id`；Rust `ComponentCatalog` 的组件查找 | 生成带包版本的 schema 地址；按组件身份与版本找到所选实现。本地参考已有消费路径 |
| 包 `metadata` 及展示子字段、作者 `schema_version` | 不再填写 | 加载器及 PackageManifest 校验拒绝旧字段 | 已同步删除九份声明、生成契约和 manifest 中的字段；内部传输结构的协议标记不受影响 |

本次交叉核对另修复以下冲突：

- `required_capabilities` 统一为作者声明已登记需求、系统汇总、调度匹配；不再称为完全自动推断。
- 重构计划的两张旧图已改为受控 AgentRuntime 调用和 cleanup → seal → persist → report；清理不等待 ACK，ResultReport 才携带 lease。
- 按 run_id/sample_id 查询返回列表，避免同一样本多次采样时任取一条结果。
- 公开 run.yaml 不填写 schema_version，由提交边界生成；旧字段和不匹配的角色 manifest 明确拒绝。
- `tests/` 是可选开发资料；九个参考包保留测试夹具，但加载和发布不依赖这些文件。
- 运行次数只在 `limits.max_generations` 设置，PlainAgent.config 不再被描述为另一个轮数入口。
- Rust finalize 改为使用评分预留时间，防止交互预算用完却无法收集产物评分；ScoringContext 沿用原单调截止时间，避免再次相加延长预算。
- `ScoreInput` 只传评分数据；剩余时间和取消只来自 ScoringContext。Hub 已标准化样本不重复调用 Adapter。
- `models` 明确登记输入、私有材料及可选配置/交互模型；加载器发布全部声明的模型，嵌套对象由 Python 类型生成并恢复，不需要手工绑定 schema。
- 发布声明拒绝字符串布尔值、未知模型角色和重复能力名；检查 Python wheel 版本与包声明版本一致。
- SDK 的 AgentContext.task 使用 TaskSpec，Observation.data 使用 UEnvModel；九个 Adapter 的 normalize 签名与基类一致。

额外的 UEnv 组件 `dependencies` 列表已从九份数据集声明、PackageManifest、ExecutionPlan 和参考组件目录删除，递归解析及相应结构检查也已删除。重新传入该字段会被拒绝；组件仍在既有角色、工具适配器和 harness 字段中明确选择并核验版本。Python/Cargo 安装依赖由各自包管理文件管理。

## 1. 数据对象

| 含义 | 唯一字段 | 出现位置 | 说明 |
|---|---|---|---|
| 公开任务输入 | `input` | TaskSpec | Agent 和 Environment 可见 |
| 私有评分材料 | `private_data` | EpisodeRequest → ExecutionPlan → ScoreInput | 同名传递；不进入 TaskSpec/Observation/轨迹 |
| 最终候选产物 | `outcome` | ScoreInput、EpisodeResult | 不再有 AgentOutcome/FrozenOutcome |
| 动作后观测 | `observation` | Transition、EnvironmentTransition.transition | 前一观测明确叫 `observation_before` |
| 唯一 episode 奖励 | `reward` | ScoreResult | 评测与训练复用 |
| 正式评分结果 | `score` | EpisodeResult | 不另建 result/reward_result |
| 轨迹引用 | `trajectory_ref` | ScoreInput、EpisodeResult | 两处均引用 TrajectoryManifest |
| 运行镜像 | `runtime.image` | RunSpec/TaskSpec 候选，ExecutionPlan 最终值 | Worker 只读最终 plan |
| harness 选择 | `private_data.data.evaluation_plan.harness` | 私有评分材料 | 原字段原地锁定，不复制到 plan 顶层 |
| 交互结束原因 | `termination_reason` | Outcome | 只含 in_progress/final_answer/environment_terminal/budget_exhausted；系统取消或失败由 execution_status/ErrorRecord 表达，EpisodeResult/TerminalEvent 不复制 |
| 环境评分状态 | `state` | Outcome | 由 Environment 填写；生命周期事件改用 StateEvent.phase |
| 命令环境变量 | `environment_variables` | ExecRequest | 不与 RunSpec/ExecutionPlan.environment 组件选择重名 |

TaskSpec、EpisodeRequest、ExecutionPlan 的职责不同，因此同一对象从入口传到执行层时允许保持同名字段。它们不是同时生效的三份配置：Server 接受请求后生成不可变 ExecutionPlan；Worker 只执行该 plan。

## 2. RunSpec 到 ExecutionPlan

| 配置 | 用户选择位置 | Server 解析结果 | Worker 消费位置 |
|---|---|---|---|
| Environment | `RunSpec.environment` | `ExecutionPlan.environment`，补全版本摘要 | EnvironmentHost.prepare |
| Agent | `RunSpec.agent` | `ExecutionPlan.agent`，补全版本摘要 | AgentHost.prepare |
| Backend | `RunSpec.backend` | `ExecutionPlan.backend` | Backend.open |
| 模型 | `RunSpec.model` | `ExecutionPlan.model` | AgentRuntime.generate |
| 工具 | `RunSpec.tools` | `ExecutionPlan.tools`，补全适配器、scope、能力 | AgentRuntime.call_tool → ToolHost |
| Scorer | `RunSpec.scorer` | `ExecutionPlan.scorer` | Rust run_score 在每个进入评分的 attempt 中至多调用一次 ScorerHost |
| 预算 | `RunSpec.limits` | `ExecutionPlan.limits` 与绝对 `deadline_at_ms` | BudgetEnforcer |
| 训练版本 | `RunSpec.training` | `ExecutionPlan.training` | AgentRuntime 转发给模型调用 |
| 轨迹保留期 | `RunSpec.trajectory_retention_days` | 不进入 ExecutionPlan | Server/ArtifactStore 存储策略；Worker 始终记录完整标准事件 |

解析并不是复制两套可变配置。RunSpec 是作者请求，ExecutionPlan 是 Server 对版本、能力、镜像和截止时间解析后的唯一执行快照。Worker API 不接收单独的 model、backend、scorer、limits 或 seed 覆盖参数。

`purpose` 与 `training` 只有一种合法组合关系：`purpose=training` 时必须提供 `training`，`purpose=evaluation` 时禁止提供 `training`。`purpose` 只选择结果消费方，`training` 只约束训练所需的模型版本和轨迹；二者都不能改变评分器或生成第二套 reward。

Rust `EpisodeSupervisor.execute` 接收完整 DispatchRequest；所有逐 episode 配置只从 `dispatch.plan` 读取，`lease`、`remaining_timeout_ms`、`consumed_usage` 只提供派发授权和累计运行状态。其余参数是已经启动的端口对象：`AgentHost`、`EnvironmentHost`、`ScorerHost`、`Backend`、`ToolHost`、`ModelProvider`、`ArtifactStore`。AgentHost 与 EnvironmentHost 是同一 ComponentHost 协议的两种权限视图，分别连接两个角色实例；ToolHost 是 Worker 内 ToolGateway 的类型安全端口。端口只决定通过哪个基础设施连接执行，不携带逐 episode 选择。

总截止时间只由 `ExecutionPlan.deadline_at_ms` 表达，PlanResolver 在首次接纳时生成；`Lease.expires_at_ms` 只管租约。Server 派发时从 deadline 得到只减的 `DispatchRequest.remaining_timeout_ms`；Worker RPC 核验后，Supervisor 只用这个值建立“本机单调时钟当前值 + 剩余毫秒”的运行截止时间，不再用本机墙上时钟重算绝对 deadline。`consumed_usage` 是服务端累计账本状态。这两个派发字段都不是新的用户配置来源。Backend.open、Host.prepare、Agent.run、freeze 及每次模型/工具/环境/评分调用均接收同一预算派生的当前剩余毫秒；close 使用平台固定清理超时。所有模型请求由 AgentRuntime 调用 ModelProvider，Agent host 不持有可绕过该入口的模型端点。

评分阶段也不复制超时字段：`ScoreInput` 只装评分数据，`ScoringContext` 从 Supervisor 的同一本机单调截止时间读取剩余预算。若 Scorer 调用 harness，`HarnessRequest.remaining_timeout_ms` 只是 `ScoringContext` 当前剩余量与 evaluation_plan.timeout_ms 的较小值，不是另一处配置。

数据集 PackageManifest 只在发布时登记到可信组件目录。`provided_tools` 只列包可提供的工具，不是默认工具配置；实际选择只来自 `RunSpec.tools`。`RunSpec.environment` 和 `RunSpec.scorer` 分别是两个角色的唯一选择；两者通常引用同一个数据集包，由 manifest 的 environment/scorer 入口和各自配置 schema 解析到对应类。PlanResolver 不再接收独立 package 参数，也不接收特定 Worker capabilities；它从已选组件元数据汇总 `required_capabilities`，Placement 再按该结果选择 Worker。WorkerRegistration.components 是所有已安装可执行组件（包括 Backend）的唯一清单，不再并列维护 backends。capacity 是总并发槽位，Heartbeat.available_slots 是当前剩余槽位；resource_capacity 是总 CPU/内存/存储，单 episode 申请只使用 BackendSpec.resources。

## 3. 镜像与后端

容器镜像候选按以下顺序解析一次：

1. `RunSpec.runtime.image`：本次 run 的显式覆盖。
2. `TaskSpec.runtime.image`：样本固有镜像。
3. `PackageManifest.runtime.image`：包默认镜像。

Server 把选择结果写到 `ExecutionPlan.runtime.image`，同时用 `image_source` 记录来源。Worker 不再回看三个候选。Docker/Podman 缺少镜像时拒绝；Process 只拒绝用户在 RunSpec 显式要求的 image，TaskSpec/PackageManifest 的镜像候选可作为另一条容器运行方案存在，Process 本次不消费它们并验证本机 `runtime_profile` 与 Environment 声明兼容。系统不会静默切换 backend 或猜镜像。

Backend 与 Environment 独立选择。Environment 使用统一会话能力，不按 Process/Docker/Podman 分别写适配器。某组合缺能力时由 PlanResolver 在执行前拒绝。

## 4. 工具

`ExecutionPlan.tools` 是当前 attempt 唯一允许的工具表。用户只在 RunSpec.tools 选择 name/implementation/config；AgentManifest 只声明有序 supported_interfaces 和 required_tool_names，ToolSpec 声明 config_schema 与 interfaces。PlanResolver 校验配置后选择第一个共同接口，把 interface/adapter 锁定到计划；Agent 不维护具体工具白名单。Backend 只创建任务 session，不接收工具表。Environment host 先绑定 session；随后 `ToolHost.prepare(plan.tools, session, remaining_ms)` 返回真正可路由的工具表，`AgentHost.prepare(plan.agent, plan.tools, remaining_ms)` 返回经过 MCP 或原生适配后模型真正可见的工具表。Rust Supervisor 分别要求这两个结果与同一份 `ExecutionPlan.tools` 完全一致。两个返回值只是启动核验结果，不是第二份 `actual_tools` 配置。名称重复、工具缺失、接口不兼容、实现或适配器版本不同都会拒绝。

Agent 只能通过 Rust `AgentRuntime.call_tool` 调用已选工具：AgentRuntime 负责取消、预算和轨迹，ToolHost 只按 `execution_scope` 将调用送到 Agent host、sandbox host 或已授权外部连接。`Environment.finalize` 后，Supervisor 用评分剩余预算先调用 `ToolHost.freeze(remaining_ms)` 拒绝迟到工具请求，再调用 `Backend.freeze(remaining_ms)` 固定评分视图，之后才创建 scoring checkpoint 和调用 Scorer。

用户 Python 函数工具由 UEnv host 包装成统一 ToolSpec/ToolExecutor，再按解析后 interface 使用 MCP 或原生适配器展示。适配器只做协议转换，不决定权限、工具集合或预算。OpenHands 自带工具也必须发布 ToolSpec 并映射到同一计划工具名和轨迹事件。新增使用已有接口的工具不要求修改 AgentManifest。

## 5. 评分

每条 episode 只选择一个 `RunSpec.scorer`，解析后只执行 `ExecutionPlan.scorer`。每个得到最终 Outcome 的 attempt 至多调用一次；未进入评分没有 score，已经调用后 status=ok/error 的同一个 ScoreResult 都保留。Server 只接纳一个 attempt 的结果作为 episode 权威评分。

评测和训练不维护两套 scorer。二者都得到相同 reward；Bridge 在训练时读取该 reward 并与生成 token 轨迹组合。首版不提供独立离线评分接口；用户单测评分规则时直接调用 Scorer，正式结果必须提交 episode 并经过同一 Worker 评分链。

## 6. 已修复的双来源

- 删除 Python `EpisodeRuntime`，避免 Python 和 Rust 都拥有 attempt 状态机。
- 删除 Python `execution_plan.py` 和 `tool_resolution.py`，权威解析与路由改为 Rust。
- 删除 Python `SandboxBackend`，后端只由 Rust trait 表达。
- 删除 Python `evaluate_score`，评分结果补全只在 Rust `run_score`。
- 评分前与最终轨迹均使用 TrajectoryManifest，并以 trajectory_status 明确区分 scoring_checkpoint、final_complete、final_partial；不再复用 complete 布尔值。
- harness 在原有私有字段上解析，不增加第二个 harness 字段。
- 删除没有消费者的 `EvaluationPlan.result_schema`；harness 统一返回公共 HarnessResult。
- EpisodeResult 与 TerminalEvent 删除重复的 termination_reason；交互结束原因只保存在 Outcome，执行失败原因只保存在 ErrorRecord。
- WorkerRegistration 删除与 components 重叠的 backends；PackageManifest.provided_tools 与 RunSpec.tools 分别表示可提供项和本次选择。
- `env_type`、Agent 池字段和面向操作系统专家的公开权限字段继续由 schema 拒绝。

## 7. 仍需生产实现验证

本地 Rust 参考使用同步 trait 和内存 mock。它没有实现真实进程取消、RPC 重试、容器清理、持久预算账本、持久轨迹 spool、durable outbox、启动恢复或 lease/replay fencing。`EpisodeSupervisor.execute` 假定 DispatchRequest 已经由目标 Worker RPC 授权；本地只检查请求形状、摘要和预算。

Python validator 会递归执行完整 JSON Schema。Rust `ContractSchema` 参考只读取类型与字段集合，并执行控制链需要的浅层形状和语义检查；目标生产 Server/Worker 边界必须使用从唯一核心 proto 生成的完整类型或 validator，包扩展数据使用从 models.py 生成的 schema。任何真实迁移都要再次追踪“RunSpec → ExecutionPlan → 实际消费者 → 轨迹/EpisodeResult”，不能只检查字段存在。
