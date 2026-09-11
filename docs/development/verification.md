# 本地验证结果与边界

## 按主方案修正执行与持久化（2026-09-11，当前代码）

修改限定在 design。主设计要求没有降低；当前实现缺口集中在[参考实现第 8 节](reference_implementation.md#8-实现状态与待决事项)。未修改 source 快照、生产源码或现有服务。

| 检查 | 结果与范围 |
|---|---|
| 本地完整验证 | scripts/validate_design.py：59 项外层检查通过；其中包含 51 项实际执行的 Rust 测试、9 项 SDK 测试，不能重复相加。4 项真实 Agent 集成测试默认跳过，Windows 不执行 Linux 后端测试 |
| 跨进程执行 | 两个完整场景通过：无工具问答评分；sandbox 工具在 Environment 进程读写工作区后返回 Observation，再生成最终回答并评分。Agent、Environment、Scorer、Model 使用独立 Python 进程，Rust 管生命周期与事件；HTTP 模型回答和 Backend 是测试夹具，不是沙箱隔离证据 |
| 调用校验 | AgentContext 返回 Observation，保留 TaskSpec 类型与 seed；Rust 拒绝修改模型产生的工具 ID/参数/绑定。非法 token、logprob、mask 和错误训练版本不会进入评分；原生关闭方法不要求所有执行器都有 interrupt |
| 冻结与权限 | 工具后台协程按创建来源跟踪，冻结不会取消无关协程；关闭后无工具后台写入。Scorer 文件读取限制在 ScoreInput 及已验证内容可达的引用；进程环境变量继承受限。这些检查不代替操作系统隔离验收 |
| 持久化与恢复 | SQLite 原子提交与幂等、派发 HMAC/epoch/Worker 绑定、磁盘轨迹与预算预占、重启恢复 final_partial、outbox 重传和 ACK 校验通过。重试只消费保存的 RunSpec.retry；事务中保留原截止时间和累计用量，迟到旧结果不能覆盖新 attempt |
| 协议与安装包 | proto 生成 57 个核心类型、10 个内置扩展及 Rust/Python 类型；144 个生成文件重复生成无变化。SDK wheel 在脱离源码目录的隔离解释器中可导入 PlainAgent、protobuf 类型并加载内置 schema |
| Rust 静态检查 | cargo fmt 与 Windows cargo clippy --offline --locked --all-targets -- -D warnings 通过 |

本轮早先的远端 Agent 复测中，PlainAgent 场景通过；OpenHands 场景在工具冻结时暴露 FinishExecutor 不提供 interrupt 的问题。本地已修复并加回归测试。随后同步最新版的操作被自动审批拒绝，尚未上传这版代码或完成远端复测；不能用下面的历史成功记录代替这版验收。待上传文件及摘要列于仓库 target/remote-validation-files.json；文件不含凭据、依赖目录或生产源码。

复现当前本地检查：从仓库根目录运行 `python scripts/validate_design.py`。源码生成命令见 README。当前代码仍未完成全部主设计：后端常驻角色进程、全部原生工具/外部工具服务、官方代码评分 harness、Hub 发布缓存和正式 Bridge/网络服务/Trainer 集成等仍需继续实现与验收；具体边界见参考实现说明。

## 真实 OpenHands、MCP 与 Host RPC（2026-09-11，先前版本记录）

本轮修改 design 的 Rust/Python 接入代码、测试与说明，在已授权的 `8.130.75.157:/tmp/uenv-design-validation-20260911` 内安装独立 integration-venv 并验证。没有修改生产源码或既有服务，没有提交或推送。

| 检查 | 结果与范围 |
|---|---|
| 本地完整验证 | scripts/validate_design.py 的 59 项外层检查通过；其中包含 Rust 测试和 6 项 SDK 工具测试，不重复相加 |
| 真实 OpenHands | 固定 SDK 1.15.0 的 Conversation；先调用 Python add，再调用框架内置 finish；原生执行器返回的同一个对象交还会话；两次生成、两次工具执行、六条对应交互事件 |
| 真实 MCP | 固定 MCP 1.26.0，Streamable HTTP 协议协商、tools/list、tools/call；随机本机端口、一次性认证令牌；未认证请求返回 401。Python 工具返回已登记 UEnvModel，Observation 的结构化内容通过 MCP 保留 |
| 真实 IPC | Rust RpcAgentHost/RpcToolHost/RpcModelProvider 与独立 Python ComponentHost 双向通信；模型生成和工具执行回调不会互相等待；错误代码返回 Worker/Agent |
| 四个会话场景 | 混合工具正常完成；MCP 工具预算为零时无执行；MCP 已执行、原生 finish 超限时原生无执行；PlainAgent 通过同一 RPC 完成两次生成和一次工具调用 |
| 五个传输场景 | 嵌套回调与错误传播、调用超时后终止、在途取消、意外退出、不读取 stdin 的进程不能阻塞 Worker 写入超时 |
| 代码检查 | cargo fmt、Windows cargo clippy --all-targets -- -D warnings，以及文档链接/差异检查；具体运行入口见参考实现第 12 节 |

模型回答由仅绑定本机的确定性 HTTP 服务提供；真实的是 OpenHands 会话、工具、MCP 和进程通信，未调用真实模型权重或训练框架。测试不是生产迁移：原生终端/文件工具的 Backend session 适配、Agent/sandbox 角色权限分离、Hub 包加载、官方 harness 与生产 Server/Worker 调度 RPC 仍在原迁移计划内。当前可信 ComponentHost 的进程通信不能作为运行不可信扩展的隔离证明。

远端复查文件：`agent-integration-final.log` 为可复现入口的最终运行输出；`agent-integration-evidence.json` 记录版本、测试结果、实现文件 SHA-256 和收尾检查。旧验证记录继续表示各次当时的状态。复现使用 [validate_agent_integration.py](../../scripts/validate_agent_integration.py)，命令见[参考实现第 12 节](reference_implementation.md#12-真实-agentmcp-与跨进程-rpc)。

## 统一工具选择与原生格式转换（2026-09-11，上一轮参考）

本轮只修改 design 中的文档、契约、Python/Rust 参考、示例和验证代码；未修改本地 source 快照、远端源码或既有服务，未提交或推送。

| 检查 | 结果与范围 |
|---|---|
| 完整验证 | scripts/validate_design.py 的 59 项外层检查全部通过；其中调用 Rust 控制测试和 SDK 工具测试，不能将嵌套测试重复相加报告 |
| Rust 控制 | 37 项通过：3 项库测试、8 项计划测试、26 项 Supervisor 测试。原生与普通绑定经过同一 step；错误参数不消耗工具预算，重复调用不执行，超限不执行，终止后拒绝新交互，每次只记录一对工具事件 |
| Python 工具 | 6 项通过：函数签名/schema与返回转换、不同原生 Action/返回对象、上下文注入保护、隐藏工具表拒绝、原生归属/版本拒绝、串行执行与 freeze 等待在途调用 |
| 混合选择和发布 | RunSpec.tools 可同时选择普通工具和 Agent 包内原生导出；PlainAgent 误选原生工具、原生版本错误、重名和 agent.config 中重复设置 tools 均拒绝。数据集工具从函数生成发布 schema，不再手写一份 YAML 参数定义 |
| 示例迁移 | 九个 Environment 删除 parse_action/step；普通回答直接提交；CounterEnvironment 持有状态，examples/tools.py 中的 increment 执行操作；旧动作模型、计数和事件从契约与生成示例删除 |
| 生成一致性 | 57 个核心类型、10 个内置扩展、9 个包、11 组运行示例、135 项目标模块；95 个 JSON/生成文档重复生成 SHA-256 无变化 |
| 文档检查 | 10 份 Markdown 的代码围栏、106 个本地链接及锚点检查通过；git diff --check 通过。Mermaid 图未重新渲染 |

NativeToolAdapter.from_openhands 读取 ToolDefinition 的参数模型与原执行器；本轮使用兼容该接口形状的测试替身，没有安装或运行真实 OpenHands SDK。Rust ToolHost/AgentHost 使用测试端口，Python 测试传输也不是完整 RPC。真实 OpenHands Conversation 的执行拦截和原生返回交还、MCP 服务、ComponentHost IPC、进程级强制取消及训练集成仍待完成/验收。本机 Windows 未运行 Linux 专用后端测试；以前的 Linux 证据只适用于当时版本，不能当成本轮新工具链的远端验收。

最近文档检查日期：2026-09-11；最近代码验证日期：2026-09-11。验证对象仅为 `architecture-review-0905/design`。


## 工具定义与 MCP 接入组织（2026-09-11，上一轮文档）

- 独立工具包和数据集包采用相同 tools.py 规范；Environment/AgentRunner 不声明工具，状态工具通过系统注入的 Context 使用当前实例。
- MCP 服务由 UEnv 系统统一包装原工具，Agent 接入代码只负责连接或直接注册；两种接入都委托系统 step，使用同一授权工具表、预算和轨迹。
- 同步主方案、模板示例、用户指南、字段约定、目标类图与时序图、模块清单和迁移计划；本轮只修改文档和模块清单生成器，没有修改运行实现或生产源码。
- 校验通过：10 份 Markdown、104 个本地链接与标题锚点、代码围栏、3 段工具示例语法、模块清单重复生成及 git diff --check。新增公共 MCP 接口模块规划后共 135 项；未渲染 Mermaid，未执行真实 MCP/Agent 测试。

## 工具调用统一为系统 step（2026-09-11，上一轮文档）

- 明确系统 AgentRuntime.step(tool_call) 是唯一工具动作入口，AgentContext.call_tool 仅作 SDK 委托；作者实现工具函数或当前 Environment 的工具方法，不再实现独立动作解析和分发。
- 主设计 5.4、6.2—6.4、8.3，以及用户指南、包模板、字段规则、目标类图/时序图和重构计划已同步；普通问答直接提交 final_answer。
- 目标删除独立动作模型、环境动作计数与事件；工具公开结果统一为 ToolResult.observation。生成字段字典标注当前参考契约尚待迁移，文档生成器不改变运行时 schema。
- 本轮未修改运行代码、数据集代码、生产源码或远端服务；参考代码仍使用旧接口。此记录不代表新接口或真实后端验收。
- 文档校验通过：10 份 Markdown、104 个本地链接与标题锚点、代码围栏；14 个生成文件重复生成摘要一致，运行契约 JSON 的生成前后摘要不变，git diff --check 通过。未渲染 Mermaid，未重复运行后端/模型测试。

## 工具接入文档简化（2026-09-11，上一轮文档）

- 主设计 6.4、用户指南、数据集模板、字段约定和模块规划统一为 Agent 接入代码一次性注册与转交工具调用；删除目标设计中的逐工具接口协商和独立 ToolAdapter 模块。
- 内部 AgentManifest 保留发布信息；工具执行位置、Worker 权限/预算/轨迹及进程边界保留。工具表仍以 ExecutionPlan.tools 为唯一执行来源。
- 本轮仅修改文档及文档生成逻辑。参考 schema、Rust 解析器和示例中的旧协商机制尚待迁移，已在字段字典、参考实现说明与重构计划明确标注，不宣称新路径已实现。
- 两个相关生成器运行成功：核心类型仍为 59 个、扩展 schema 仍为 11 个；目标模块清单现为 134 项。以下旧记录中的 135 项对应当时版本。
- 检查 docs 下 9 份 Markdown、80 个本地链接及标题锚点、代码围栏，均通过；14 个相关生成文件重复生成摘要一致，git diff --check 通过。本轮未运行后端/模型测试，也未渲染 Mermaid。

## 过程评分与 Linux 后端验证（2026-09-11，最近代码验证）

本轮只修改 design，并在用户批准的 `8.130.75.157:/tmp/uenv-design-validation-20260911` 中验证。未修改 source、远端生产目录或现有服务；以下后端测试不代表生产迁移完成。

| 验证对象 | 结果与实际范围 |
|---|---|
| 过程评分 | SDK/schema/Rust 校验已实现 generation_rewards；Python 两次生成示例返回 0、1，整体奖励仍为 1。Rust 检查正常负分、非法/重复/其他 attempt 编号、非法值与额外字段；每次只评分一次，结果与 score 事件一致 |
| 本地完整检查 | 56 项通过，其中一项执行 36 个 Rust 测试；cargo fmt、Windows clippy、git diff --check 通过 |
| Linux 常规测试 | 38 个测试通过；包括真实子进程超时、输出限额和 Podman 拒绝创建资源检查；需要特权的真实后端测试默认 ignored，另行显式运行 |
| Process 真实测试 | bubblewrap 0.9.0、Linux namespaces、cgroup v2；精简 BusyBox rootfs。核验 UID、capabilities、宿主测试文件不可见、没有公网路由、CPU/内存/进程数配置、工作区容量、超时后独立进程组中的后台任务消失、只读候选和重复清理 |
| Docker 真实测试 | 28.3.3 专用引擎，独立 socket/data/exec 目录，关闭网桥及宿主网络参数修改；与 Process 使用相同 rootfs 内容。固定 digest 镜像上的同一组隔离、容量、后台任务清理和冻结测试通过 |
| Podman 真实测试 | 4.9.3/conmon 2.1.10 独立解包环境未通过。早期缺库产生运行时残留；修复依赖后容器可退出，但附着 CLI 超时。当前参考 open 在创建资源前返回 PODMAN_DRIVER_NOT_VALIDATED，不将此驱动宣称为可用，不以关闭隔离或增大超时掩盖失败 |
| 生成一致性 | 59 个核心类型、11 个内置扩展、9 个包、11 组运行示例、135 项目标模块；96 个生成文件重复生成 SHA-256 无变化 |
| 文档 | 10 份 Markdown、98 个本地链接目标及代码围栏检查通过；未重新渲染 Mermaid 图 |

远端 Rust 版本为 1.96.0；Linux 工具链没有 cargo-clippy，因此 Linux 专属模块完成了编译与运行测试，但未通过该环境的 clippy 检查。没有修改远端既有 Rust 安装补装组件。

Docker Hub 拉取超时后，使用 `scripts/serve_backend_fixture.py` 在仅绑定 127.0.0.1 的临时测试镜像服务中提供同一个精简 rootfs，再按完整 digest 拉取。这是实际容器测试夹具，不是官方 SWE 镜像，也不是 Hub 实现。测试没有调用真实模型、OpenHands、MCP 或官方 harness。

可复查的远端证据保留在独立目录：`validation-final.json` 保存 Process/Docker 实测与当时源码 SHA-256，`validation-admission.json` 保存随后加入 Podman 准入拒绝后的完整 Rust 测试和源码 SHA-256，`cleanup.json` 保存资源清理结果。真实后端测试通过之后仅增加了 Podman 拒绝启用的保护；Process/Docker 实测结果不包含正式 ComponentHost 通信或常驻环境能力。

专用 Docker daemon 和镜像测试服务已停止；两个专用引擎均无剩余容器，sessions 目录为空，没有测试挂载或活动测试进程，专用 Docker cgroup 已移除。早期 Podman 依赖错误产生的 4 个 conmon/crun 残留经逐 PID 身份核验后清理。工具、代码与日志留在独立测试目录供复查，没有清理或重启其他服务。

完整常驻 Environment、ComponentHost/RPC、取消接线、受控公网出口、正式 harness 与真实训练框架仍未接通；实现范围见[参考实现第 11 节](reference_implementation.md#11-linux-后端命令执行与隔离)。以下记录仅表示各次检查当时的状态。

## 主方案、全部文档与参考代码一致性复查（2026-09-11，先前记录）

- 审查范围为 README 及 docs 下全部文档，共 10 份 Markdown，并核对 Python SDK、包加载器、九个数据集包、PlainAgent/Counter 示例、Rust 控制流程、协议生成器与生成示例。历史验证记录保留原日期与当时字段，不作为当前规范；生产源码与远端服务未修改。
- 修正 README 已删除类型、可选 Scorer 的局部矛盾、失败结果的字段必填范围、部分轨迹序号规则、配置读取位置、指南章节文字及模块清单中的旧接口名称。具体问题与实现缺口集中记录在 reference_implementation.md 第 10 节。
- 修复参考代码：发布入口具体直接子类/非别名校验；所有已声明模型的递归重复执行字段检查；单次模型输出上限及响应内容/结束原因校验；零工具超时拒绝；评分 Metric 值与字段检查；Rust harness 输入绑定和剩余时间收紧；失败事件序号不复用。实际 token 用量不因超限错误被清零。
- scripts/validate_design.py 的 55 项验证通过，其中一项执行全部 35 个 Rust 测试。新增回归覆盖非法新包、嵌套重复预算、模型超限/非法返回、错误指标、评分输入篡改/超时放大与轨迹序号缺口。
- 三个生成器执行成功；59 个核心类型、11 个内置扩展 schema、九个包、11 组请求配置示例及 135 项目标模块说明重新生成。对 contracts、reference/generated、docs/generated 的 96 个文件再次生成并逐文件比较 SHA-256，零差异。
- cargo fmt 与 cargo clippy --offline --locked --all-targets -- -D warnings 通过；91 个 Markdown 本地链接和锚点、10 份文档代码围栏及 git diff --check 通过。检查到 28 个 Mermaid 围栏，未逐图渲染。
- 结论是参考中已实现路径的本轮冲突已修正，不是主方案全部功能已实现：真实 Bridge/CLI、Hub 数据 API、proto 生成、后端/进程隔离、OpenHands/MCP、官方 harness、租约/持久化恢复、训练消费和 generation_rewards 仍需实施。以下均为历史记录。

## Observation 内容统一复验（2026-09-11）

- Observation 只保留 content、terminated、episode_truncated。ContentPart 统一支持 text、artifact、structured；每项只允许与 kind 对应的一种载荷。结构化内容复用已登记的 UEnvModel 和 TypedConfig，作者调用 structured_part，不手写 schema_ref。
- 同步主方案、字段规范、模板、用户指南、参考说明、重构计划、Python SDK、九个数据集环境及 CounterEnvironment、Rust 控制代码；重新生成契约、示例和模块清单。简单问答直接返回完整文本；代码和仓库任务用结构化内容保留公开输入，避免平行复制相同信息。
- Rust 统一模型入口把结构化业务数据转换成 JSON 文本，原始观测轨迹保留结构，GenerationEvent.messages 记录转换后消息。没有新增 Agent 专属转换路径；普通 JSON 模型输出不自动转换成业务模型。
- 53 项设计验证通过，其中一项执行全部 31 个 Rust 测试。覆盖内容互斥、旧顶层 data 拒绝、未知 schema 与错误字段类型拒绝、结构化模型发布、原始观测保留及实际模型输入记录；69 个 Markdown 本地链接和锚点、代码围栏、cargo fmt 和 git diff --check 检查通过。
- 仅修改 design，未修改生产源码。Python 完整 schema 校验与 Rust 形状校验仍分属参考边界；真实 Host/RPC 接通和端到端验证仍待实施，generation_rewards 也仍未实现。以下为历史记录，不作为当前接口规范。

## 最终提交统一为 final_answer 复验（2026-09-11）

- 文本、补丁文本、文件引用和混合内容统一使用 final_answer: ContentPart[]。删除 ScoreInput、EpisodeResult、HarnessRequest 的独立 artifacts 字段，删除 Python Environment.finalize、Rust Host 接口及 Supervisor 调用；不增加替代收集方法。
- 需要导出补丁时，Agent 在交互结束前经已选择的受控工具完成并显式提交；Worker 不自动提取工作区或工具结果。保留文件完整性校验、停止写入、后端冻结、可选状态读取与评分、清理。finalize_reserve_ms 仍表示系统收尾预算。
- 同步主方案、流程图、字段规范、模板、用户指南、参考实现、重构计划及 Python/Rust 参考代码；重新生成 59 个公共契约、11 个扩展 schema、九个包示例和 135 项模块清单。
- 52 项设计/代码验证通过，其中包含 30 个 Rust 测试。新增覆盖文本补丁、文件引用、混合回答原样传入 Scorer/Harness 和公开结果；无效文件不进入评分且完成清理；旧 artifacts 输出字段被拒绝；纯文本评分器不能静默丢弃文件内容。
- 本轮限于 design 参考实现，未修改生产源码；真实补丁导出工具与后端集成仍需生产验收。generation_rewards 仍为待实现的目标字段。以下保留历史记录，不作为当前接口规范。

## 简化交互结束返回值复验（2026-09-11）

- 删除 EpisodeOutput 及 outcome 包装，不增加替代根类型。Agent.run 返回回答内容列表；Environment.finalize 返回产物列表，state_snapshot 按需返回评分状态。ScoreInput 和 EpisodeResult 直接使用同名字段，公开结果不包含 state 或 private_data。
- Rust Worker 独自记录 termination_reason，区分环境终止、环境截断、实际预算停止和正常提交；仅用量达到上限不判为截断。保留停止工具写入、收集产物、冻结后端、可选状态读取与评分、清理的顺序。
- 同步主方案、字段规范、数据集模板、用户指南、参考实现与重构计划；更新 Python SDK/Agent/Scorer、Rust 控制代码和测试，重新生成 59 个公共契约、11 个扩展 schema、九个包示例及 135 项模块清单。
- 51 项设计/代码验证通过，其中包含 29 个 Rust 测试。覆盖显式空回答不被模型响应替换、评分状态在清理后仍可用且不进入公开结果、无评分不读取状态、旧包装拒绝、预算拒绝被 Agent 捕获后仍可正确记录原因、错误评分保留与清理顺序。
- 69 个 Markdown 本地链接和锚点、代码围栏、cargo fmt 和 git diff --check 检查通过；未逐图渲染 Mermaid。验证仅覆盖 design 参考实现，生产源码未修改；generation_rewards 仍是待实现的目标字段。以下保留历史记录，不作为当前接口定义。

## 删除环境奖励字段复验（2026-09-11）

- Observation 仅保留 content、data、terminated、episode_truncated。删除 SDK、序列化、CounterEnvironment、协议源、生成字段字典和当前规范中的环境奖励字段；正式评分统一归 Scorer。
- 重新生成公共契约及九个数据集示例。50 项设计/代码验证通过，包含 Rust 控制测试；核对 Observation 精确字段集合及未知字段拒绝规则，当前文档与代码无遗留引用，git diff --check 通过。
- 本轮限于 design 参考实现；以下历史记录反映各次修改时的状态，不作为当前字段定义。

## 统一 Observation 复验（2026-09-11）

- reset 与 step 统一返回 Observation，包含 content、data、terminated、episode_truncated 和可选 environment_reward。移除 StepResult 类型与 AgentContext.transition，仅维护当前 observation。
- 同步九个数据集、CounterEnvironment、PlainAgent、Rust Runtime/Supervisor、协议生成器、轨迹 EnvironmentTransition.observation 及文档；EpisodeOutput 继续保存最终输出。
- 重新生成 60 个公共契约类型、11 个扩展 schema、9 个数据集包示例和 136 项模块清单。50 项设计/代码验证通过，包含 Rust 控制测试；新增初始化结束或截断后不生成、旧嵌套观测格式拒绝的回归检查。cargo fmt 已执行。
- Markdown 链接/锚点、代码围栏和 git diff --check 通过；未逐图渲染 Mermaid。验证仅覆盖 design 参考实现，未修改生产源码；generation_rewards 仍是待接入的目标字段。以下为历史记录。

## 过程评分文档修订（2026-09-11）

- 主方案、字段规范、数据集模板、用户指南和重构计划统一约定：以一次模型生成为过程评分单位，交互结束后一次 Scorer 调用返回整体 reward 和可选 generation_rewards。
- 新字段的作者、消费者、ID 关联、缺项、非法值、错误返回和训练消费边界已经明确；不新增逐动作/文本片段评分或实时评分链。
- 本次仅修订文档；参考 SDK、生成 schema、Rust 校验和训练导出尚未支持 generation_rewards，参考实现说明明确列出缺口。模板中的新增 JSON 为目标示意，不能作为当前可执行结果。
- 检查 Markdown 本地链接与锚点、代码围栏、示例 JSON 及 diff 空白；未渲染 Mermaid，未重跑未修改的代码测试。以下为历史验证记录。

## 交互类型命名复验（2026-09-10）

- 保留 Observation；步骤返回类型统一使用 StepResult，episode 输出类型统一使用 EpisodeOutput。同步 Python SDK、九个数据集环境示例、Agent 示例、Rust 类型校验入口和文档类图，不增加旧名别名。
- 重新生成 61 个公共契约类型、11 个扩展 schema、9 个数据集包的示例及 136 项模块清单。字段结构和执行行为保持不变。
- 使用本机已有依赖的 `D:/AppData/anaconda3/python.exe` 运行生成器和 `scripts/validate_design.py`，49 项验证通过，包含离线 Rust 控制测试。类型引用扫描与 `git diff --check` 通过。
- 验证范围仅为 design 参考代码和生成物，未修改生产源码。以下为历史验证记录。

## 扩展代码归属与流程说明复验（2026-09-10）

- 样本准备图明确为“UEnv 调用数据集包内 DatasetAdapter.normalize”，返回 PreparedSample 后由系统组装 TaskSpec。用户指南区分包作者实现转换规则与普通用户复用已支持格式。
- 主方案职责表、参考类图/时序图和模块清单区分 SDK 接口、系统 Host 与包内 Environment/Scorer 实现；方法签名统一使用源码中的 normalize(row)。没有新增扩展类或业务实现要求。
- 删除模板中未定义的包内 prepare/build 入口说法，明确资源存储/摘要与数据集字段解释的边界。Code/SWE 参考仍依赖已准备的资源引用与 EvaluationPlan，完整原始数据导入尚未实现。
- 修正遗留的 Python 原始事件上报描述、RunSpec 独立评分器选择描述，以及参考第 9.2 节中预算建立和停止工具写入的顺序；freeze 失败进入清理，只有 close 的一步失败仍继续关闭其余资源。
- 重新生成 136 项模块清单；49 项 Python/设计验证通过，包含 28 个 Rust 测试入口。10 份 Markdown 本地链接路径及代码块检查、diff 空白检查通过；未逐图渲染 Mermaid。未新增测试、临时脚本或目录。
- 本轮只修改 design 文档和模块说明生成器，未改变运行行为、数据集实现或生产源码。以下为历史验证记录。

## 数据集包统一选择复验（2026-09-10）

- RunSpec 只通过 dataset_package.id/version 选择数据集代码包；ExecutionPlan 只保存一份锁定后的 dataset_package。Environment 和 Scorer 不再分别携带 implementation，二者固定从本包入口加载。
- environment 是本包环境参数，scoring.enabled 是用户必填的唯一评分开关。开启时补齐并校验 scoring.config，关闭时禁止该参数；包内缺少 Scorer 却开启评分时拒绝。评测和训练要求开启，轨迹采集可关闭。
- Rust PlanResolver、EnvironmentHost/ScorerHost 调用、Supervisor 和结果校验已同步。评分结果的 scorer 身份由锁定的数据集包派生；它是结果来源记录，不是第二份选择。关闭评分不解析/派发私有评分材料，不创建 ScorerHost，正常完成其余生命周期。
- 11 份 run.yaml 及生成的批次、RunSpec、ExecutionPlan 全部改造；九个数据集包继续保留各自三个专属类。契约与字段字典重新生成，共 61 个核心类型、11 个内置扩展 schema；模块清单为 136 项。
- 49 项 Python/设计验证通过，包含 28 项 Rust 测试的执行入口。检查覆盖跨包覆盖拒绝、旧 scorer 字段拒绝、评分开关类型、关闭时禁止参数、包内缺少评分入口和无评分生命周期。cargo fmt、clippy、diff 空白检查及 10 份 Markdown 本地路径/代码块检查通过；未逐图渲染 Mermaid。
- 本次只修改 design，未改生产源码或远程服务，未新增目录或临时文件。正式客户端、Hub 服务、真实进程和后端集成仍待实施。以下为历史记录；其中独立 Environment/Scorer 选择和通过省略 scorer 控制评分的旧规则，以本节为准。

## 统一入口与职责审查复验（2026-09-10）

- CLI 明确调用 UEnvClient，同一 BridgeService 负责配置准备与提交；dry-run 使用同一过程。这里只修改目标接口和参考约束，未实现或宣称已有产品客户端。
- Python 参考拒绝 YAML 重复键、未登记组件版本；Environment 与 Scorer 按各自 id/version 解析，可选用不同兼容包，任务 schema 不兼容仍拒绝。
- Rust Supervisor 核对环境 StepResult 与 EpisodeOutput 结束原因，禁止 finalize 改写原因；顺序调整为停止工具写入、收集结果、冻结后端、可选评分。真实进程冻结仍待验收。
- AgentRuntime 检查模型事件的 model_id/source；模型或环境动作已发起却没有合法记录时，最终轨迹标记 final_partial。用量只表示已确认部分，不将缺失量解释为零；新增错误模型记录与不完整轨迹回归检查。
- ToolResult 增加复用 TypedConfig 的可选 data，ToolSpec.output_schema 改为按需声明；有 schema 的成功结构化结果须由实际 ToolHost 校验。已有信封/schema 测试，真实 MCP 映射仍待实施。
- 删除 Python Host 目标目录中独立模型客户端与权威事件客户端，避免绕过受控入口；三个生成器成功，60 个核心类型、11 个内置扩展 schema、九个包、11 份运行示例，目标模块清单为 136 项。
- 49 项 Python/设计验证全部通过，其中包含 Rust 测试执行入口；Rust 共 27 项测试。cargo fmt、cargo clippy --offline --locked --all-targets -- -D warnings、diff 空白检查通过。10 份 Markdown 的本地链接路径和代码块边界检查通过，未逐图渲染 Mermaid。
- 问题、外部实现对照与未接通的接口集中记录在 reference_implementation.md 第 10 节。本次仅修改 design；生产源码及远程服务未修改。未新增文件夹或临时脚本；依赖、编译缓存仍在系统临时目录，未重复 wheel 构建。

## 内部文件存储接口复验（2026-09-10）

- 内部文件存储接口统一命名为 FileStore；内存参考实现为 MemoryFileStore，目标本地文件实现为 LocalFileStore，模块清单使用 file_store.rs。文档、Rust 调用点、测试替身和模块生成器同步命名。
- FileStore 从常驻组件表移除，只负责存取文件字节和完整性校验。Hub 管版本与授权，Server 管运行记录与保存期限，TrajectoryWriter 管轨迹格式及分片；FileStore 不读取 RunSpec。普通用户不配置或实现该接口，现有 ArtifactRef 协议不变。
- 模块清单重新生成；45 项 Python/设计验证通过，包含 26 项 Rust 测试的执行入口；Rust 格式及 diff 空白检查通过。检查旧接口名称无残留，未新增测试或更改存储行为。
- 本次变更仅在 design。内存参考仍不代表真实文件存储驱动、回收服务或生产集成已经实现。

## 动作输入与模型内容解析复验（2026-09-10）

- step 只接收所选环境声明的动作模型；新增 parse_action 转换归一化内容，不增加动作次数、不执行动作或评分。models.action 生成 action_schema，加载器绑定同一模型并检查方法标注，RunSpec 不增加覆盖入口。
- 复用已有 AnswerAction，字段统一为 content: ContentPart[]，保留完整回答；不再使用旧 answer 字段。九个数据集显式实现 parse_action/step，并共享系统动作 schema；未新增 Parser 类或包内重复 schema。
- PlainAgent 根据 StepResult 决定继续或结束，模型 stop 不再直接表示最终回答。支持工具和环境反馈的多轮；工具已执行的环境动作不重复提交。回答提交计一次 environment_step，11 份运行配置及生成批次/计划同步将该预算从 0 调为 1，max_generations 原值保持不变。
- Rust 增加受控 parse_action、统一动作校验和最近 StepResult 查询，终止后拒绝新的模型、工具、解析或动作调用。格式错误在执行前拒绝；解析不增加动作计数。原模型内容和执行动作分别使用既有 generation.response 和 environment_transition.action，不新增事件根类型。
- 三个生成器成功：仍为 60 个核心类型、11 个内置扩展 schema、九个包和 138 条目标模块说明。
- 45 项 Python/设计验证通过（包含 Rust 测试入口）；Rust 共 26 项测试通过。新增检查覆盖九个环境的原文提交、CounterEnvironment 的模型 JSON 多轮交互、声明/标注冲突、错误动作拒绝、终止后拒绝交互、环境预算和工具终止反馈。
- cargo fmt 与 clippy --offline --locked --all-targets -- -D warnings 通过；本地 Markdown 链接与代码块边界检查通过。未对 Mermaid 逐图渲染。
- 本次只修改 design 中的文档、参考代码和生成物。测试使用 Python Context 和 Rust 端口模拟；生产源码、真实模型/后端、OpenHands/MCP 与跨进程回调未修改或验收。依赖及 Rust 编译缓存位于系统临时目录，design 内未新增临时脚本或构建目录。本次未重复 wheel 构建。

以下为历史验证记录；其中旧的 PlainAgent 结束条件、问答动作计数和测试数量以本节为准。

## PlainAgent 多轮复验（2026-09-10）

- PlainAgent.run 改为生成、工具执行与反馈循环；最终回答提前结束。full 保留全部历史，last_generation 保留系统提示、初始任务和最近完整模型/工具交互。两种策略不改变原轨迹。
- 所有 PlainAgent 公共运行示例初始使用 limits.max_generations=1；DSCodeBench 从 30 改为 1 并同步生成计划。该字段仍为唯一模型调用上限，不增加 Agent 自有轮数配置；用户提高上限后允许工具反馈驱动多次生成。
- GenerationEvent.tool_calls 与 assistant Message.tool_calls 复用现有 ToolCall 结构，不新增工具请求根类型。受控 ModelProvider 绑定模型请求与已选工具，Rust 校验生成身份、工具绑定和重复调用身份；真正执行仍由 call_tool 的预算/权限入口接纳并记录事件。
- AgentRuntimeError 是已有 ErrorRecord 的 SDK 异常包装，不增加传输类型。PlainAgent 只把预算拒绝转为 budget_exhausted；取消、模型故障和权限错误保持错误语义。
- 40 项验证通过，其中包含 Rust 测试入口；Rust 共 25 项测试（3 项单元、6 项计划、16 项执行控制）。新增测试覆盖多次生成、工具反馈、完整/最近历史、提前停止、上限 1、长度耗尽、错误传播、结构化工具消息约束，以及 Rust 拒绝未选择工具。
- cargo fmt、clippy --offline --locked --all-targets -- -D warnings 和 diff 空白检查通过。验证只覆盖本地 Python Context 与 Rust 端口，真实跨进程 SDK、模型 API/MCP/OpenHands 接入仍需按重构计划完成。生产源码未改。

## 轨迹采集与可选评分复验（2026-09-10）

- RunSpec 新增 trajectory_collection；scorer 在评测/训练必填，在轨迹采集可省略。training 仍仅用于在线训练接入。没有新增提交协议、执行器或轨迹根类型。
- 统一使用 limits.finalize_reserve_ms；Python 契约、Rust 预算、九份原运行配置及生成计划同步替换旧字段。对照前一提交，九份原 ExecutionPlan 除预算字段名称和随之变化的 plan_digest 外，执行配置与任务内容一致。
- 无评分时不下发私有材料、不解析隐藏 harness、不创建 ScorerHost、不生成评分前快照或 score 事件；正常完成仍保存 EpisodeOutput、清理与最终完整轨迹。有评分失败时保留错误评分和轨迹。
- Rust validate_result_for_plan 结合计划检查身份、Scorer 身份和 score 应否存在；配置了 scorer 的 completed 丢失成功评分时拒绝，未配置时拒绝伪造评分。生产 Server 尚需把此校验与完整 schema/租约事务接通。
- 新增 gsm8k_collection、gsm8k_collection_scored 两份完整运行配置及对应生成批次/计划。已有九个数据集保留三个专属类；发布加载器和 PackageManifest 支持仅采集包省略 Scorer 入口及配置 schema，已通过无评分器包发布测试。
- 35 项 Python/设计验证通过，其中包含 Rust 测试入口；Rust 共 24 项测试通过（3 项单元、6 项计划、15 项执行控制）。fmt、clippy --offline --locked --all-targets -- -D warnings 通过。
- 82 个本地文件链接及标题锚点、26 个 Mermaid 块的边界和时序分支配对检查通过；未逐图渲染。三个生成器均已执行，所有当前规范与示例使用唯一预算字段名。
- 以上验证仅针对 design 的参考实现。生产源码未修改；真实采集导出、Hub 服务、训练消费、模型、容器和 OpenHands 的接入与验收仍按重构计划实施。没有新增临时脚本或仓库内构建缓存。

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
- Environment.step 只能通过 AgentRuntime 的单一入口调用，Rust 在动作副作用前检查并预占预算，再校验和记录 StepResult。
- Backend 只创建任务 session；ToolHost 从唯一 `ExecutionPlan.tools` 绑定实际路由。工具执行失败时仍写入与 ToolCall 同 id 的 ToolResult(error)。
- ToolHost 的实际可路由表和 AgentHost 的实际模型可见表都必须精确等于 `ExecutionPlan.tools`；隐藏原生工具和缺失适配器都会在 reset 前失败并清理。
- 评分器异常后，冻结 EpisodeOutput 与 `status=error` 的同一个 ScoreResult 仍保存在 EpisodeResult。
- 模型、工具、环境错误保留明确的 phase、operation_id 和 retryable；评分超时不会被改写为笼统的评分失败。
- 轨迹序号由 Rust 连续分配，私有材料和覆盖它的 plan_digest 不进入公开轨迹。
- 轨迹事件按 UTF-8 JSONL 序列化后的字节大小分片，单片上限为系统内部固定的 4 MiB；只在完整事件之间切分，并标记 `application/x-ndjson`。
- `TrajectoryManifest.trajectory_status` 用 scoring_checkpoint、final_complete、final_partial 分开表达评分快照和最终记录完整性，不再复用 complete 布尔值。
- 任一事件或 checkpoint 写入失败都会使最终轨迹标记为 final_partial；已形成的 EpisodeOutput/ScoreResult 不因 score 事件写入失败而丢失。
- EpisodeOutput.termination_reason 是唯一交互结束原因；EpisodeResult/TerminalEvent 不再复制。WorkerRegistration.components 是包括 Backend 在内的唯一安装清单。
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
