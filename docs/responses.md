# 对规划要求的逐条答复

本文回答问题；另附独立的 `uenv_design.md` 描述完整目标系统。所有生产代码仍保持原状，本次完成设计、字段规范和参考模板，不把尚未接入真实服务的示例说成已完成重构。

## 一、六项规划要求

### 1. 当前功能、保留、修改与重写

已经按具体职责列出功能处置清单，见 current_capabilities.md。可以保留的主要是已有通信、注册心跳、并发配额、租约、SQLite/outbox、镜像缓存、容器驱动、官方 harness、OpenHands SDK 接入的有效部分。

需要修改的是参数转换、身份/版本、池管理、Hub 包结构、错误语义和轨迹存储接入。需要重写的是数据集分流的主执行逻辑、Worker 直接调用模型的循环、旧 reward 多入口、完整轨迹模型，以及 Rust 编写的用户评分逻辑。不能按“整个 Server 保留、整个 Worker 重写”这样粗分，因为各层都有可保留的可靠实现。

### 2. 路径统一后的调用流程

目标：Bridge 生成标准请求 -> Rust Server 校验、解析组件、排队、选择 Worker、发租约 -> Rust Worker 准备资源并启动 Python 组件 host -> Environment.reset -> AgentRunner.run -> Environment.finalize -> Rust Worker 冻结产物并在本 attempt 调用一次 Python Scorer -> Rust Worker 校验评分、执行首次清理、写 terminal 并封存轨迹 -> durable outbox 上报 -> Server 确认终态 -> Bridge 转成框架结果。

设计方案分别展开了每层内部调用；Agent 内部的模型决策循环只出现一次。Rust Worker 负责唯一 attempt 状态机、预算、工具准入、进程、资源、评分顺序、轨迹和清理；Python AgentRunner 负责模型与环境交互策略。目标由 Worker 管理 AgentRunner，不引入 Agent 池、池选择字段或独立 Agent 调度；现有 OpenHands 链路仅通过迁移兼容层做对照，旧池化调度完成替换后退役。真实 SDK 和受控工具执行保留。

### 3. 目录、类与文件职责

module_map.md 列出目标 Bridge、Server、Worker、Python runtime、SDK 和 Hub 的逐文件职责，统一采用 domain/application/infrastructure/interfaces。字段/函数/文件统一 snake_case，类/trait 统一 PascalCase；Rust 与 Python 只保留必要语言差异。

模块导出文件不写业务；不使用一个无界 utils 文件收容所有逻辑；不把一个超长函数拆成很多 include 文件来伪装职责清晰。目录按职责分层，跨层依赖和注入位置在方案中规定。

### 4. 用户接口、扩展方式与已支持数据集示例

目标用户接口包括 package init/validate/test/publish、dataset prepare、run、按 run 查询/取消和按 run+sample 导出轨迹，以及对应 Python SDK。普通用户不使用 episode_id、attempt_id 或 lease 操作任务。独立重评分不进入首版公开接口；调用方需要单测评分规则时直接调用自己的 `Scorer.score`。

所有数据集使用同一个包模板。每个数据集必须声明自己的 Adapter、Environment、Scorer 三个职责入口类，分别放在 dataset_adapter.py、environment.py、scorer.py，并绑定本包入口。三个类分别实现 normalize、reset、score；少写代码依靠公共函数、组合和脚手架，方法体可以直接调用共享实现，但不能只写 pass 或用 import 别名代替。辅助类型和工具按需增加。自定义 Agent 实现 AgentRunner.run；自定义 Tool 实现 ToolExecutor.execute。backend、agent、tools 在运行参数中显式配置。产品侧自动构建仍是目标接口，现有九个参考包已提供三个入口类。

本次按同一模板生成九个数据集包示例：GSM8K、PubMedQA、SciTab、OlymMATH、DSCodeBench、SWE Verified/Lite/Pro/Smith。OlymMATH 的语言/难度用参数表示。四个文本 scorer 提供 Python 实现，代码类提供 Worker harness adapter。参考例使用合成数据，代码类缺真实 harness 时明确失败；未把这些接口例包装成已完成生产迁移或官方评分验收。

### 5. 鲁棒性设计与过度设计

不能把“有很多保护代码”等同于过度设计。并发限额、租约、结果持久化、超时取消、子进程回收和轨迹重传都有明确必要性，应保留。

