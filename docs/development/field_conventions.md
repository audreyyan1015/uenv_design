# 统一字段命名与归属

本规范约束目标协议、Python 参考代码、生成示例与用户扩展。一个概念只有一个规范字段名；配置解析后只有一个生效位置。不同边界可以传递同名字段或不可变快照，不允许同一接收者遇到两份可独立修改的权威值。

公开字段必须描述 AI 任务作者能直接判断的业务意图，不能要求用户理解操作系统实现。网络需求只使用 boolean `internet_access`；`sandbox_mode`、`network_policy`、`system_access_level`、`syscall_profile`、`command_mode` 等字段禁止进入公开协议。进程、容器、系统权限和隔离规则由 Backend 内部实现。自动校验会扫描公共 Schema，防止这些底层字段重新出现。

## 0. 字段发现和扩展边界

字段是否已有不能靠作者搜索源码或阅读生成 schema 判断，也不能把全部内部协议直接展示给普通用户。数据集作者默认只看到 `PreparedSample`、本包 `models.py` 类型和三个入口的方法参数；运行用户只看到严格分组的 RunSpec。`uenv describe PreparedSample`、`uenv describe RunSpec` 和 `uenv describe package@version:TypeName` 分别提供对应视图，Python SDK 类型提示与这些输出来自同一契约。

数据集字段采用 HUD 的方式，在 Python `models.py` 中通过类型标注定义并生成 schema。运行配置采用 Harbor 的方式，固定为严格的 RunSpec，分组包含 Environment、Agent、Backend、Model、Tools 和 Limits，Scorer 作为同级必选配置；提交前校验类型、必填项和组合兼容性。系统对象是封闭类型，未知字段直接拒绝。只有 Adapter、Environment 或 Scorer 理解的业务内容才加入 `models.py`；需要改变系统调度、权限、模型、预算、评分生命周期或结果处理的新能力必须修改核心 proto。

EpisodeRequest、ExecutionPlan、episode_id、attempt_id、lease、input_digest、plan_digest 和 `TypedConfig.schema_ref` 是系统自动生成或传递的内部字段，不出现在普通用户模板和默认查询中。包 metadata 及其展示子字段已删除，ExecutionPlan 也不接受 metadata。

`uenv package validate` 至少检查：系统对象无未知字段；包模型不遮蔽系统控制字段；已登记常见别名不重新进入新模型；入口类引用的模型都能生成并注册 schema。确定重复时报错并指向规范字段；疑似同义时只警告并要求作者判断。程序可以识别登记过的 `timeout`，但无法百分之百确定 `allowed_time` 是否表达同一个含义，因此不能声称校验能自动消除所有语义重复。

### 0.1 字段必须有明确用途

每个字段都必须回答：**谁填写、谁读取、读了以后产生什么具体作用。** 此要求适用于公共配置、数据集业务模型、内部请求、Hub 包声明、结果和轨迹，也适用于 JSON 对象内部的每个子字段。不能只解释外层对象，把内部字段的用途留空。

新增或保留字段时，按以下统一格式说明；说明附在该字段的唯一规范来源，字段文档由此生成，不再增加一套独立维护的字段定义：

| 说明项 | 必须回答的内容 |
|---|---|
| 字段路径与含义 | 完整路径、类型、单位、必填性、默认值；嵌套对象逐字段展开 |
| 谁填写 | 具体用户角色或生成它的组件；派生字段说明计算来源 |
| 谁读取 | 具体消费组件及操作；已实现时提供代码位置，不能只写“系统使用” |
| 具体作用 | 改变哪个执行行为、判断、查询结果或用户可见内容；缺失时怎样处理 |
| 唯一生效来源 | 哪处值具有决定权；跨边界传递和派生值不能形成第二套覆盖配置 |
| 实现与验证 | 区分已实现、已纳入本次交付但待实现、无对应功能；说明可观察的验证结果 |

“保存到数据库”“复制到请求”“通过 schema 校验”和“以后可能用到”本身不足以说明用途，必须继续追踪到实际消费者。展示、检索、审计和兼容性判断也可以是有效用途，但要指出具体页面、查询、诊断操作或格式读取行为，不能只贴上用途标签。

