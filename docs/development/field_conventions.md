# 统一字段命名与归属

本规范约束目标协议、Python 参考代码、生成示例与用户扩展。一个概念只有一个规范字段名；配置解析后只有一个生效位置。不同边界可以传递同名字段或不可变快照，不允许同一接收者遇到两份可独立修改的权威值。

公开字段必须描述 AI 任务作者能直接判断的业务意图，不能要求用户理解操作系统实现。网络需求只使用 boolean `internet_access`；`sandbox_mode`、`network_policy`、`system_access_level`、`syscall_profile`、`command_mode` 等字段禁止进入公开协议。进程、容器、系统权限和隔离规则由 Backend 内部实现。自动校验会扫描公共 Schema，防止这些底层字段重新出现。

## 0. 字段发现和扩展边界

字段是否已有不能靠作者搜索源码或阅读生成 schema 判断，也不能把全部内部协议直接展示给普通用户。数据集作者默认只看到 `PreparedSample`、本包 `models.py` 类型和已声明入口的方法参数；运行用户只看到严格分组的 RunSpec。`uenv describe PreparedSample`、`uenv describe RunSpec` 和 `uenv describe package@version:TypeName` 分别提供对应视图，Python SDK 类型提示与这些输出来自同一契约。

数据集字段采用 HUD 的方式，在 Python `models.py` 中通过类型标注定义并生成 schema。运行配置采用 Harbor 的方式，固定为严格的 RunSpec，分组包含 Environment、Agent、Backend、Model、Tools 和 Limits，Scorer 作为同级配置：评测和训练必填，轨迹采集可省略；提交前校验类型、必填项和组合兼容性。系统对象是封闭类型，未知字段直接拒绝。只有 Adapter、Environment 或 Scorer 理解的业务内容才加入 `models.py`；需要改变系统调度、权限、模型、预算、评分生命周期或结果处理的新能力必须修改核心 proto。

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
| 一次工具执行 | `tool_call` | 一次获准的工具调用；使用 `tool_call_id` 和 `tool_call_count` |

Agent SDK 如果内部使用 turn、iteration 等名称，接入层必须映射到上述公共名称，不能把框架术语写入 UEnv 公共字段。

| 字段 | 提交时来源 | 执行时位置 | 消费规则 |
|---|---|---|---|
| run_id | BatchRequest.run_spec.run_id；输入只在 RunSpec 中出现 | ExecutionPlan.run_id | 从批次共同配置派生，关联作业；不在 Worker 再查配置 |
| episode_id、seed | EpisodeRequest | ExecutionPlan 同名字段 | 采样执行身份和种子，重试保持 |
| task | EpisodeRequest.task | ExecutionPlan.task | 完整公开 TaskSpec 原值，不增加 task_view 等副本类型 |
| private_data | 准备入口从原始样本转换或读取标准化样本；Adapter 返回 UEnvModel，SDK 封装后与 task 配对进入 EpisodeRequest | ExecutionPlan.private_data → ScoreInput.private_data | 不生成中间材料文件；公开任务与公开轨迹不得携带 |
| dataset_package、environment、scoring、agent、backend、model、tools、limits、training、purpose | RunSpec 同名字段 | ExecutionPlan 同名字段 | 保持名称与语义，组件坐标在原位置升级为已锁定引用；purpose 只选择结果消费方 |
| runtime.image | RunSpec、TaskSpec、PackageManifest 的同名字段 | ExecutionPlan.runtime.image | 候选按 run > task > package 选择；Worker 仅消费最终值 |
| runtime.image_source | 无 | ExecutionPlan.runtime.image_source | run/task/package，仅供审计，不参与再次选择 |
| attempt_id | Server 生成，用户不填写 | ExecutionPlan.attempt_id | 正常为 1；只有基础设施重试才增加 |
| required_capabilities | 包及已锁定组件声明的运行功能需求并集 | ExecutionPlan.required_capabilities | 仅供 Worker 调度和兼容性检查；不授予文件、网络或宿主访问 |
| internet_access | Environment 所属 PackageManifest | ExecutionPlan.internet_access | boolean；表示是否需要公共互联网。Server 核验后锁定，Worker/Backend 唯一消费；RunSpec 与组件配置不能覆盖 |
| request_id | EpisodeRequest | Server 单条提交记录 | 单条 episode 请求的幂等键；不作为批次身份，不进执行计划 |
| batch_id | BatchRequest、BatchReceipt 与各 EpisodeRequest | Server 批次记录 | 同一批的唯一身份；不是单条幂等键，不进执行计划 |
| sample_index | EpisodeRequest | Server 批次成员记录 | 该 episode 在批内的位置；不进执行计划 |
| retry | RunSpec.retry | Server | Worker 不决定 episode 重试 |
| trajectory_retention_days | RunSpec | Server | 只控制权威轨迹保存期，不进入 ExecutionPlan，不改变事件内容 |
| lease | Server | DispatchRequest.lease | 授权与失效 fencing，独立于任务与执行配置 |
| remaining_timeout_ms | Server 根据 ExecutionPlan.deadline_at_ms 派生 | DispatchRequest | 只能收紧时间，不是第二份用户配置；Worker RPC 核验后，Supervisor 用它建立本机单调截止时间 |
| consumed_usage | Server 的 episode 用量账本 | DispatchRequest | 此前 attempt 已确认的累计用量；Worker 据此继续计数，首次为全 0 |