要收敛的是多套终态管理、每一层各自重跑业务、为每个数据集建一条恢复链、缺参数时静默回退，以及同一字段多种名字。可延后的是没有收益证据的自适应预热、强制所有后端快照恢复、本次不需要的多主 Server。设计方案逐机制给了保留/合并/可选/删除结论。

### 6. 字段及嵌套 JSON 统一规范

完整系统协议在 `contracts/proto/uenv/v1/` 定义一次，再生成 Rust/Python 类型、RPC stub、机器可校验的核心 JSON Schema 和核心维护者使用的 field_dictionary.md。普通用户不阅读这份完整字典：数据集作者只看 PreparedSample、本包模型和三个入口；运行用户只看按 Environment、Agent、Backend、Model、Tools、Scorer、Limits 分组的严格 RunSpec。episode_id、attempt_id、lease、digest 等内部字段由系统处理。

数据集业务字段采用 HUD 的方式写在 Python models.py 中，由类型标注生成 schema；用户不手写 schema_ref。运行配置采用 Harbor 的严格分组和提前校验方式。包 metadata、display_name、description、tags 已从当前模板和契约删除，不再保留无消费者的展示字段。`uenv package validate` 对确定重复报错，对 `allowed_time` 这类无法百分之百确认语义的名称只发出警告，交给作者判断。

## 二、五项补充问题

### 1. 当前集成的数据集评分能否都改为 Python？

能。GSM8K、PubMedQA、SciTab、OlymMATH 的提取、比较和标签判定都可迁到 Python；DSCodeBench 已有 Python harness，可重用；SWE 各变体的评分组织和结果解释可由 Python Scorer 调官方 harness。

“评分规则是 Python”不要求所有评分控制都迁到 Python。Python Scorer 可以请求 Worker 运行 Go/JavaScript/pytest 等测试，再解释结果；Rust Worker 仍负责后端执行、私有材料授权、强制超时、每个进入评分的 attempt 至多调用一次评分、补全系统字段和校验结果。Rust 不再实现用户需要改动的数据集评分政策。

语言迁移与政策升级分开：先有可识别版本的对照，之后再改评分规则。现有 Rust/Python 解析已经出现不一致，因此不能不做对照就承诺每条数据得分完全相同。

### 2. 打分位置是否应该统一？

应该。目标统一为 Rust Worker 在每个得到最终 Outcome 的 attempt 中至多调用一次 Python Scorer。Agent 返回回答/代码/补丁，不能把自己填的 reward 当成权威结果；Server/Bridge 不计算得分。用户可直接单元测试 `Scorer.score` 的业务规则，正式字段补全只在 Rust `run_score` 中执行。

Agent 需要多轮测试反馈时，通过 Environment 的 Observation 或公开工具结果获得，不向 Agent 暴露隐藏测试。这些交互反馈写入轨迹，但不产生第二个正式分数。每个得到最终 Outcome 的 attempt 至多调用一次同一个 scorer；Server 只接纳一个 attempt 的 ScoreResult 作为 episode 的权威评分，评测和训练读取该对象。

### 3. 任务专用逻辑不能污染主流程

同意，作为验收规则。主流程允许读取 schema、组件引用、资源和能力，不允许判断 dataset == swe/gsm8k/dscodebench。题目拼接、表格处理、repo 准备、测试 patch、答案提取与评分都放入扩展包。

任务相关代码仍然可以有分支，但分支属于该包。例如 SWE 包内部选择对应版本的测试计划是合理的；Server 因为 SWE 名称去创建 gateway 是不合理的。

### 4. 后端和智能体由参数独立指定

同意，也修正之前容易产生歧义的表述：数据集包不选择后端或智能体。RunSpec.backend、RunSpec.agent、RunSpec.tools 是明确的用户选择。

后端统一抽象为 Rust `Backend` trait，Process、Docker、Podman 实现同一套资源生命周期、执行和文件接口。原先用于启动插件 Python 子进程的能力整理为 Rust ComponentHostProcess/ScorerHostProcess；它们负责进程监管，不是另一种数据集执行策略。