无消费者、无具体作用的字段从当前规范和默认模板中删除。待实现功能的字段只能随明确的功能交付计划列为待实现，不能作为已支持能力发布；功能验收时必须同时验证消费路径。已有重复字段合并到唯一规范字段，无法确认业务语义时由作者判断，不自动改名。

自动校验可以检查字段说明是否完整、类型是否合法和已登记的重复名称，但不能证明字段真的有用。代码审查必须追踪读取路径；有必要时用行为测试确认字段改变后产生了预期结果。每个字段不必单独增加一个测试，已有功能测试可以共同覆盖。

### 0.2 枚举命名规则

所有协议枚举使用小写 `snake_case`，不接受大小写、连字符或旧别名。字段后缀决定取值语义：`*_status` 使用生命周期状态词，`*_policy` 使用策略名，`*_level` 使用有序等级，`*_mode` 只用于执行算法或交互方式。真正的开关使用 boolean，不使用 `enabled`、`disabled`、`on`、`off` 字符串。可选字段省略；只有 schema 明确允许 null 时才能填写 JSON null（如 ScoreResult.success/reward）。只有“无”本身是业务选项时才使用 `none`。同一语义不能在不同类型中改用近义词。

| 字段种类 | 规范示例 | 禁止示例 |
|---|---|---|
| 生命周期状态 | `pending`、`running`、`completed`、`failed`、`cancelled` | 同时出现 `complete`/`completed` 或 `canceled`/`cancelled` |
| 操作结果 | `ok`、`error`、`timeout`、`cancelled` | 用 `failed` 表达一次调用返回的错误 |
| 策略 | `fixed_episode`、`per_generation` | `disabled`、`enabled` |

历史源码中的值可以在导入或迁移边界显式转换，但不能进入 vNext 公共协议。

