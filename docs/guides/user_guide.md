# UEnv 用户指南

本指南按运行任务、自定义 Agent 和工具组织。产品 CLI/客户端仍是目标接口；仓库中的 reference 是可执行的本地参考，不是已部署服务。示例模型地址、镜像和任务资源需要替换为真实配置。

新增数据集的文件结构、三个入口类、数据格式和原始字段映射统一见[数据集包模板](dataset_package_template.md)，不在本指南另维护一份。

## 1. 准备任务数据

使用已有数据集时，提供原始数据，或选择 Hub 中固定 revision 的数据及样本范围。原始数据经该包 DatasetAdapter 转换；已经标准化的 Hub 样本直接校验读取。两者都进入相同准备入口，用户不直接与 Worker 交换数据。

本地原始数据的目标命令示例：

```text
uenv dataset prepare --package datasets/gsm8k@1.0.0 --input raw.jsonl --output tasks.jsonl
```

输出的 tasks.jsonl 是 UTF-8 JSONL，每行包含 task 和可选 private_data。含答案或隐藏测试的文件按评分材料管理，不能直接作为 Agent 输入。格式与 Hub 保存内容见[数据集模板第 3.3 节](dataset_package_template.md#33-完整数据集和-ground-truth-如何存入-hub)。Hub 数据选择 API 的完整请求结构尚待确定，当前不提供未经定义的 CLI 参数示例。

## 2. 填写运行配置

本节是 run.yaml 的统一使用说明。所有数据集使用同一个模板；不维护 GSM8K 专用、SWE 专用等不同配置协议。可从[精简 GSM8K 示例](../../reference/runs/gsm8k.yaml)或[九份运行示例](../../reference/runs)开始。示例的组件选择和预算值不同，字段的含义不随数据集改变。

### 2.1 文件和 SDK 怎样交给 Bridge

run.yaml 保存在调用方的训练、评测或实验项目中，不属于数据集包，也不发布到 Hub。它是可选的文件输入方式；Python 调用方可以直接提供等价配置，由 SDK 构造 RunSpec。SDK 是 Bridge 的编程入口，不绕过 Bridge 另走执行链。

```mermaid
flowchart LR
  YAML[run.yaml] --> READ[CLI 调用 Bridge 读取 YAML]
  READ --> NORMALIZE[Bridge 共用配置函数<br/>补齐声明默认值并校验]
  SDK[SDK 提供配置对象] --> NORMALIZE
  NORMALIZE --> BATCH[Bridge 组装 BatchRequest<br/>一份 run_spec 与多条 episodes]
  TASK[已准备的任务与可选私有材料] --> BATCH
  BATCH --> SERVER[Server 校验并保存配置与任务<br/>为每条任务生成 ExecutionPlan]
  SERVER --> WORKER[派发给 Worker 执行]
```

Bridge 只调用一次 submit_batch：BatchRequest.run_spec 保存这一批共用的完整 RunSpec，episodes 保存一条或多条 EpisodeRequest。单条任务也使用同一接口，只含一个成员；没有前置的配置注册请求。

同一批次只允许一套运行配置。EpisodeRequest 不携带 RunSpec、run_id 或 Agent/模型/预算覆盖；它的运行身份来自 BatchRequest.run_spec.run_id。需要不同执行配置时分别提交批次，并使用不同 run_id。任务自己的数据、种子和样本身份仍可不同。

Server 在接收批次时保存配置与任务；后续批次沿用同一 run_id 时，补齐后的完整 RunSpec 必须一致，否则返回 RUN_CONFIG_CONFLICT，不覆盖已保存配置。Server 比较旧配置是为了拒绝冲突，生成计划仍只读取本次 BatchRequest.run_spec。配置变更使用新的 run_id。

Server 不读取 YAML 文件，Worker 接收包含 ExecutionPlan 的派发请求。CLI 和环境变量不能另行覆盖已提交配置。run_id 由调用方或 SDK 在首次提交前确定并复用，无需先请求 Server 创建它；九份设计示例显式给出稳定值，便于对应生成夹具。

可查看[GSM8K 批次请求示例](../../reference/generated/episodes/gsm8k/batch_request.json)，其中 run_spec 和 episodes 就是一次提交的完整内容。以上是统一目标接口；当前本地已有批次 schema、九份批次示例和 Rust 批次校验，真正的 Bridge/RPC、数据库事务与并发幂等仍待接入。

### 2.2 每组字段负责什么

| 字段 | 谁填写 | 谁读取及具体作用 |
|---|---|---|
| run_id | 调用方或 SDK 在首次提交前确定；同一配置跨批次复用 | Server 关联配置与结果，Bridge 按它提交和查询 |
| purpose | 运行用户或训练框架 | Bridge/调用方选择评测汇总或训练消费；不改变评分 |
| environment、scorer | 运行用户 | Server 分别锁定角色；Worker 加载环境规则和单条评分实现 |
| agent | 运行用户 | Server 校验接口，Worker 启动所选智能体 |
| backend | 运行用户 | Server 校验资源与兼容性；Worker 选择执行后端，按 resources 限制资源 |
| model | 运行用户或训练框架 | Worker 模型入口使用端点、模型及 generation 参数；训练端点可指向 Bridge ModelGateway |
| tools | 运行用户，显式给出完整列表 | Server 解析接口；Worker 只开放列表中的工具；[] 表示无工具 |
| limits | 运行用户 | Server 固定总截止时间，Worker 限制模型调用、工具调用、环境动作和累计输出 |
| retry | 运行用户，允许省略 | Server 决定基础设施失败是否重试及退避；不用于模型网络请求重试 |
| training | 训练框架或运行用户，仅训练时提供 | Bridge/模型入口约束版本和训练轨迹；评测时不得填写 |
| runtime.image | 需要覆盖镜像时由运行用户填写 | Server 选择并锁定镜像；Worker 使用最终值；Process 不接受显式镜像覆盖 |
| trajectory_retention_days | 运行用户，允许省略 | 目标 Server/ArtifactStore 控制轨迹保存期；真实保留期服务仍待实现 |

题目、答案、仓库信息和隐藏测试属于任务数据，不写进 run.yaml。Docker 引擎连接地址、宿主路径和隔离规则属于 Worker 部署配置，也不让普通运行用户填写。完整嵌套字段、类型及声明默认值查[生成字段字典](../generated/field_dictionary.md)，不在这里复制第二份字段清单。

### 2.3 哪些参数可以省略

有声明默认值的参数允许省略。当前精简示例省略了默认资源、模型生成参数、重试退避、轨迹保留天数和默认组件参数。Environment、Agent、Scorer、Backend 的实现选择，模型端点和身份，以及 limits 中的预算仍须明确给出；不按数据集名称自动选择它们。

组件 config 省略时按空对象处理，再补该组件参数模型声明的默认值。若组件还有无默认值的必填参数，仍然报错。例如 Process 的 config.runtime_profile 必填，Docker/Podman 没有对应参数。

默认值只在统一契约或组件模型中定义一次。Bridge 只补缺失字段；显式填写的 0、false、空列表不会被默认值替换，类型错误和非法 null 直接报错。补齐后形成完整 RunSpec，Server 校验与 Worker 执行不再补值。--dry-run 应显示补齐后的配置，用户可以确认实际提交内容。默认值规则随对应协议或组件版本维护，不从 Worker 本机配置临时取值。

参考中的默认值用于演示，不代表真实部署的资源和采样推荐。model.source=simulated 明确标记模拟模型；接真实模型时需填写真实端点并声明 real，不能把示例当成真实模型评测证据。

### 2.4 组件特有参数和同义字段

只有组件独有且用户需要调整的行为才能放在 config 中，并受所选组件的 schema 校验。当前九个数据集的 environment.config 和 scorer.config 均无额外参数；新数据集若确有业务参数，在 models.py 中定义并登记相应角色配置。

总超时只用 limits.total_timeout_ms，模型调用上限只用 limits.max_generations，采样温度只用 model.generation.temperature。组件 config 不能再定义另一个同义入口，schema 不接受任意扩展字典。未知字段和已登记别名可自动拒绝；任意自定义字段的语义仍需作者和审查者判断。

不同作用范围的限制分别保留：工具 timeout_ms 限制一次工具调用，limits.total_timeout_ms 限制整个 episode；model.generation.max_output_tokens 限制一次生成，limits.max_total_output_tokens 限制累计输出。前者都不能放大后者。

PlainAgent 的 history_policy 用于选择实际历史策略；OpenHands 的固定 SDK 历史处理不要求用户填写。OpenHands 不公开 sdk_iteration_limit，模型调用预算统一在 limits；真实接入时须验证 SDK 内部迭代与模型调用的关系，不能直接把两个计数当成同一个数。

### 2.5 更换 Agent、Backend 和镜像

更换组件只改相应 implementation 及该组件确实需要的 config，题目和评分材料保持不变；不通过修改 Adapter 来选择 Agent。工具列表仍由用户明确填写，新 Agent 必需的工具缺失时应报错。

Process 的 runtime_profile 仅表示管理员准备好的本机运行环境；Docker/Podman 不接受该字段，其引擎连接由 Worker 部署时确定。容器镜像仍只使用 runtime.image，来源与优先级统一见[主方案第 7.3 节](../uenv_design.md#73-镜像来源与唯一解析规则)。

参数独立选择不保证任意组合可用。系统按所选组件的类型、工具接口、运行依赖和资源能力判断兼容性；不按数据集名称选择默认后端，也不在组合失败时偷偷切换。

## 3. 提交、查看和取消

```text
uenv run --tasks tasks.jsonl --config run.yaml
uenv run status RUN_ID
uenv run cancel RUN_ID
uenv trajectory export --run RUN_ID --sample SAMPLE_ID --format jsonl
```

RUN_ID 使用提交返回的运行标识，SAMPLE_ID 使用样本标识。运行前可用 --dry-run 检查展开结果；CLI 不提供另一套 Agent、模型或预算覆盖参数。

需要程序调用时，客户端通过 submit_batch(tasks, run_spec) 一次提交任务与配置，并提供查询/订阅结果、取消和读取轨迹的接口，签名见[主方案第 2.4 节](../uenv_design.md#24-运行与查询接口)。同一样本可能执行多次，因此结果与轨迹查询返回列表，不能默认只取一项。

## 4. 理解结果

| 看到的结果 | 含义与后续操作 |
|---|---|
| execution_status=completed，score.status=ok | 执行和评分完成；再看 score.success、reward，判断任务表现 |
| score.status=ok，success=false | 评分器正常工作，候选答案或产物未成功；可检查回答和轨迹 |
| score.status=error | 评分程序或评测依赖出错；查 error，不能当作模型答错 |
| 进入评分前执行失败 | 没有 score；先检查执行错误和可用的部分轨迹 |

结果包含最终回答或产物、评分、用量和轨迹引用。评分结构及训练消费规则见[主方案第 8 章](../uenv_design.md#8-评分与轨迹)。用户不需要填写内部调度身份、租约、摘要或事件序号。

## 5. 自定义智能体

设计约束：后续不引入 Agent 池。RunSpec.agent 只指定 implementation 和 config，不提供 pool_id、agent_pool_id 或 placement。Agent 由本次任务的 Worker 管理；用户不注册 Agent 池，也不配置独立 Agent 容量或调度。模型服务端点通过运行配置由 Bridge 传入，Agent 不加载模型权重。

实现异步 `AgentRunner.run(context)`。context 只提供公开 TaskSpec、观测、获准工具描述和 `generate/call_tool/step` 能力；三个操作统一使用 `await`，另有只读 seed。不同 Agent 的模型消息由其自行构造。模型调用必须通过 Worker 的 AgentRuntime/ModelProvider，以便在实际调用前强制预算并保留真实生成信息；Agent 不直连端点。Agent 不接收 private_data，也不能直接写 ScoreResult。

参考 extension_templates.py 中的 PlainAgent 是无工具单轮例。正式 PlainAgent 还提供明确的历史策略与工具循环；OpenHandsAdapter 将 SDK Conversation 事件转换成同样的模型、工具和终态记录。用户可以让数学选择 OpenHands，或让仓库修复选择 PlainAgent+工具，只要能力满足。

**自定义 Agent 不等于自定义工具，也不需要逐个编写工具适配。**AgentManifest 只填写实现入口、配置 schema、有序 supported_interfaces 和 required_tool_names。自己写 Agent 循环时，获取 UEnv 提供的获准工具描述，通过 call_tool 调用；基于已支持框架时，复用 UEnv 的框架接入或该框架的 MCP 支持。只有引入未支持的工具接口或框架专属语义时，才需补一次接口适配；普通工具可复用这份接入。

接口写法可直接查看[PlainAgent 示例](../../reference/extension_templates.py)。该示例只演示一次文本生成；需要工具循环或状态交互时，由 Agent 自己组织调用顺序，仍使用 context 提供的异步操作。框架接入的实际完成情况见[参考实现状态](../development/reference_implementation.md#8-实现状态与待决事项)。

## 6. 自定义工具

用户只写一份 Python 工具：在数据集可选 tools.py 或独立工具包中定义函数、参数/返回类型和说明；发布工具生成或校验 ToolSpec 的 entrypoint、config_schema、input/output schema 和 interfaces。需要状态或资源时实现 ToolExecutor。

运行用户在 RunSpec.tools 选择工具及配置，不填写 adapter。PlanResolver 按 AgentManifest.supported_interfaces 的顺序，从 ToolSpec.interfaces 选择第一个共同接口，再锁定 interface/adapter。UEnv 据此直接注册或包装成 MCP；自定义 Agent 不逐个适配工具。Agent 自带工具也发布为 ToolSpec 并进入同一列表，由 UEnv 限制执行并记录参数和结果。选择工具只开放该工具接口，不会开放任意网络或宿主目录。

代码示例和调用图统一见 [工具设计第 6.4 节](../uenv_design.md#64-工具定义接入与调用)。上述为目标用法；当前参考格式、尚未实现的接口和验收要求集中见 [迁移说明第 11.3 节](../development/source_refactoring_plan.md#113-工具接入与验收)。

有状态或需要文件访问的工具可参考同一文件中的[ReadFileTool](../../reference/extension_templates.py)。工具的 Python 函数包装和 MCP 服务属于待实现接入功能，不能将类型声明通过校验当成外部 Agent 已能调用。

## 7. 查字段与新增数据集

以下目标命令只展开当前角色需要填写的字段：

```text
uenv describe RunSpec
uenv describe PreparedSample
uenv describe my-org/my-dataset@1.0.0:MyInput
uenv package validate ./my_dataset
```

运行用户查看 RunSpec；数据集作者查看 PreparedSample 和自己在 models.py 中定义的业务模型。题目、状态、动作和参考材料属于业务字段；后端、智能体、模型、工具和执行预算使用已有运行字段，不能在业务模型里再定义一遍。

校验器会拒绝已登记的重复控制字段，对疑似同义字段给出提示；程序无法百分之百判断 timeout 和 allowed_time 是否表达相同含义，作者仍需确认业务语义。完整作者步骤见[数据集包模板](dataset_package_template.md)，维护者规则见[字段规范](../development/field_conventions.md)。

普通用户不运行 build_contracts.py、build_examples.py、build_module_map.py 或 validate_design.py。它们是本设计仓库的维护工具，使用方法见[仓库 README](../../README.md#谁需要运行哪些命令)。