必须区分“指定后端”和“任务依赖”。同一个 SWE 任务可以由 Docker 或 Podman 使用兼容镜像运行；若 Process 有经过验证的依赖配置，也可运行。若只有 OCI 镜像而 Process 无法满足依赖，则提前报告不兼容。系统不因数据集名字拒绝 Process，也不在用户指定 Process 后自动换 Docker。

智能体同样独立。代码修复可以用 PlainAgent+工具，也可以用 OpenHands；数学也可以使用 OpenHands，只要该配置满足预算、输入能力和工具协议。代价和适用性由用户显式选择。示例配置中的推荐组合不成为系统绑定规则。

工具兼容补充：参数可独立选择，不代表任意组合都可运行。Agent 自带工具不会自动与用户 tools 合并；用户提交完整绑定列表，通过必要工具、公共接口和后端能力检查后才能执行。OpenHands 的不可移除原生工具与 `tools=[]` 存在冲突时明确拒绝。部署、工具关系与执行图见 [完整设计第 3—6 章](uenv_design.md)；内部进程时序见[参考说明](reference_sdk_vnext3.md#92-worker-内部多轮调用时序)。

### 5. 原有问题的通俗解释

以下每项都用“发生什么、为什么需要改、如何改”说明，不要求读者了解 Rust。

1. **程序结束不代表答对。** 数学题答错后程序也会正常结束，但现在某个公共字段会因此记为“已解决”。这会误导成功率统计。把执行完成和答案正确拆成两个字段。
2. **显示短回答时，不应同时删掉保存的完整回答。** 当前先按完整回答打分，再截断后保存。之后排查评分或回放轨迹时可能拿不到当时的输入。保存完整原文，界面需要时单独生成短预览。
3. **多轮回答要能分别对应模型版本和 token。** 现在把多轮 token 合起来，并只保存最早的版本。模型中途更新后，就无法确认每轮来自哪个版本。每次模型调用各存一条记录。
4. **评分程序出错不等于模型答错。** 当前部分评分异常直接变零分。训练可能把系统故障当成模型错误。评分出错单独标记，不自动给训练使用。
5. **两份评分代码可能给不同答案。** Rust 和 Python 的部分测试日志处理规则不一样，同一份日志可能产生不同判定。保留一个权威实现，其他调用它；迁移时用相同输入逐项比较。
6. **任务失败后也必须释放资源。** 当前有些提前退出路径没有把环境实例归还或销毁。反复失败可能导致可用实例越来越少。所有退出路径统一执行清理，并验证资源已停止。
7. **用户指定的超时不应被另一层悄悄延长。** Worker 当前会取请求超时和默认值中较大的一个。用户设置短时限可能不生效。使用统一总截止时间，各阶段不得超过剩余时间。
8. **取消也需要保留已发生的步骤。** 当前超时会保留部分轨迹，取消却丢弃。排查取消前的问题时缺信息。两种情况都保存已有步骤，分别标注终态。
9. **配置写错不能改跑另一种程序。** 用户明确选 Agent，但缺任务 ID 时可能回退 Native。结果不符合用户要求。提交时直接指出缺少哪个参数。
10. **缺少模型配置不能直接使用标准答案。** 有一条测试用途分支在特定缺配置情况下返回 target。正式任务可能误用。测试模拟必须显式开启，正常任务缺配置就报错。
11. **请求内容不应藏在另一个默认路径的文件中。** 当前 reset 的部分输入靠临时文件传递。更换机器、容器或重放请求时容易缺文件。把配置或明确文件引用放进请求协议。
12. **同一 ID 不应表示两种东西。** 当前有时 instance_id 是数据集样本，有时是运行环境实例。查询会混淆。分别使用 sample_id、task_id 和 session_id。
13. **可选的后端会话预热不能混用不同版本。** 如果以后保留 `BackendSessionWarmPool`，兼容键必须包含完整 Backend、runtime、Environment 和依赖版本，并在借出前验证。它只是资源启动优化，不是 Agent 池，也不改变正式执行路径。
14. **标准答案不应与模型可见材料混在一起。** 当前接口在一些地方把题目、评分配置一起传递。并不能据此认定已经泄露，但边界不清楚。用独立类型和权限限定评分器才能读取的材料。

以上源码事实与风险定位见前一份 review.md 和本次功能处置清单。资源耗尽等线上后果尚需故障测试复现，不能把静态风险说成已经测出的线上事故。