配置的实际消费者和实现缺口见[参考实现第 5 节](reference_implementation.md#5-配置消费路径核对)，测试证据见[验证记录](verification.md)。本文件只维护命名、归属和校验规则。

## 1. 提交与执行

运行层级固定使用以下名称，公共协议不使用含义不明确的 `turn`：

| 层级 | 规范名称 | 含义 |
|---|---|---|
| 一次逻辑执行 | `episode` | 一个样本的一次完整 rollout 身份；基础设施重试时保持不变 |
| 一次执行尝试 | `attempt` | episode 的一次实际执行；正常只有 `attempt_id=1`，基础设施重试时才增加，每个 attempt 单独记录轨迹 |
| 一次模型调用 | `generation` | 一次真实模型生成；使用 `generation_id`、`max_generations` 和 `generation_count` |
| 一次环境动作 | `environment_step` | 一次 `Environment.step` 状态转移；使用 `environment_step_index` 和 `environment_step_count` |
| 一次工具执行 | `tool_call` | 一次获准的工具调用；使用 `tool_call_id` 和 `tool_call_count` |

Agent SDK 如果内部使用 turn、iteration 等名称，接入层必须映射到上述公共名称，不能把框架术语写入 UEnv 公共字段。

| 字段 | 提交时来源 | 执行时位置 | 消费规则 |
|---|---|---|---|
| run_id | EpisodeRequest；必须等于所引用 RunSpec.run_id | ExecutionPlan.run_id | 关联作业，不在 Worker 再查配置 |
| episode_id、seed | EpisodeRequest | ExecutionPlan 同名字段 | 采样执行身份和种子，重试保持 |
| task | EpisodeRequest.task | ExecutionPlan.task | 完整公开 TaskSpec 原值，不增加 task_view 等副本类型 |
| private_data | 准备入口从原始样本转换或读取标准化样本；Adapter 返回 UEnvModel，SDK 封装后与 task 配对进入 EpisodeRequest | ExecutionPlan.private_data → ScoreInput.private_data | 不生成中间材料文件；公开任务与公开轨迹不得携带 |
| environment、agent、scorer、backend、model、tools、limits、training、purpose | RunSpec 同名字段 | ExecutionPlan 同名字段 | 保持名称与语义，组件坐标在原位置升级为已锁定引用；purpose 只选择结果消费方 |
| runtime.image | RunSpec、TaskSpec、PackageManifest 的同名字段 | ExecutionPlan.runtime.image | 候选按 run > task > package 选择；Worker 仅消费最终值 |
| runtime.image_source | 无 | ExecutionPlan.runtime.image_source | run/task/package，仅供审计，不参与再次选择 |
| attempt_id | Server 生成，用户不填写 | ExecutionPlan.attempt_id | 正常为 1；只有基础设施重试才增加 |
| required_capabilities | 包及已锁定组件声明的运行功能需求并集 | ExecutionPlan.required_capabilities | 仅供 Worker 调度和兼容性检查；不授予文件、网络或宿主访问 |
| internet_access | Environment 所属 PackageManifest | ExecutionPlan.internet_access | boolean；表示是否需要公共互联网。Server 核验后锁定，Worker/Backend 唯一消费；RunSpec 与组件配置不能覆盖 |
| request_id | EpisodeRequest | Server 单条提交记录 | 单条 episode 请求的幂等键；不作为批次身份，不进执行计划 |
| batch_id | BatchRequest、BatchReceipt 与各 EpisodeRequest | Server 批次记录 | 同一批的唯一身份；不是单条幂等键，不进执行计划 |
| sample_index | EpisodeRequest | Server 批次成员记录 | 该 episode 在批内的位置；不进执行计划 |
| retry | RunSpec.retry | Server | Worker 不决定 episode 重试 |
| trajectory_retention_days | RunSpec | Server/ArtifactStore | 只控制权威轨迹保存期，不进入 ExecutionPlan，不改变事件内容 |
| lease | Server | DispatchRequest.lease | 授权与失效 fencing，独立于任务与执行配置 |
| remaining_timeout_ms | Server 根据 ExecutionPlan.deadline_at_ms 派生 | DispatchRequest | 只能收紧时间，不是第二份用户配置；Worker RPC 核验后，Supervisor 用它建立本机单调截止时间 |
| consumed_usage | Server 的 episode 用量账本 | DispatchRequest | 此前 attempt 已确认的累计用量；Worker 据此继续计数，首次为全 0 |

公开客户端只提交批次；单条执行使用只含一个成员的 BatchRequest。因此每个 EpisodeRequest 都有自己的 request_id，同时也总有 batch_id 和从 0 开始的 sample_index。三者分别表示成员幂等、批次身份和批内位置，不能互相代替。

`purpose=training` 必须同时提供 `training`，`purpose=evaluation` 必须省略 `training`。这是同一份运行配置的条件约束：purpose 决定结果交给训练器还是评测汇总器，training 只描述训练所需的版本/轨迹条件，不是第二种评分模式。

绝对 `deadline_at_ms` 只保存在受信 ExecutionPlan，供 Server 审计和派发时计算剩余量。Worker RPC 拒绝被放大的 `remaining_timeout_ms`；进入 Supervisor 后，运行截止时间只由“本机单调时钟当前值 + 收到的 remaining_timeout_ms”建立，不再用本机墙上时钟重算绝对 deadline。调用 Agent、Environment、Tool 或 Scorer 子进程时只传当前剩余毫秒，子进程不能延长或重算。

ExecutionPlan 不接受 episode、run、components、tool_bindings 等旧结构。WorkerRegistration.components 是唯一安装清单，Environment、Agent、Scorer、Tool、Backend 等可执行组件都登记在这里，不再另设 backends 清单；capabilities 表示 Worker 可提供的运行能力；capacity 是并发 episode 槽位总数，Heartbeat.available_slots 是它的当前剩余快照；resource_capacity 是 CPU/内存/存储总量，BackendSpec.resources 是单个 episode 的资源申请。四者含义和单位不同，调度同时满足槽位与资源余量，不能相互覆盖。

PackageManifest.provided_tools 只登记包能够提供的 ToolSpec；RunSpec.tools 是用户为本次 run 明确选择的 ToolBinding；ExecutionPlan.tools 是锁定版本和适配器后的唯一实际工具表。选择某个工具只允许调用该接口，不授予任意网络、路径或宿主访问。三者处于发布、选择、执行三个阶段，不能并行生效，也不能自动合并包或 SDK 的默认工具。

用户只填写工具 name/implementation/config。AgentManifest.supported_interfaces 是按优先级排列的公共接口，ToolSpec.interfaces 声明工具的可用接口与适配器；PlanResolver 选择第一个共同接口，并把 interface/adapter 作为解析结果保存在 ExecutionPlan.tools。AgentManifest 不逐个登记兼容工具，因此同接口的新工具无需修改 Agent。MCP 配置由同一执行计划和当前会话生成，不新增独立生效的 mcp_tools/tool_bindings 列表。config 是运行配置，arguments 是模型本次调用输入，不能混用。

启动时有两个只读核验结果：ToolHost 返回 session 中真正可路由的工具，AgentHost 返回模型经过 MCP 或原生适配后真正可见的工具。两者都必须与同一份 `ExecutionPlan.tools` 完全一致；它们不是名为 `actual_tools` 的第二份配置，也不能相互覆盖。

## 2. 业务与结果

Hub 数据格式目标见主方案第 9 章：标准化 JSONL 行复用 task: TaskSpec 与可选 private_data: TypedConfig，身份仍在 task.dataset/sample_id；不另加 data_id 或重复顶层 dataset 字段。数据 revision 与代码包 version 分开，一次样本输入只选择本地内容或 Hub 固定版本之一，在 prepare/Bridge 准备阶段解析后使用同一 EpisodeRequest。运行配置不写入数据行。行格式和 Hub 数据 API 校验尚待迁移，当前状态集中见[Hub 迁移说明](source_refactoring_plan.md#114-hub-与数据输入迁移)。

| 概念 | 规范名 | 处理旧字段 |
|---|---|---|
| 通用指令型任务正文 | instruction | 源 question、QUESTION、problem、code_problem、problem_statement 在语义确为“待执行指令”时经 Adapter 映射；SciTab claim 等不同业务概念保持自己的统一字段 |
| 工具会话名 | name | binding_name 不进入规范 ToolCall |
| 组件实现引用 | implementation | 工具调用不再用 tool 另起名字；角色字段 agent/backend/scorer 用来表达不同组件的职责 |
| 评分器选择 | scorer | RunSpec.scorer 是唯一作者配置，ExecutionPlan.scorer 只锁定同一选择；删除 evaluation_scorer、training_scorer 和 ScoringPlan |
| episode 奖励标量 | reward | Scorer 在 ScoreResult.reward 产生一次；评测直接读取，Bridge 向训练框架复制到 FrameworkSample.reward，不重算 |
| 环境转移上限 | max_environment_steps | Rust BudgetEnforcer 不使用 max_steps 别名 |
| 命令环境变量 | ExecRequest.environment_variables | 不使用 environment，以免与运行组件选择混淆 |

正式评分只在 attempt 得到最终 Outcome 后产生：每个 attempt 至多调用一次 Scorer，Server 只接纳一个 attempt 的结果作为 episode 权威评分。进入评分前失败没有 ScoreResult；已经调用后 status=ok/error 的结果都必须保留。ScoreInput 和 ScoreResult 不定义 RewardPolicy、StepReward、RewardAssignment、purpose 或 stage；RunSpec.purpose 只选择下游消费方。环境原生反馈 environment_reward、最终 reward、任务是否成功 success 含义不同，不能互相冒充。FrameworkSample.reward 的复制不意味着 Bridge 有权重算评分。

`ScoreInput` 不含 `remaining_timeout_ms`。Scorer 的剩余时间只从 `ScoringContext` 读取；调用 harness 时再取该剩余时间与 `private_data.data.evaluation_plan.timeout_ms` 的较小值。

SciTab.claim 是待核验陈述，contexts 是背景证据，answer 是私有参考答案，final_answer 是 Agent 最终提交的有序内容段。这些有不同含义与类型，不强行改为 instruction。新数据集可扩展新的业务概念，但不能把已经存在的公共概念换名重定义。

## 3. 身份、摘要与时间

- task_id 标识规范任务；sample_id 是原始数据集内样本键；episode_id 标识一次采样执行；attempt_id 标识重试尝试。传递与返回时均使用原名原值。
- input_digest 校验完整 input 信封；plan_digest 校验去掉 plan_digest 字段后的完整 ExecutionPlan，包含 task 与 private_data。统一使用参考 canonical_bytes 的排序紧凑 UTF-8 JSON，然后 SHA-256；不宣称采用另一套通用 JSON canonicalization 标准。
- plan_digest 包括 attempt_id，因此重试摘要改变，但执行配置不变。摘要用于内容一致性，不替代可信传输、租约或权限校验。
- plan_digest 只保存在受信 Server/Worker 计划记录中。因为它覆盖 private_data，用户可读 TrajectoryManifest 不携带该摘要，避免低熵私有答案被枚举比对。
- total_timeout_ms 是首次接纳起的总时长；deadline_at_ms 是首次接纳时间加该时长。Server 以它计算派发时只减的 remaining_timeout_ms。Worker RPC 核验该值后，Supervisor 用本机单调时钟建立本次接收后的截止时间；运行中不把总时长重新计时，也不再依赖墙上时钟。
- timeout_ms 是某次工具、命令或评分操作的上限，score_reserve_ms 是为最终评分保留的时长。它们不能延长总截止时间。
- Usage 与 limits 中各计数按 episode 累计，重试不能清零已消费的预算。Server 通过 DispatchRequest.consumed_usage 传递已确认用量；Rust `retry_execution_plan` 固定配置与截止时间。生产持久账本、跨 Worker 恢复、未知输出用量处理和 lease fencing 尚未实现。
- 每条 GenerationEvent.output_token_count 必填，并作为预算统计值；output_token_ids、output_logprobs、loss_mask 是可选的详细训练轨迹，存在时必须对齐且 token 数与 count 一致。TrainingSpec.require_token_trace=true 时缺少详细轨迹的结果不得进入训练，普通评测仍可保存。

## 4. 校验与扩展要求

当前 `scripts/build_contracts.py` 是本地参考包的过渡协议源，生成 schema 与递归字段字典；不要直接修改生成文件。目标生产协议只编辑 `contracts/proto/uenv/v1/*.proto`，由统一工具生成 Rust/Python 类型、RPC stub、核心 JSON schema 和字段字典。数据集业务字段只编辑包内 `models.py`，发布工具生成包 schema；用户不维护 `schemas/` 目录。所有九个参考数据集仍通过 scripts/build_examples.py 同步生成，完成 proto 工具链迁移前不能把这套本地生成方式描述为生产现状。

本地 Rust `PlanResolver` 校验请求身份、input 摘要、精确组件、工具授权、harness、镜像优先级、预算和配置锁定。组件 catalog、Agent profile 与 image_resolver 是显式注入的可信依赖；它不接收独立 package 或特定 Worker capabilities。PackageManifest 在发布时转换为 catalog 元数据，运行时只沿 `RunSpec.environment/scorer` 已选引用读取对应角色入口、配置 schema 和要求。`required_capabilities` 由解析器汇总，Placement 再选择 Worker。Python `fixture_plan.py` 只生成静态示例，不是生产包注册协议或真实镜像解析器。

新增数据集只在 Adapter 接收源字段别名。发布工具从 models.py 生成的 schema 拒绝旧别名与未知字段；不提供在 Worker 同时兼容多套名字的 get/fallback 链。未来正式升级需要在导入边界做显式版本转换，不能把多套兼容逻辑散到主流程。

自动测试可以拒绝已知别名、未知字段、摘要冲突和多份组件选择。任意用户模型的语义是否重复仍需包发布审查；不能声称生成的 JSON Schema 能自动理解并消除所有同义词。

评分依据不另设存储包装类型；PreparedSample.private_data 是作者侧 UEnvModel；SDK 封装为 TypedConfig 后，在 EpisodeRequest.private_data、ExecutionPlan.private_data、ScoreInput.private_data 中原名传递。材料不正确配对属于导入错误，由受信 Adapter/prepare 负责；本地不宣称 schema 或 plan_digest 能判断标准答案是否正确。

DatasetAdapter.normalize(record) 是确定转换，sample_id 只由 PreparedSample 返回；数据 revision 由发布/准备流程记录，episode seed 只在 EpisodeRequest，不再另建 ImportContext。Rust Supervisor 接受一份 DispatchRequest，其中逐 episode 执行配置只有 ExecutionPlan；remaining_timeout_ms 与 consumed_usage 只收紧时间并延续累计用量。HarnessRequest 使用 outcome，唯一评测配置保留在 private_data.data.evaluation_plan.harness。是否需要公共互联网从所选 Environment 的包元数据锁定到 ExecutionPlan.internet_access，后者是 task session 的唯一执行值。backend config 已删除 network_policy 和 workspace_root_profile，runtime_profile 不得承载访问规则或覆盖该值。


## 5. Outcome 合并后的字段归属

| 含义 | 唯一字段名 | 填写与消费规则 |
|---|---|---|
| 最终提交内容 | Outcome.final_answer | Agent 填写；Environment 收集后仍使用原名；Scorer 读取定稿后的副本 |
| 提交的文件等产物 | Outcome.artifacts | Agent 可提交引用，Environment 可收集补充；进入评分前检查产物引用及完整性 |
| 交互结束原因 | Outcome.termination_reason | Agent 提交必须结束；Environment.snapshot 的过程快照显式使用 in_progress；最终评分拒绝 in_progress |
| 评分所需环境状态 | Outcome.state | 仅 Environment 填写；Agent 携带此字段时拒绝 |
| 动作后的公开观测 | Transition.observation | 目标轨迹通过 EnvironmentTransition.transition 引用同一类型；不在顶层重复字段，也不使用 observation_after |
| 动作前的公开观测 | EnvironmentTransition.observation_before | 历史前态，与动作后的 observation 不同 |

Outcome 只定义一次，提交、收集、评分和最终结果不再通过改名表达阶段。模型一次响应的 response 不等于最终提交 final_answer；EpisodeResult.execution_status 是系统执行状态，Outcome.termination_reason 是成功形成候选结果时交互为何停止，两者不互换。`cancelled`、`error` 不属于 termination_reason：系统取消或失败分别由 execution_status 与 ErrorRecord 表达。EpisodeResult 和 TerminalEvent 不再复制 termination_reason；未形成终态 Outcome 时可保留 `in_progress` 快照，但不能进入评分。

轨迹生命周期事件使用 StateEvent.phase，避免与 Outcome.state 的环境评分状态混用。后端命令的环境变量使用 ExecRequest.environment_variables，RunSpec.environment 与 ExecutionPlan.environment 始终只表示 Environment 组件选择。

EnvironmentContext 只读的 deadline/seed 是 ExecutionPlan 派生的执行值，不是覆盖入口。资源 session 由后端 open 返回，系统把 EnvironmentContext 的统一资源操作绑定到该 session；Environment 不接收 backend 类型或配置，也不再次选择 backend。传给不同权限角色的 SessionRef 只是同一会话的身份引用。所有实际执行选择仍来自唯一 plan；组件与模型连接器不得在内部另读默认配置覆盖它。


## 6. 轨迹复用 Transition

EnvironmentTransition 仅定义 environment_step_index、observation_before、action、transition。transition 引用既有 Transition，其 observation、terminated、episode_truncated、environment_reward 不在轨迹内容顶层再次声明。AgentRuntime 在执行动作前分配 index，执行后校验并独立复制，用户不手写这些轨迹字段。

TrajectoryManifest 使用 `trajectory_status=scoring_checkpoint/final_complete/final_partial` 区分评分前快照、完整最终轨迹和缺失事件的最终轨迹，不再用 `complete` 同时表达“尚未结束”和“记录不完整”。`created_at_ms` 只表示当前 manifest 的创建时间；`event_count` 与从 0 连续的 sequence 一起校验事件数。权威轨迹始终记录完整标准事件，查询端 summary 是派生视图，不进入 RunSpec。

该结构已经同步到 scripts/build_contracts.py、生成 schema、字段字典与 Rust 记录入口。校验拒绝同时携带顶层 observation/terminated 等旧字段及 transition 的双重表示。此结构不改变 ExecutionPlan 的唯一配置来源。