BatchRequest 固定包含 batch_id、run_spec、episodes；run_spec 是本批唯一完整 RunSpec，episodes 非空。每条 EpisodeRequest 不包含 run_id、run_spec 或组件配置；ExecutionPlan.run_id 从 run_spec.run_id 派生。相同 run_id 的完整配置必须一致，Server 在保存任务的事务内核验并拒绝冲突；旧配置只供比较，不作为补值或覆盖来源。重复 batch_id 的请求内容必须一致；成员 request_id 的幂等判断包含所属 run_id，避免跨运行复用请求身份。上述数据库事务与幂等接入仍待生产实现。

公开客户端只提交批次；单条执行使用只含一个成员的 BatchRequest。因此每个 EpisodeRequest 都有自己的 request_id，同时也总有 batch_id 和从 0 开始的 sample_index。三者分别表示成员幂等、批次身份和批内位置，不能互相代替。

`purpose` 只接受 evaluation、training、trajectory_collection；training 仅在 purpose=training 时必填，其余用途禁止填写。scoring.enabled 必须显式填写，evaluation/training 要求 true，trajectory_collection 可设 false。是否评分只读该布尔值；关闭时禁止 scoring.config，不增加 enable_scoring 或 collect_only。training 只描述在线训练接入所需版本/轨迹条件，不是第二种评分模式。

绝对 `deadline_at_ms` 只保存在受信 ExecutionPlan，供 Server 审计和派发时计算剩余量。Worker RPC 拒绝被放大的 `remaining_timeout_ms`；进入 Supervisor 后，运行截止时间只由“本机单调时钟当前值 + 收到的 remaining_timeout_ms”建立，不再用本机墙上时钟重算绝对 deadline。调用 Agent、Environment、Tool 或 Scorer 子进程时只传当前剩余毫秒，子进程不能延长或重算。

ExecutionPlan 不接受 episode、run、components、tool_bindings 等旧结构。WorkerRegistration.components 是唯一安装清单，Environment、Agent、Scorer、Tool、Backend 等可执行组件都登记在这里，不再另设 backends 清单；capabilities 表示 Worker 可提供的运行能力；capacity 是并发 episode 槽位总数，Heartbeat.available_slots 是它的当前剩余快照；resource_capacity 是 CPU/内存/存储总量，BackendSpec.resources 是单个 episode 的资源申请。四者含义和单位不同，调度同时满足槽位与资源余量，不能相互覆盖。

PackageManifest.provided_tools 只登记包能够提供的 ToolSpec；RunSpec.tools 是用户为本次 run 明确选择的 ToolBinding；ExecutionPlan.tools 是锁定工具版本、配置和执行位置后的唯一实际工具表。选择某个工具只允许调用该接口，不授予任意网络、路径或宿主访问。三者处于发布、选择、执行三个阶段，不能并行生效，也不能自动合并包或 SDK 的默认工具。

