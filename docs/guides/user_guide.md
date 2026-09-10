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

从[公开 GSM8K 运行配置](../../reference/runs/gsm8k.yaml)或[其他数据集示例](../../reference/runs)开始。run.yaml 是唯一用户执行配置入口，也可以由调用方构造等价 RunSpec；用户不编辑生成的 ExecutionPlan。

| 想调整什么 | 填写位置 |
|---|---|
| 环境和评分实现 | environment、scorer |
| 智能体 | agent |
| 执行后端 | backend |
| 模型服务地址和生成参数 | model |
| 本次启用的工具 | tools |
| 模型调用次数、总时限等预算 | limits |
| 评测或训练 | purpose；training 仅在训练时填写 |
| 显式覆盖运行镜像 | runtime.image |
| 轨迹保存天数 | trajectory_retention_days |

各组件的 config 直接填写公开参数，不填写 schema_ref。选择某个组件后，可查询它的参数类型；模型调用次数统一填写 limits.max_generations。单轮问答使用无工具 Agent，设 max_generations 为 1。

更换 Agent 或 Backend 只改这份配置，不改数据集转换和评分代码。Docker/Podman 使用兼容镜像；Process 使用本机 runtime_profile。缺少能力或依赖时系统拒绝该组合，用户需要修正配置或部署依赖。镜像的填写位置、默认值和优先级统一见[主方案第 7.3 节](../uenv_design.md#73-镜像来源与唯一解析规则)。

## 3. 提交、查看和取消

```text
uenv run --tasks tasks.jsonl --config run.yaml
uenv run status RUN_ID
uenv run cancel RUN_ID
uenv trajectory export --run RUN_ID --sample SAMPLE_ID --format jsonl
```

RUN_ID 使用提交返回的运行标识，SAMPLE_ID 使用样本标识。运行前可用 --dry-run 检查展开结果；CLI 不提供另一套 Agent、模型或预算覆盖参数。

需要程序调用时，客户端提供创建 run、提交批次、查询/订阅结果、取消和读取轨迹的接口，签名见[主方案第 2.4 节](../uenv_design.md#24-运行与查询接口)。同一样本可能执行多次，因此结果与轨迹查询返回列表，不能默认只取一项。

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
