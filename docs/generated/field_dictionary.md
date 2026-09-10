# UEnv vNext 字段字典

本文件由本地过渡生成器 scripts/build_contracts.py 生成。目标生产版本改由 contracts/proto/uenv/v1/*.proto 生成；本文件不是另一处可编辑协议，禁止手工编辑；修改生成器后重新生成。所有对象默认拒绝未知字段；表中的必填性针对完整传输对象；标注的默认值只由 Bridge 提交入口补齐，Server/Worker 校验不补值。run.yaml 展开后的完整 RunSpec 是唯一用户配置输入。

嵌套字段的必填是指其父对象已提供时；可选父对象省略时无需补子字段。

JSON 数字不得为 NaN/Infinity；时间统一毫秒。未提供的可选字段省略，不用空字符串代替 null；有明确允许空字符串的字段以定义为准。

TypedConfig 的 data 不是任意 JSON：必须递归满足 schema_ref 指向的版本化 schema。表中 $ref 继续展开到同名结构。新扩展只在包内 models.py 定义，由发布工具生成并注册 schema，不在主流程增加名称分支。

## ComponentRef

作者指定的组件坐标；提交时解析为 ResolvedComponent

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `version` | string | 是 | 精确版本或提交时解析的版本约束 |
| `digest` | string | 否 | 内容 SHA-256；必须对应实际字节 |

## ResolvedComponent

不可变组件引用；执行期间不得重新解析 latest

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `version` | string | 是 | 已解析精确版本 |
| `digest` | string | 是 | 内容 SHA-256；必须对应实际字节 |

## ArtifactRef

不可变产物引用；URI 不得含凭据

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `uri` | string | 是 | 支持的存储 URI；访问凭据由身份系统提供 |
| `digest` | string | 是 | 内容 SHA-256；必须对应实际字节 |
| `size_bytes` | integer | 是 | 字节数；最小值：0 |
| `media_type` | string | 是 | MIME 类型 |

## ErrorRecord

统一结构化错误；任务答错不属于基础设施错误

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `code` | string | 是 | 稳定大写错误码，如 SCORER_FAILED |
| `phase` | string | 是 | 发生阶段；枚举：validation, queue, prepare, agent, model, tool, environment, score, persist, cleanup |
| `message` | string | 是 | 可读说明，不放密钥或完整私有评分输入 |
| `retryable` | boolean | 是 | 由错误分类决定的重试资格；不是立即重试命令 |
| `operation_id` | string | 否 | 已经接纳的外部操作身份；按 phase 引用同一次操作已有的 generation_id、tool_call_id 或 environment_step_index 字符串，不另造第二套身份 |
| `diagnostic_ref` | ArtifactRef | 否 | 完整诊断材料引用 |

## ContentPart

消息内容段；文本与外部产物二选一，由 kind 指定

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `kind` | string | 是 | 内容种类；枚举：text, artifact |
| `text` | string | 否 | 原始文本，不截断 |
| `artifact` | ArtifactRef | 否 |  |

## Message

模型可见消息；工具调用详情通过 ToolCall 单独记录

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `role` | string | 是 | 消息来源；枚举：system, user, assistant, tool |
| `content` | array<ContentPart> | 是 | 有序内容段 |
| `tool_call_id` | string | 否 | 不透明身份标识；不得用其他实体的 ID 代填 |

## TypedConfig

扩展信封；必须由 SchemaRegistry 绑定 schema_ref 对应的已注册 schema 后校验 data，禁止仅做信封校验

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `schema_ref` | string | 是 | 精确 schema URI，未知 URI 必须拒绝 |
| `data` | 按 discriminator 选择 | 是 | 未绑定前禁止执行验证通过；SchemaRegistry 按已注册版本化 schema 替换为 oneOf/$ref |

## DatasetRef

数据来源身份

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `revision` | string | 是 | 不可变数据版本或内容 digest |
| `split` | string | 是 | 数据划分名 |
| `subset` | string | 是 | 数据子集名；无子集为空 |

## RuntimeSpec

镜像候选；按 run、task、package 优先级选择，字段始终叫 image

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `image` | string | 是 | 非空 OCI 镜像引用；执行计划必须锁定内容 digest |

## TaskSpec

唯一公开任务定义；可传给 Environment/Agent；不含评分材料及引用；runtime 仅声明样本运行资源候选，不选择后端

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `schema_version` | 按 discriminator 选择 | 是 | 契约版本；固定值：vnext.3 |
| `task_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `dataset` | DatasetRef | 是 |  |
| `sample_id` | string | 是 | 数据集内稳定样本 ID |
| `input` | TypedConfig | 是 | 公开业务字段，必须由 dataset package 声明其 schema |
| `input_digest` | string | 是 | 内容 SHA-256；必须对应实际字节 |
| `runtime` | RuntimeSpec | 否 | 可选样本镜像候选，不是最终执行镜像 |

## ComponentSpec

选择一个实现及其 schema 验证后的参数

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `implementation` | ComponentRef | 是 |  |
| `config` | TypedConfig | 是 |  |

## ToolBinding

用户明确授权的一项模型可见工具；包含 SDK 原生工具，不自动合并默认工具

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `name` | string | 是 | 本次会话唯一工具名；绑定 SDK 工具时使用适配器声明的名字 |
| `implementation` | ComponentRef | 是 | 实际执行实现；原生状态工具也发布为有版本的包装组件 |
| `config` | TypedConfig | 是 |  |

## ToolInterface

工具实现可使用的一种标准接入接口；Agent 按自己声明的优先顺序选择首个兼容项

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `interface` | string | 是 | 版本化接口标识，例如 mcp.v1 或 openhands_native.v1 |
| `adapter` | ComponentRef | 是 | 把该工具接口接入 Agent 的受管适配器 |
| `execution_scope` | string | 是 | 工具由哪个受控执行入口承接；不是底层访问授权；枚举：agent_state, sandbox, external_service |
| `required_capabilities` | array<string> | 是 | 该接入方式需要的 Worker 能力；不会授予文件、网络或宿主权限 |

## ResolvedToolBinding

预检后固定的实际工具绑定；Worker 初始化后必须核验 SDK 实际工具表与它一致

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `name` | string | 是 | 本次会话唯一模型可见工具名 |
| `implementation` | ResolvedComponent | 是 |  |
| `adapter` | ResolvedComponent | 是 |  |
| `config` | TypedConfig | 是 |  |
| `interface` | string | 是 | Agent 与工具共同支持并已锁定的版本化接入接口 |
| `execution_scope` | string | 是 | 工具由哪个受控执行入口承接；不是底层访问授权；枚举：agent_state, sandbox, external_service |
| `required_capabilities` | array<string> | 是 | 仅用于 Worker 兼容性匹配；不会授予文件、网络或宿主权限 |

## Resources

每个 episode 的资源上限；Worker 还需套管理员上限

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `cpu_cores` | number | 是 | CPU 核数；提交默认值：1 |
| `memory_bytes` | integer | 是 | 内存字节数；最小值：1；提交默认值：1073741824 |
| `process_limit` | integer | 是 | 最大进程数；最小值：1；提交默认值：64 |
| `disk_bytes` | integer | 是 | 工作区字节预算；最小值：1；提交默认值：1073741824 |

## BackendSpec

由用户明确选择的任务后端；不读取数据集名称做选择

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `implementation` | ComponentRef | 是 |  |
| `config` | TypedConfig | 是 |  |
| `resources` | Resources | 是 | ；提交默认值：{} |

## GenerationConfig

规范模型生成参数；额外 provider 参数须另注册 schema

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `temperature` | number | 是 | 采样温度；最小值：0；提交默认值：0 |
| `top_p` | number | 是 | 核采样概率；提交默认值：1 |
| `max_output_tokens` | integer | 是 | 单次生成最大 token 数；最小值：1；提交默认值：1024 |
| `stop` | array<string> | 是 | 停止字符串列表；提交默认值：[] |

## ModelSpec

统一模型端点；训练时可指向 Bridge ModelGateway

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `endpoint` | string | 是 | 端点地址，不包含密钥 |
| `credential_ref` | string | 是 | 凭据引用；允许空表示不需要凭据；提交默认值："" |
| `model_id` | string | 是 | 模型身份 |
| `generation` | GenerationConfig | 是 | ；提交默认值：{} |
| `source` | string | 是 | 模型来源，模拟必须显式声明；枚举：real, simulated |
| `max_transport_retries` | integer | 是 | 仅尚未产生可用生成结果时的网络重试上限；最小值：0；提交默认值：0 |

## Limits

预算命名不可混用；完整 RunSpec 必须包含所有预算值

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `total_timeout_ms` | integer | 是 | 从 Server 接收起的总预算，包含排队及评分；最小值：1 |
| `score_reserve_ms` | integer | 是 | 为结果收集、冻结及最终评分预留预算；三者共享总截止时间；最小值：0 |
| `max_generations` | integer | 是 | 成功或部分完成的模型生成调用预算；最小值：1 |
| `max_tool_calls` | integer | 是 | 接受执行的工具调用预算；最小值：0 |
| `max_environment_steps` | integer | 是 | 环境转移次数预算；无状态任务可为 0；最小值：0 |
| `max_total_output_tokens` | integer | 是 | episode 累计生成 token 上限；最小值：1 |

## TrainingSpec

训练接入规则，不是数据集参数

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `parallel_mode` | string | 是 | 提交/消费策略；枚举：sync, one_step_off_policy, fully_async |
| `require_token_trace` | boolean | 是 | 为 true 时缺少真实 token/logprob 必须拒绝进入训练 |
| `version_policy` | string | 是 | 模型版本规则；枚举：fixed_episode, per_generation |
| `requested_policy_version` | string | 是 | 期望版本，fixed_episode 必填；per_generation 可空 |
| `max_policy_lag` | integer | 是 | 允许的版本落后量；需模型注册表提供可比较序号；最小值：0 |

## RetryPolicy

episode 失败重试仅 Server 决定

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `max_attempts` | integer | 是 | 含首次在内的最大 attempt 数；最小值：1；提交默认值：1 |
| `initial_backoff_ms` | integer | 是 | 初始退避毫秒；最小值：1；提交默认值：500 |
| `max_backoff_ms` | integer | 是 | 退避上限毫秒；最小值：1；提交默认值：5000 |

## RunSpec

用户创建作业时提供的完整配置

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `schema_version` | 按 discriminator 选择 | 是 | 契约版本；固定值：vnext.3 |
| `run_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `purpose` | string | 是 | 作业用途；枚举：evaluation, training |
| `environment` | ComponentSpec | 是 |  |
| `agent` | ComponentSpec | 是 | 智能体实现与参数；不提供 Agent 池或 placement；按 agent 角色校验接口和配置 |
| `tools` | array<ToolBinding> | 是 | 完整显式工具绑定；空数组要求实际无模型可见工具，不兼容的 Agent 必须拒绝 |
| `scorer` | ComponentSpec | 是 | 本次运行唯一评分器；评测与后训练复用它产生的同一个 ScoreResult |
| `backend` | BackendSpec | 是 |  |
| `model` | ModelSpec | 是 |  |
| `limits` | Limits | 是 |  |
| `retry` | RetryPolicy | 是 | ；提交默认值：{} |
| `trajectory_retention_days` | integer | 是 | Server 保存权威轨迹的天数；只影响保留期，不改变记录内容；最小值：1；提交默认值：7 |
| `training` | TrainingSpec | 否 | 仅 purpose=training 时必填；purpose=evaluation 时禁止出现 |
| `runtime` | RuntimeSpec | 否 | 用户显式镜像选择，优先于 task 和 package |

## EpisodeRequest

Bridge -> Server；不接受客户端指定 attempt 或 lease

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `request_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `run_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `episode_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `task` | TaskSpec | 是 |  |
| `private_data` | TypedConfig | 否 | 可选评分依据，随受控请求配对提交；大型测试使用内部 ArtifactRef；不得交给 Agent/Environment 或公开轨迹 |
| `seed` | integer | 是 | 本次 episode 种子；最小值：0 |
| `sample_index` | integer | 是 | 批次内位置，从 0 开始；最小值：0 |
| `batch_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |

## BatchRequest

提交一组 episode；batch ID 必须和各项一致

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `run_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `batch_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `episodes` | array<EpisodeRequest> | 是 | 非空任务列表，数量受服务器限制 |

## BatchReceipt

提交确认不是执行成功

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `batch_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `episode_ids` | array<string> | 是 | 已接纳的 episode ID |

## Lease

Server 颁发，Worker 核验；不能由用户任务参数携带

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `lease_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `epoch` | integer | 是 | 服务实例任期；最小值：0 |
| `expires_at_ms` | integer | 是 | UTC Unix 毫秒；跨机器用于记录，超时执行使用本机单调时钟；最小值：0 |
| `token` | string | 是 | 租约授权值；不得写入用户轨迹 |

## ExecutionPlan

由请求与配置转换而来，不嵌套 EpisodeRequest/RunSpec；每项执行配置仅一处生效

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `run_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `episode_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `task` | TaskSpec | 是 |  |
| `private_data` | TypedConfig | 否 | 可选评分依据，随受控请求配对提交；大型测试使用内部 ArtifactRef；不得交给 Agent/Environment 或公开轨迹 |
| `seed` | integer | 是 | 本次 episode 种子；最小值：0 |
| `purpose` | string | 是 | 作业用途；枚举：evaluation, training |
| `model` | ModelSpec | 是 |  |
| `limits` | Limits | 是 |  |
| `training` | TrainingSpec | 否 | 仅 purpose=training 时必填；purpose=evaluation 时禁止出现 |
| `environment` | object | 是 | 选择一个实现及其 schema 验证后的参数 |
| `environment.implementation` | ResolvedComponent | 是 |  |
| `environment.config` | TypedConfig | 是 |  |
| `agent` | object | 是 | 选择一个实现及其 schema 验证后的参数 |
| `agent.implementation` | ResolvedComponent | 是 |  |
| `agent.config` | TypedConfig | 是 |  |
| `backend` | object | 是 | 由用户明确选择的任务后端；不读取数据集名称做选择 |
| `backend.implementation` | ResolvedComponent | 是 |  |
| `backend.config` | TypedConfig | 是 |  |
| `backend.resources` | Resources | 是 | ；提交默认值：{} |
| `scorer` | object | 是 | 选择一个实现及其 schema 验证后的参数 |
| `scorer.implementation` | ResolvedComponent | 是 |  |
| `scorer.config` | TypedConfig | 是 |  |
| `attempt_id` | integer | 是 | Server 生成；正常首次执行为 1，只有基础设施重试才递增，且不重选配置；最小值：1 |
| `tools` | array<ResolvedToolBinding> | 是 | 唯一生效工具表；保留 RunSpec.tools 的字段名，锁定版本和执行路由 |
| `required_capabilities` | array<string> | 是 | 完整运行能力需求，仅供 Worker 调度与兼容性核验；不是访问授权 |
| `internet_access` | boolean | 是 | Environment 是否需要访问公共互联网；Server 锁定，Backend 受控落实 |
| `runtime` | object | 否 | 本条计划唯一生效的容器资源，不是待选择的候选 |
| `runtime.image` | string | 是 | 唯一生效镜像；task.runtime 只是原始任务候选，Worker 不再消费它 |
| `runtime.image_source` | string | 是 | 最终镜像的来源，用于审计；枚举：run, task, package |
| `plan_digest` | string | 是 | 内容 SHA-256；必须对应实际字节 |
| `deadline_at_ms` | integer | 是 | 首次接纳时间加 limits.total_timeout_ms，重试不续期；Worker 唯一总截止时间；最小值：0 |

## DispatchRequest

Server -> Worker；派生预算字段只能收紧 ExecutionPlan 中的限制

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `plan` | ExecutionPlan | 是 |  |
| `lease` | Lease | 是 |  |
| `remaining_timeout_ms` | integer | 是 | 派发时距离 plan.deadline_at_ms 的剩余上限；Worker 转为本机单调时钟；最小值：1 |
| `consumed_usage` | Usage | 是 | 此前 attempt 已确认消耗的 episode 累计用量；首次派发全为 0 |

## SessionRef

资源 session 身份；不等于 task_id 或 sample_id

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `session_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `backend` | ResolvedComponent | 是 |  |

## Observation

模型可见观测；文本、多模态 artifact 与结构化数据可同时存在

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `content` | array<ContentPart> | 是 | 有序原始可见内容 |
| `data` | TypedConfig | 否 | 结构化可见状态，不含评分私有材料 |

## Transition

一次环境动作产生的状态转移，不等于模型调用或工具调用

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `observation` | Observation | 是 |  |
| `terminated` | boolean | 是 | 环境到达自然终态 |
| `episode_truncated` | boolean | 是 | 因外部预算等限制截断 |
| `environment_reward` | ['number', 'null'] | 否 | 可选环境原生信号，不自动成为训练 reward |

## Outcome

统一执行结果；Agent 提交与环境收集共用，state 仅由 Environment 填写

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `final_answer` | array<ContentPart> | 是 | 可以为空；不要求所有任务产生文本答案 |
| `artifacts` | array<ArtifactRef> | 是 | 冻结的文件、补丁或其他产物 |
| `state` | TypedConfig | 否 | 环境提供的评分状态快照，不发送给 Agent |
| `termination_reason` | string | 是 | 交互为何结束；in_progress 只用于过程快照，系统取消或失败由 EpisodeResult.execution_status 与 ErrorRecord 表达；枚举：in_progress, final_answer, environment_terminal, budget_exhausted |

## ScoreInput

Worker -> Python Scorer；只在 episode 结束后创建，不能发送到 Agent

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `task` | TaskSpec | 是 |  |
| `outcome` | Outcome | 是 |  |
| `trajectory_ref` | ArtifactRef | 是 |  |
| `private_data` | TypedConfig | 否 |  |

## Metric

一条 episode 的单个具名指标

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `name` | string | 是 | 如 accuracy/resolved/tests_passed |
| `value` | number | 是 | 有限数，拒绝 NaN/Infinity |
| `unit` | string | 是 | 如 ratio/count |
| `direction` | string | 是 | 优化方向；枚举：higher, lower, none |

## ScoreResult

统一评分结果；Python Scorer 返回同名 SDK 类的业务字段，Rust Worker 补全系统字段后才满足本传输 schema

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `status` | string | 是 | 评分是否成功完成；系统维护，评分器不得填写；SDK 构造时为 None，最终传输按必填/可选规则校验；枚举：ok, error |
| `success` | ['boolean', 'null'] | 是 | 任务是否成功；无法确定时为 null |
| `metrics` | array<Metric> | 是 | 命名指标列表，名称唯一 |
| `reward` | ['number', 'null'] | 是 | 本条 episode 的唯一奖励；评分成功时必须为有限数，评测展示与后训练复用该值 |
| `evidence` | array<ArtifactRef> | 是 | 评分证据 |
| `scorer` | ResolvedComponent | 是 | ；系统维护，评分器不得填写；SDK 构造时为 None，最终传输按必填/可选规则校验 |
| `error` | ErrorRecord | 否 | ；系统维护，评分器不得填写；SDK 构造时为 None，最终传输按必填/可选规则校验 |

## Usage

本 episode 截至当前 attempt 的累计用量；不同操作独立计数，基础设施重试不清零

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `generation_count` | integer | 是 | 模型生成调用次数；最小值：0 |
| `tool_call_count` | integer | 是 | 工具执行次数；最小值：0 |
| `environment_step_count` | integer | 是 | 环境转移次数；最小值：0 |
| `output_token_count` | integer | 是 | 实际生成 token 数；最小值：0 |

## EpisodeResult

Worker 产生候选结果，Server 校验租约后形成唯一权威终态

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `run_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `episode_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `attempt_id` | integer | 是 | 有效 attempt；最小值：1 |
| `task_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `execution_status` | string | 是 | 执行是否完成；枚举：completed, failed, timeout, cancelled |
| `score` | ScoreResult | 否 | episode 结束后产生的唯一正式评分结果 |
| `outcome` | Outcome | 否 |  |
| `trajectory_ref` | ArtifactRef | 否 |  |
| `usage` | Usage | 是 |  |
| `started_at_ms` | integer | 否 | UTC Unix 毫秒；跨机器用于记录，超时执行使用本机单调时钟；最小值：0 |
| `finished_at_ms` | integer | 是 | UTC Unix 毫秒；跨机器用于记录，超时执行使用本机单调时钟；最小值：0 |
| `error` | ErrorRecord | 否 |  |
| `cleanup_status` | string | 是 | 清理可独立重试，不改变任务评分；枚举：completed, pending, failed |

## GenerationEvent

一次模型调用的原始记录；逐调用保留版本和 token 对齐

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `generation_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `model_id` | string | 是 | 实际模型 |
| `policy_version` | string | 否 | 实际策略版本 |
| `parameter_version` | integer | 否 | 实际参数版本序号；最小值：0 |
| `tokenizer` | ResolvedComponent | 否 |  |
| `source` | string | 是 | 真实或模拟；枚举：real, simulated |
| `messages` | array<Message> | 是 | 该次真实输入消息 |
| `response` | array<ContentPart> | 是 | 该次原始输出 |
| `output_token_count` | integer | 是 | 本次实际生成 token 数；预算统计的唯一来源；最小值：0 |
| `input_token_ids` | array<integer> | 否 | 真实输入 token 序列 |
| `output_token_ids` | array<integer> | 否 | 真实生成 token 序列；存在时长度必须等于 output_token_count |
| `output_logprobs` | array<number> | 否 | 长度必须等于 output_token_ids |
| `loss_mask` | array<integer> | 否 | 与生成 token 一一对应 |
| `finish_reason` | string | 是 | 模型停止原因；枚举：stop, tool_calls, length, cancelled, error |
| `duration_ms` | integer | 是 | 模型调用耗时；最小值：0 |

## ToolCall

规范工具调用；动态 arguments 使用工具声明的 schema 递归验证

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `tool_call_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `generation_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `implementation` | ResolvedComponent | 是 |  |
| `name` | string | 是 | ExecutionPlan.tools 中的唯一工具名；implementation 必须与该项一致 |
| `arguments` | TypedConfig | 是 |  |
| `timeout_ms` | integer | 是 | 允许执行时间；最小值：1 |

## ToolResult

工具结果不等于 episode 结果

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `tool_call_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `status` | string | 是 | 工具状态；枚举：ok, error, timeout, cancelled |
| `content` | array<ContentPart> | 是 | 返回给 Agent 的内容 |
| `output_truncated` | boolean | 是 | 仅标记工具返回预览是否截断 |
| `raw_output_ref` | ArtifactRef | 否 |  |
| `error` | ErrorRecord | 否 |  |

## EnvironmentTransition

动作前事实与 Environment.step 返回值；动作后字段只在 Transition 中定义一次

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `environment_step_index` | integer | 是 | 从 0 开始；最小值：0 |
| `observation_before` | Observation | 是 | 动作前观测 |
| `action` | TypedConfig | 是 |  |
| `transition` | Transition | 是 | Environment.step 返回值的独立副本 |

## StateEvent

基础生命周期事件

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `phase` | string | 是 | 执行阶段；枚举：preparing, running, scoring, finalizing, cleaning |
| `session` | SessionRef | 否 |  |

## TrajectoryEvent

所有数据集共享事件信封，事件 payload 按 kind 绑定

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `schema_version` | 按 discriminator 选择 | 是 | 契约版本；固定值：vnext.3 |
| `event_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `run_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `episode_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `attempt_id` | integer | 是 | attempt 身份；最小值：1 |
| `task_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `sequence` | integer | 是 | 由 Worker 事件收集器分配的连续序号，从 0 开始；最小值：0 |
| `occurred_at_ms` | integer | 是 | UTC Unix 毫秒；跨机器用于记录，超时执行使用本机单调时钟；最小值：0 |
| `parent_event_id` | string | 否 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `kind` | string | 是 | 事件类型；枚举：state, observation, generation, tool_call, tool_result, environment_transition, score, error, terminal |
| `payload` | 按 discriminator 选择 | 是 | 由 kind 决定的确定类型 |

## TrajectoryManifest

attempt 的轨迹索引；评分前快照与最终封存使用同一结构，终态事件不反向包含此索引

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `schema_version` | 按 discriminator 选择 | 是 | 契约版本；固定值：vnext.3 |
| `run_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `episode_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `attempt_id` | integer | 是 | attempt 身份；最小值：1 |
| `task_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `event_segments` | array<ArtifactRef> | 是 | 按 sequence 有序的 JSONL 事件分片 |
| `event_count` | integer | 是 | 事件总数；最小值：0 |
| `trajectory_status` | string | 是 | 清单所处阶段及最终记录完整性；不表示任务执行成功或失败；枚举：scoring_checkpoint, final_complete, final_partial |
| `created_at_ms` | integer | 是 | UTC Unix 毫秒；跨机器用于记录，超时执行使用本机单调时钟；最小值：0 |

## TerminalEvent

轨迹内终态摘要，避免与 EpisodeResult.trajectory_ref 形成 digest 环

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `execution_status` | string | 是 | 终态；枚举：completed, failed, timeout, cancelled |
| `usage` | Usage | 是 |  |
| `error` | ErrorRecord | 否 |  |

## ToolSpec

用户工具发布声明

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `implementation` | ComponentRef | 是 |  |
| `entrypoint` | string | 是 | Python 函数或 ToolExecutor 的 module:symbol 入口 |
| `config_schema` | string | 是 | ToolExecutor 构造配置的精确 schema 标识 |
| `description` | string | 是 | 给 Agent 的说明 |
| `interfaces` | array<ToolInterface> | 是 | 至少一种标准接入方式，接口名在本工具内唯一 |
| `input_schema` | ArtifactRef | 是 |  |
| `output_schema` | ArtifactRef | 是 |  |
| `side_effect` | string | 是 | 副作用与重试依据；枚举：read_only, idempotent, non_idempotent |

## AgentManifest

自定义 Agent 的发布声明；按接口复用工具，不逐个登记所有工具实现

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `implementation` | ComponentRef | 是 |  |
| `entrypoint` | string | 是 | AgentRunner 的 module:Class 入口 |
| `config_schema` | string | 是 | AgentRunner 构造配置的精确 schema 标识 |
| `supported_interfaces` | array<string> | 是 | 至少一项且不重复 |
| `required_tool_names` | array<string> | 是 | 允许为空；空数组表示无必需工具 |

## EntryPoints

环境包 Python 入口，均为 module:Class

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `dataset_adapter` | string | 是 | 原始行 -> PreparedSample |
| `environment` | string | 是 | Environment 实现 |
| `scorer` | string | 是 | 唯一单条 episode Scorer 实现；评测与后训练共用 |

## PackageManifest

统一环境包，不绑定 agent 或 backend

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `runtime` | RuntimeSpec | 否 | 数据集包默认镜像候选 |
| `id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `version` | string | 是 | 精确版本 |
| `entrypoints` | EntryPoints | 是 |  |
| `task_schema` | string | 是 | 任务业务字段 schema 标识 |
| `private_schema` | string | 否 | 私有评分字段 schema 标识 |
| `config_schemas` | object | 是 | 同一包内两个运行角色各自接受的配置 schema；角色名就是唯一索引 |
| `config_schemas.environment` | string | 是 | Environment.config 接受的 schema 标识 |
| `config_schemas.scorer` | string | 是 | Scorer.config 接受的 schema 标识 |
| `internet_access` | boolean | 是 | Environment 是否需要公共互联网；数据行、RunSpec、Agent 和 Tool 不得覆盖 |
| `required_capabilities` | array<string> | 是 | 部署所需的 Worker 功能；不写 Docker/OpenHands 名称，也不授予访问权限 |
| `artifacts` | array<ArtifactRef> | 是 | 代码 wheel 和显式声明的运行文件；镜像只由 runtime.image 引用 |
| `provided_tools` | array<ToolSpec> | 是 | 包可提供的工具声明；本次启用项只由 RunSpec.tools 决定 |
| `schemas` | array<ArtifactRef> | 是 | 包发布的带 $id 的 JSON Schema，注册前校验 digest |

## WorkerRegistration

Worker 声明能力，供 Server 匹配

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `worker_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `endpoint` | string | 是 | Worker 地址 |
| `capacity` | integer | 是 | 可同时承载的 episode 槽位总数；不同于 CPU/内存/存储 resource_capacity；最小值：1 |
| `capabilities` | array<string> | 是 | 运行能力 |
| `components` | array<ResolvedComponent> | 是 | 已安装可执行组件；包括后端，不另设后端清单 |
| `resource_capacity` | Resources | 是 | Worker 可管理的 CPU/内存/存储总量；BackendSpec.resources 是单个 episode 的资源申请 |

## Heartbeat

控制面健康与容量快照

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `worker_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `timestamp_ms` | integer | 是 | UTC Unix 毫秒；跨机器用于记录，超时执行使用本机单调时钟；最小值：0 |
| `active_leases` | array<string> | 是 | 活跃 lease ID |
| `available_slots` | integer | 是 | 当前可用 episode 槽位；由 WorkerRegistration.capacity 减去已占用槽位得到，不是第二份总容量；最小值：0 |
| `draining` | boolean | 是 | 不再接受新任务 |

## CancelRequest

Server 或客户端取消请求；取消意图具有幂等性

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `request_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `episode_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `reason` | string | 是 | 取消原因 |

## ResultReport

Worker 重传结果；Server 按 attempt+lease 去重和排除旧结果

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `lease` | Lease | 是 |  |
| `result` | EpisodeResult | 是 |  |

## Ack

提交/上报/取消的确认

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `accepted` | boolean | 是 | 是否接受 |
| `code` | string | 是 | OK/STALE_ATTEMPT/CONFLICT 等稳定码 |
| `message` | string | 是 | 说明 |

## CursorRequest

查询或订阅结果/事件

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `run_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `episode_id` | string | 否 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `after_sequence` | integer | 否 | 已接收的最后序号；最小值：0 |
| `limit` | integer | 是 | 返回上限；最小值：1 |

## ExecRequest

任务后端执行命令，不包含数据集字段

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `session_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `argv` | array<string> | 是 | 参数列表 |
| `cwd` | string | 是 | 沙箱内路径 |
| `environment_variables` | object | 是 | 命令环境变量映射；密钥由受控注入，不入轨迹 |
| `timeout_ms` | integer | 是 | 命令预算；最小值：1 |

## ExecResult

底层命令结果

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `exit_code` | integer | 是 | 进程退出码；信号退出另记 error；最小值：0 |
| `stdout_ref` | ArtifactRef | 是 |  |
| `stderr_ref` | ArtifactRef | 是 |  |
| `duration_ms` | integer | 是 | 耗时；最小值：0 |
| `error` | ErrorRecord | 否 |  |

## FrameworkSample

Bridge 将统一轨迹映射为训练框架输入的规范中间结构

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `episode_id` | string | 是 | 不透明身份标识；不得用其他实体的 ID 代填 |
| `attempt_id` | integer | 是 | attempt；最小值：1 |
| `generation_event_ids` | array<string> | 是 | 使用的模型生成事件，保持顺序 |
| `reward` | number | 是 | 从唯一 ScoreResult.reward 复制的训练标量，不重新计算 |
| `policy_versions` | array<string> | 是 | 与 generation_event_ids 一一对应 |
| `trajectory_ref` | ArtifactRef | 是 |  |

## HarnessRequest

Scorer -> Worker 提供的 harness 执行能力

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `outcome` | Outcome | 是 | 与 ScoreInput.outcome 相同的完整评分快照，不另命名或只传 artifacts |
| `private_data` | TypedConfig | 是 | 原样传入；唯一评测配置在 data.evaluation_plan，不新增顶层副本 |
| `remaining_timeout_ms` | integer | 是 | 剩余预算；最小值：1 |

## HarnessResult

harness 结果，候选失败和 harness 错误不同

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `status` | string | 是 | 评测程序是否正常完成；枚举：ok, error |
| `success` | ['boolean', 'null'] | 是 | 正常完成时必须为布尔值 |
| `tests_run` | integer | 是 | 实际测试数；最小值：0 |
| `tests_passed` | integer | 是 | 通过数量；最小值：0 |
| `report_ref` | ArtifactRef | 是 |  |
| `metrics` | array<Metric> | 否 | 可选的详细指标 |
| `error` | ErrorRecord | 否 |  |

## EmptyConfig

无额外参数

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|

## PlainAgentConfig

轻量智能体配置

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `history_policy` | string | 是 | 模型上下文策略；枚举：full, last_generation；提交默认值："full" |
| `system_prompt` | string | 是 | 智能体系统提示词；空表示不额外添加；提交默认值："" |

## OpenHandsAgentConfig

OpenHands adapter 的明确可调参数

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `system_prompt` | string | 是 | 用户附加的系统提示；提交默认值："" |

## ProcessBackendConfig

进程任务后端；无容器镜像

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `runtime_profile` | string | 是 | 管理员预注册的本机依赖配置名；不得改变平台安全底线或计划中的 internet_access |

## ContainerBackendConfig

Docker/Podman 无额外用户参数；引擎连接属于 Worker 部署配置，镜像选择位于 runtime.image

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|

## ToolConfig

公共命令/文件工具参数

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `timeout_ms` | integer | 是 | 工具执行预算，受 episode 总 deadline 限制；最小值：1；提交默认值：30000 |
| `max_preview_bytes` | integer | 是 | 返回消息预览长度；原始输出另存 artifact；最小值：1；提交默认值：8192 |

## TerminalArgs

终端工具参数

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `argv` | array<string> | 是 | 显式 argv；需要 shell 时明确给出 shell -c 参数 |
| `cwd` | string | 是 | 沙箱内目录 |

## ReadFileArgs

读取工具参数

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `path` | string | 是 | 工作区内路径 |

## WriteFileArgs

写入工具参数

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `path` | string | 是 | 工作区内路径 |
| `content` | string | 是 | 完整写入内容 |

## AnswerAction

问答环境动作

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `answer` | string | 是 | 完整回答 |

## EvaluationPlan

包拥有的评测执行计划；不能给 Agent 读取

| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |
|---|---|---|---|
| `harness` | ResolvedComponent | 是 | 唯一 harness 选择，固定精确版本和 digest |
| `setup_argv` | array<array> | 是 | 准备步骤 |
| `test_argv` | array<string> | 是 | 测试入口命令 |
| `cwd` | string | 是 | 评分沙箱目录 |
| `timeout_ms` | integer | 是 | 评测预算；最小值：1 |