用户只填写工具 name/implementation/config。工具接入方式由所选 Agent 的接入代码固定，PlanResolver 不做公共接口协商，ExecutionPlan.tools 不保存 interface/adapter。ToolSpec 只描述工具自身的入口、类型和运行要求；AgentManifest 保留发布信息、生成的 provided_tools 和真正必需的工具名，不维护 supported_interfaces。MCP 配置由同一执行计划和当前会话生成，不新增独立生效的 mcp_tools/tool_bindings 列表。config 是运行配置，arguments 是模型本次调用输入，不能混用。本地参考已移除协商字段；生产接入范围见[迁移说明](source_refactoring_plan.md#113-工具接入与验收)。

ToolSpec.execution_scope 和 required_capabilities 由工具作者声明，分别指定执行位置和所需 Worker 能力；PlanResolver 校验后写入唯一的 ExecutionPlan.tools，Worker 据此路由与检查能力。execution_scope 保留 agent_state、sandbox、external_service，移除接口协商不移除这些进程和资源边界；Agent 接入代码不能覆盖它们。原 ToolInterface 中的这两个字段迁移到 ToolSpec，不新增同义字段。

工具的发布入口统一来自所属包的 tools.py；数据集包和独立工具包使用同一 ToolSpec。Environment/AgentRunner 不另注册工具。系统注入的 context 不属于 arguments，不生成模型输入字段；环境实例只在受管 Host 内供获准工具使用，MCP 不能传入或取得实例。自动生成的 MCP 连接由 Worker 管理，Agent 接入代码读取并交给框架；MCP 只转发到 step，不重新声明工具配置。

启动时有两个只读核验结果：ToolHost 返回 session 中真正可路由的工具，AgentHost 返回模型经过 MCP 或原生适配后真正可见的工具。两者都必须与同一份 `ExecutionPlan.tools` 完全一致；它们不是名为 `actual_tools` 的第二份配置，也不能相互覆盖。

### 1.1 用户输入与完整 RunSpec

YAML 文件与 SDK 输入共用 Bridge 配置规范化函数，输出完整 RunSpec 后放入 BatchRequest.run_spec，一次提交配置与任务；不存在独立配置注册接口。系统字段默认值只定义在核心契约，组件参数默认值只定义在对应模型并生成 schema；本地过渡来源仍是 scripts/build_contracts.py 与包 models.py。不得再建立一份数据集默认运行配置表或让 Worker 补值。文件填写步骤见[用户指南第 2 节](../guides/user_guide.md#2-填写运行配置)。

提交入口仅为缺失字段应用声明默认值，不覆盖显式值，不把 null 当成省略；未声明默认值的必填字段缺失时报错。可省略的 config 先作为空对象处理，再按组件模型补值并校验。提交后的 RunSpec 和 ExecutionPlan 保持完整必填约束；schema 的 default 是提交注解，不代表 Server/Worker validator 自动修改输入。当前参考只应用明确对象属性和数组元素的默认值，不猜测条件或联合分支的默认值。

config 只能描述组件独有行为，不能重复 model、limits 等公共执行参数。ProcessBackendConfig.runtime_profile 仅为本机运行环境选择；ContainerBackendConfig 不包含该字段，引擎连接属于 Worker 部署配置。OpenHands 配置不再包含固定 history_policy 或 sdk_iteration_limit；统一模型调用上限只使用 limits.max_generations。SDK 内部迭代处理属于框架接入，不得形成另一份用户轮数配置。

## 2. 业务与结果

Hub 数据格式目标见主方案第 9 章：标准化 JSONL 行复用 task: TaskSpec 与可选 private_data: TypedConfig，身份仍在 task.dataset/sample_id；不另加 data_id 或重复顶层 dataset 字段。数据 revision 与代码包 version 分开，一次样本输入只选择本地内容或 Hub 固定版本之一，在 prepare/Bridge 准备阶段解析后使用同一 EpisodeRequest。运行配置不写入数据行。行格式和 Hub 数据 API 校验尚待迁移，当前状态集中见[Hub 迁移说明](source_refactoring_plan.md#114-hub-与数据输入迁移)。

| 概念 | 规范名 | 处理旧字段 |
|---|---|---|
| 通用指令型任务正文 | instruction | 源 question、QUESTION、problem、code_problem、problem_statement 在语义确为“待执行指令”时经 Adapter 映射；SciTab claim 等不同业务概念保持自己的统一字段 |
| 工具会话名 | name | binding_name 不进入规范 ToolCall |
| 组件实现引用 | implementation | 工具调用不再用 tool 另起名字；agent/backend 和工具的 implementation 选择相应组件；数据集角色共用 dataset_package，不另设 scorer 实现选择 |
| 数据集代码包 | dataset_package | RunSpec 唯一选择；ExecutionPlan 锁定同一包，Environment 与 Scorer 不另设 implementation |
| 是否评分 | scoring.enabled | 用户明确填写，Worker 只按此值决定是否调用包内 Scorer；参数仅在 scoring.config，关闭时禁止参数 |
| episode 奖励标量 | reward | Scorer 在 ScoreResult.reward 产生一次；评测直接读取，Bridge 向训练框架复制到 FrameworkSample.reward，不重算 |
| 命令环境变量 | ExecRequest.environment_variables | 不使用 environment，以免与运行组件选择混淆 |

开启评分时，正式评分只在 attempt 完成回答提交和产物收集后产生：每个 attempt 至多调用一次 Scorer，Server 只接纳一个 attempt 的结果作为 episode 权威评分。进入评分前失败没有 ScoreResult；已经调用后 status=ok/error 的结果都必须保留。ScoreInput 和 ScoreResult 不定义 RewardPolicy、StepReward、RewardAssignment、purpose 或 stage；RunSpec.purpose 只选择下游消费方。可选 generation_rewards 与整体评分在同次调用中返回，规则见第 2.1 节。整体 reward、逐次生成 reward、任务是否成功 success 含义不同，不能互相冒充。FrameworkSample.reward 的复制不意味着 Bridge 有权重算评分。

`ScoreInput` 不含 `remaining_timeout_ms`。Scorer 的剩余时间只从 `ScoringContext` 读取；调用 harness 时再取该剩余时间与 `private_data.data.evaluation_plan.timeout_ms` 的较小值。Rust 受控入口必须再次核验原 final_answer/private_data/state 并收紧时间，不能只依赖 Python 辅助函数。

SciTab.claim 是待核验陈述，contexts 是背景证据，answer 是私有参考答案，final_answer 是 Agent 最终提交的有序内容段。这些有不同含义与类型，不强行改为 instruction。新数据集可扩展新的业务概念，但不能把已经存在的公共概念换名重定义。

### 2.1 整体评分与逐次生成评分

以下约定已落实到参考 SDK、schema 和 Rust 评分校验；真实训练消费的接入进度见参考实现说明。Scorer 在交互结束后调用一次，同时返回整体评分和可选过程评分，不增加第二种请求、Scorer 类或评分阶段配置。

| 字段 | 类型 | 谁填写 | 谁读取及具体作用 |
|---|---|---|---|
| ScoreResult.reward | number；评分错误时为 null | 数据集 Scorer；错误结果由 Worker 填 null | 评测查询用于整体统计，训练端用于整体奖励训练；不被过程分数覆盖 |
| ScoreResult.generation_rewards | 可选对象数组 | 数据集 Scorer；不提供时省略，空列表同样表示无过程评分 | Worker 校验，Server 随结果保存，Bridge 保留关联交给支持过程奖励的训练端；评测端可逐次查看 |
| generation_rewards[].generation_id | 非空 string | Scorer 从 ScoreInput.trajectory_ref 指向的记录中取已有 ID | Worker 验证其属于当前 episode 的当前 attempt，定位唯一模型生成；不新建编号 |
| generation_rewards[].reward | 有限 number | 数据集 Scorer | 下游用于该次生成的评价或训练；可为零或负数，不统一限制为 0 到 1 |

每项仅含 generation_id 与 reward；不另设 turn、step、token 范围或环境动作评分字段。模型生成内部不再拆分评分。模型生成引用与已有轨迹身份保持一致，列表顺序没有归属含义，不允许重复 generation_id。

Worker 校验字段类型、有限数值、ID 唯一性，以及评分前轨迹中的生成身份。引用不存在的生成、其他 attempt 的生成或重复 ID 时，整次评分失败；不得悄悄丢掉非法项。只能评价已记录且有模型响应内容的生成。轨迹缺失到无法验证关联时拒绝相关过程评分，不补造生成或分数。status=error 时整体 reward 为 null，generation_rewards 省略，不把部分失败的过程分数当作正式结果发布。

Scorer 可结合对应生成触发的工具执行和环境反馈评分；这些记录不是新的评分单位。无法确认因果关联时，只把材料用于整体评分。未评分的生成缺项表示未知，不视为零分。整体分数与过程分数分别表达不同范围的评价，不默认求和、平均或相互复制。

沿用同一个 ScoreResult 和 score 事件保存结果，不回写 GenerationEvent 新增第二份权威 reward。现有 FrameworkSample.reward 仍是整体 reward；FrameworkSample.generation_rewards 原样保留过程评分及原有生成身份，真实训练适配器接入时须一同传递，再由训练框架决定如何映射到实际 token，不能由 Scorer 输出 advantage 或训练张量。

## 3. 身份、摘要与时间

- task_id 标识规范任务；sample_id 是原始数据集内样本键；episode_id 标识一次采样执行；attempt_id 标识重试尝试。传递与返回时均使用原名原值。
- input_digest 校验完整 input 信封；plan_digest 校验去掉 plan_digest 字段后的完整 ExecutionPlan，包含 task 与 private_data。统一使用参考 canonical_bytes 的排序紧凑 UTF-8 JSON，然后 SHA-256；不宣称采用另一套通用 JSON canonicalization 标准。
- plan_digest 包括 attempt_id，因此重试摘要改变，但执行配置不变。摘要用于内容一致性，不替代可信传输、租约或权限校验。
- plan_digest 只保存在受信 Server/Worker 计划记录中。因为它覆盖 private_data，用户可读 TrajectoryManifest 不携带该摘要，避免低熵私有答案被枚举比对。
- total_timeout_ms 是首次接纳起的总时长；deadline_at_ms 是首次接纳时间加该时长。Server 以它计算派发时只减的 remaining_timeout_ms。Worker RPC 核验该值后，Supervisor 用本机单调时钟建立本次接收后的截止时间；运行中不把总时长重新计时，也不再依赖墙上时钟。
- timeout_ms 是某次工具、命令或评分操作的上限，finalize_reserve_ms 是为提交校验、冻结及可选评分保留的时长，指系统收尾预算，不对应 Environment 方法。它们不能延长总截止时间。
- Usage 与 limits 中各计数按 episode 累计，重试不能清零已消费的预算。Server 通过 DispatchRequest.consumed_usage 传递已确认用量；Rust `retry_execution_plan` 固定配置与截止时间。生产持久账本、跨 Worker 恢复、未知输出用量处理和 lease fencing 尚未实现。
- 每条 GenerationEvent.output_token_count 必填，并作为预算统计值；output_token_ids、output_logprobs、loss_mask 是可选的详细训练轨迹，存在时必须对齐且 token 数与 count 一致。TrainingSpec.require_token_trace=true 时缺少详细轨迹的结果不得进入训练，普通评测仍可保存。

## 4. 校验与扩展要求

当前 `scripts/build_contracts.py` 是本地参考包的过渡协议源，生成 schema 与递归字段字典；不要直接修改生成文件。目标生产协议只编辑 `contracts/proto/uenv/v1/*.proto`，由统一工具生成 Rust/Python 类型、RPC stub、核心 JSON schema 和字段字典。数据集业务字段只编辑包内 `models.py`，发布工具生成包 schema；用户不维护 `schemas/` 目录。所有九个参考数据集仍通过 scripts/build_examples.py 同步生成，完成 proto 工具链迁移前不能把这套本地生成方式描述为生产现状。

本地 Rust `PlanResolver` 校验请求身份、input 摘要、精确组件、工具授权、harness、镜像优先级、预算和配置锁定。组件 catalog、Agent profile 与 image_resolver 是显式注入的可信依赖；它不接收独立 package 或特定 Worker capabilities。PackageManifest 在发布时转换为 catalog 元数据，运行时只沿 `RunSpec.dataset_package` 已选引用读取对应角色入口、配置 schema 和要求。`required_capabilities` 由解析器汇总，Placement 再选择 Worker。Python `fixture_plan.py` 只生成静态示例，不是生产包注册协议或真实镜像解析器。

新增数据集只在 Adapter 接收源字段别名。发布工具从 models.py 生成的 schema 拒绝旧别名与未知字段；不提供在 Worker 同时兼容多套名字的 get/fallback 链。未来正式升级需要在导入边界做显式版本转换，不能把多套兼容逻辑散到主流程。

自动测试可以拒绝已知别名、未知字段、摘要冲突和多份组件选择。任意用户模型的语义是否重复仍需包发布审查；不能声称生成的 JSON Schema 能自动理解并消除所有同义词。

评分依据不另设存储包装类型；PreparedSample.private_data 是作者侧 UEnvModel；SDK 封装为 TypedConfig 后，在 EpisodeRequest.private_data、ExecutionPlan.private_data、ScoreInput.private_data 中原名传递。材料不正确配对属于导入错误，由受信 Adapter/prepare 负责；本地不宣称 schema 或 plan_digest 能判断标准答案是否正确。

数据集包内 DatasetAdapter.normalize(row) 是确定转换，sample_id 只由 PreparedSample 返回；数据 revision 由发布/准备流程记录，episode seed 只在 EpisodeRequest，不再另建 ImportContext。Rust Supervisor 接受一份 DispatchRequest，其中逐 episode 执行配置只有 ExecutionPlan；remaining_timeout_ms 与 consumed_usage 只收紧时间并延续累计用量。HarnessRequest 使用 final_answer 和可选 state，唯一评测配置保留在 private_data.data.evaluation_plan.harness。是否需要公共互联网从所选 Environment 的包元数据锁定到 ExecutionPlan.internet_access，后者是 task session 的唯一执行值。backend config 已删除 network_policy 和 workspace_root_profile，runtime_profile 不得承载访问规则或覆盖该值。


## 5. 最终回答、产物与评分状态的字段归属

| 含义 | 唯一字段名、类型 | 谁填写，谁读取，以及作用 |
|---|---|---|
| 最终提交内容 | final_answer：ContentPart[]；ScoreInput 必需，completed 的 EpisodeResult 必需，可以为空 | Agent.run 返回；Worker 原样放入 ScoreInput 和 EpisodeResult；Scorer 判分，调用方读取回答 |
| 评分所需环境状态 | ScoreInput.state：可选 UEnvModel，传输为 TypedConfig | Environment.state_snapshot 返回，Worker 校验并交给 Scorer；不进入公开 EpisodeResult |
| 配对的答案和隐藏测试 | ScoreInput.private_data：可选 UEnvModel，传输为 TypedConfig | 包内 DatasetAdapter 转换或标准化数据提供；准备入口配对，Worker 仅交给 Scorer |
| 交互结束原因 | EpisodeResult.termination_reason：string 枚举 | Worker 根据当前观测、预算拒绝和生成完成信息填写；调用方用于区分停止原因 |
| 本次公开交互结果 | Observation | reset 或工具提供；Agent 读取；工具结果在 ToolResult.observation 中保存同一类型 |

ScoreInput 的完整字段为 task、final_answer、trajectory_ref、可选 state、可选 private_data。task 引用 TaskSpec，trajectory_ref 引用评分前轨迹清单 ArtifactRef；Worker 组装，Scorer 读取任务、最终提交、轨迹及私有评分材料。HarnessRequest 原名复制 final_answer、可选 state 和 private_data，并从 ScoringContext 获取 remaining_timeout_ms；不包含另一套输出包装或执行配置。

EpisodeResult 直接保存 final_answer、termination_reason，以及既有身份、execution_status、score、trajectory_ref、usage、时间、error、cleanup_status。state 和 private_data 不在公开结果中。final_answer 的 ContentPart 可以是文本、通过 artifact 引用的文件或有类型的结构化内容；补丁、图片或报告属于最终提交时都放在这里。ScoreInput、EpisodeResult、HarnessRequest 原名传递同一份内容，不另设 artifacts，也不增加输出包装。

Agent 显式选择最终提交：直接返回文本，或在交互结束前经受控工具生成文件后返回引用。Worker 校验 ContentPart、引用及文件完整性，不自动从工作目录、工具结果或模型响应中补出答案。Environment 不提供 finalize/collect_artifacts；文件存储仍由现有系统能力负责。final_answer=[] 适用于没有显式提交内容的任务，例如仅按环境状态评分的任务，不能表示“文件在另一份 artifacts 列表中”。

termination_reason 仅取 final_answer、environment_terminal、environment_truncated、budget_exhausted。优先依据当前观测的 terminated，再看 episode_truncated；否则依据公共预算实际拒绝、交互截止时间或最后一次生成的 length 截断，最后才是正常显式提交。用量恰好达到上限但未因此停止，不判为预算截断。Agent 不填写该字段。系统失败和取消使用 execution_status 与 ErrorRecord；Agent 未返回时不生成默认回答或结束原因。显式返回 [] 与缺少 final_answer 不同：前者可表示合法的仅环境状态评分任务，后者不能满足 completed 的校验。

GenerationEvent.response 保留一次模型响应，不等于 Agent 最终提交的 final_answer。TerminalEvent 不复制 termination_reason。StateEvent.phase 表示生命周期，ScoreInput.state 表示评分状态；命令环境变量使用 ExecRequest.environment_variables，RunSpec.environment 与 ExecutionPlan.environment 表示环境配置。

EnvironmentContext 只读的 deadline/seed 是 ExecutionPlan 派生的执行值，不是覆盖入口。资源 session 由后端 open 返回，系统把 EnvironmentContext 的统一资源操作绑定到该 session；Environment 不接收 backend 类型或配置，也不再次选择 backend。传给不同权限角色的 SessionRef 只是同一会话的身份引用。所有实际执行选择仍来自唯一 plan；组件与模型连接器不得在内部另读默认配置覆盖它。


## 6. 轨迹复用 Observation

Observation 只包含 content: ContentPart[]（默认 []）、terminated: bool（默认 False）、episode_truncated: bool（默认 False）。reset 或工具提供公开反馈；普通工具返回值由 SDK 包装。只有当前 Environment.reset 和获准访问该实例的工具可设置环境结束标志，Worker 校验并锁定结束事实。Agent 读取反馈并停止后续交互，不存在另一份 ToolResult.content/data。

ContentPart 的唯一字段定义如下，Observation、Message、最终提交、工具 content 和轨迹都引用它：

| 字段 | 类型与约束 | 谁填写、谁读取、具体作用 |
|---|---|---|
| kind | text / artifact / structured，必填 | 内容生产方填写，SDK/Worker 和接收方据此读取唯一对应内容 |
| text | string，仅 kind=text 必填 | 内容生产方提供原始文本，Agent/模型/Scorer 按所在消息用途读取 |
| artifact | ArtifactRef，仅 kind=artifact 必填 | 内容生产方引用文件，系统校验，接收方经受控文件能力读取 |
| structured | TypedConfig，仅 kind=structured 必填 | 作者传入已登记 UEnvModel，structured_part 自动包装；SchemaRegistry 校验 schema_ref 及内部 data，Agent/Scorer 可直接按模型读取 |

三种内容互斥，禁止一项同时填写 text 和 structured，也禁止未知模型、缺失 schema 或不符合字段类型的数据。structured.schema_ref 与 structured.data 复用 TypedConfig 的既有定义，不再定义一套结构化信封。models.observation 登记观测中使用的结构化业务模型，不是重新定义 Observation 根类型；已有登记类型直接复用。

统一模型输入转换在 Worker AgentRuntime 内完成，先于预算预占和 ModelProvider 调用：将 structured.data 编码成确定的 JSON 文本，其他项保持原样与原顺序；不覆盖原始观测，不新增执行配置。GenerationEvent.messages 记录转换后传给 ModelProvider 的消息，观测事件保留结构化项。完整的已登记模型校验属于 Host/RPC 的 SchemaRegistry 边界；Rust 参考目前校验内容种类、互斥字段和信封形状，不等同于生产跨进程完整 schema 校验。

工具调用本身就是动作：ToolCall.arguments 保存唯一输入，ToolResult.observation 保存唯一公开结果。ToolResult 另外保留 tool_call_id、status、output_truncated、可选 raw_output_ref/error，表示系统执行状态与记录关联；不与 Observation 混写。删除 EnvironmentTransition 和环境动作身份/计数，step 与 call_tool 不重复计数或记录。

TrajectoryManifest 使用 `trajectory_status=scoring_checkpoint/final_complete/final_partial` 区分评分前快照、完整最终轨迹和缺失事件的最终轨迹，不再用 `complete` 同时表达“尚未结束”和“记录不完整”。`created_at_ms` 只表示当前 manifest 的创建时间；`event_count` 统计实际保存的事件数；final_complete 的 sequence 从 0 连续，final_partial 保留原序号与缺口，不用最大序号代替事件数。权威轨迹始终记录完整标准事件，查询端 summary 是派生视图，不进入 RunSpec。

参考生成器、schema、SDK/Rust 与示例已删除旧动作模型和独立环境计数，工具公开内容统一为 ToolResult.observation；真实 RPC/框架接入范围见[工具迁移说明](source_refactoring_plan.md#113-工具接入与验收)。

## 7. 轨迹采集与可选评分的跨字段约束

RunSpec.dataset_package、environment、scoring 与 ExecutionPlan 同名传递；purpose 不隐式添加、替换或禁用评分器。Bridge 组装无评分批次时省略成员 private_data；Server 对受控请求中已有私有材料也只在开启评分时解析和下发。scoring.enabled=false 的 ExecutionPlan 不允许 private_data，不解析隐藏 harness，不创建 ScorerHost。

EpisodeResult.score 只在实际评分后存在，不能用空对象、null 或零分表示未评分。正常无评分采集允许 completed；开启评分时的 completed 必须同时有 status=ok 的 score。Rust validate_result_for_plan 校验此条件和评分器身份；Server 必须结合权威计划、完整 schema 和租约检查接纳，不能仅用 EpisodeResult 的可选字段规则。评分失败仍保留 error ScoreResult 和轨迹。

limits.finalize_reserve_ms 由用户填写，Rust BudgetEnforcer 从总预算扣出交互截止时间，保留给提交校验、冻结及可选评分；不增加另一份评分时间配置。不评分同样需要收尾。清理和上报继续遵循主方案的可恢复规则，不因评分跳过而省略。

轨迹采集沿用 TrajectoryEvent/TrajectoryManifest/trajectory_ref；无 score 事件不表示轨迹缺失。未评分没有 scoring_checkpoint，最终按事实完整性封存为 final_complete/final_partial。默认导出权威 attempt，历史尝试显式选择；派生筛选文件不修改原始轨迹，不自动发布为 Hub 数据版本。

PlainAgent 的轮数只使用 limits.max_generations，工具动作次数只使用 limits.max_tool_calls，不引入 max_rounds/max_steps 或 max_environment_steps。Agent 保有模型循环，Worker 唯一执行预算；工具反馈可继续驱动生成，环境结束时停止。PlainAgent 将无工具请求的完整 stop 回复作为 final_answer；其他 Agent 自行决定提交时机，length 不能伪装成完整回复。history_policy 只影响下一次模型输入，不影响轨迹保留。

Message.tool_calls 与 GenerationEvent.tool_calls 复用 ToolCall，前者只允许在 assistant 消息中出现；finish_reason=tool_calls 时后者必须非空，其余停止原因不携带该字段。归一化请求不是执行凭据，只有 Worker 接纳的 tool_call/tool_result 事件表示实际执行。Message.role=tool 必须填写已有 tool_call_id；不为反馈另造关联身份。工具实现与预算只能从既有 ExecutionPlan.tools 绑定，模型不能填写控制配置。

工具作者通过函数签名或既有 ToolExecutor 的输入模型定义参数，发布生成 ToolSpec.input_schema。models.action、PackageManifest.action_schema 和 Environment.action_model 在目标协议中删除；不为同一个工具再定义动作参数。复杂工具参数模型放 models.py，被工具直接引用。

AgentContext.call_tool(tool_call) 委托 Rust AgentRuntime.step(tool_call)；只有后者接纳执行、占用一次工具预算并产生 tool_call/tool_result。函数名不同表示 SDK 与 Worker 的调用层级，不表示两种操作；既有 ToolCall 的身份、name 和 arguments 原样关联。普通完整回答直接返回 final_answer，不生成 AnswerAction。

AgentContext.observation 初始为 reset 的返回值，随后为最近一次已返回工具调用的 Observation；它是公开反馈，不保证包含完整环境状态。Worker 以已接受的结束事实控制生命周期，不能被后续 False 标志清除。工具结束状态与 Agent 主动提交状态区分；一个操作只执行、计数和记录一次。

### 工具选择与原生定义的归属

- RunSpec.tools 的 name、implementation、config：运行用户填写；Bridge/Server 读取；决定本次可用工具与唯一工具配置。
- ToolCall.arguments：模型填写，Agent 接入代码保持语义转换；ToolHost 按已加载工具 schema 校验后执行。它是参数 JSON 对象，不带 TypedConfig 信封。
- AgentManifest.provided_tools：Agent 接入维护者的导出程序生成；组件目录读取；登记随 Agent 包发布的原生工具，不默认启用，不重复手写框架 schema。
- ResolvedToolBinding.native_agent：目录和解析器为原生工具填写；解析器、Worker 和 Host 读取；校验工具所属 Agent、版本和 digest，选择受管原生执行器。用户不填写；普通 UEnv 工具省略。
- SDK/MCP 选择由 Agent 集成代码固定。删除 supported_interfaces、ToolInterface、ToolSpec.interfaces 和 ResolvedToolBinding.interface/adapter；不迁入其他配置对象。
