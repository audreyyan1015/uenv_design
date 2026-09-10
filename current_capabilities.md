# 当前功能与代码处置清单

基线提交 af675b20b91c66672b0517b378205603fe424bf3；2026-09-07 再次只读连接原服务器，工作区干净，共有 193 个 Rust 文件和 189 个 Python 文件。下表的“已有”表示存在实现与调用入口，不表示本次已运行验证。测试文件存在也不等于本次测试通过。保留的含义是保留实现中有效逻辑及行为，不保证源文件一行不改。

## 数据集范围

| 数据集 | 代码支持证据 | 现状判断 | 目标包 |
|---|---|---|---|
| GSM8K | math/qa manifest；gsm8k scoring.rs；Bridge 数据准备脚本 | 有题目转换和评分实现 | datasets/gsm8k |
| PubMedQA | math/qa manifest；标签评分；Bridge benchmark/train scripts | 有训练/评测输入和标签评分 | datasets/pubmedqa |
| SciTab | math/qa manifest；表格转换和标签评分 | 有表格任务、分类评测实现 | datasets/scitab |
| OlymMATH | math/qa manifest；数学评分；Bridge EN/ZH、easy/hard 元数据转换 | 变体共享算法，不需四条调度链 | datasets/olymmath |
| DSCodeBench | code manifest；evaluate_code.py/dscodebench_harness.py | 有代码生成、测试、分数解释；另有 Code Agent 服务路径 | datasets/dscodebench |
| SWE-bench Verified | BenchmarkVariant、dataset/session、grader、OpenHands adapter | 有完整相关执行模块；本次未跑 benchmark | datasets/swe-verified |
| SWE-bench Lite | 同家族 enum/parser/grader 分支 | 有变体支持，不能仅此认定独立端到端验收 | datasets/swe-lite |
| SWE-bench Pro | pro_eval、runtime_contract、OpenHands official runner | 有专用 harness 和运行依赖处理 | datasets/swe-pro |
| SWE-smith | smith_eval、dataset、训练准备与运行代码 | 有专用训练/评测路径与资源要求 | datasets/swe-smith |

`math`、`qa`、`code`、`swe` 是现有环境/路由标识，不是应新增的四个数据集。OpenEnv/MCP 是接口适配，不列为数据集。ROLL/GEM 目录有实验脚本，尚不据此宣称所有相关环境已集成验收。

证据：math/qa 数据集声明（生产源码 `plugins/math/manifest.yaml:13`）、Code 数据集声明（生产源码 `plugins/code/manifest.yaml:11`）、SWE 四变体（生产源码 `uenv-worker/src/swe/variant.rs:8`）。

## 功能处置

| 功能 | 当前关键代码 | 判断 | 处置方式与验收 |
|---|---|---|---|
| Python Bridge 框架接入 | verl_agent_loop.py、clients.py | 修改/拆分 | 保留有效 VeRL 映射，分出 request/result/model/trace；移除业务别名猜测 |
| Rust AdapterCore 批次映射 | core/src/core.rs、protocol.rs、service.rs | 合并进 Rust Server | 独立进程可退役，但批次流、背压和协议校验继续使用 Rust；Python Bridge 只做框架转换和调用 |
| 通用批次提交 | AdapterCore.execute_batch、Server submit_episode_batch | 保留行为、修改协议 | ID 对齐、部分失败、重复提交契约不丢失 |
| 模型 Gateway | bridge/model_gateway.py | 保留并修改 | 保留推理服务接入，规范化真实 token/version，去除数据集语义 |
| Native SWE 直连训练 baseline | native_swe_agent_loop.py | 移出正式核心 | 基线对照脚本保留于 experiments，不作为产品的另一条任务链 |
| VeRL 版本补丁 | verl_*_patch.py、sitecustomize.py | 隔离后保留必要项 | 锁定依赖版本、记录作用范围，能由正式 adapter 代替时移除，禁止无条件全局修改 |
| Server 提交与幂等 | episode_coordinator.rs、service/episode.rs | 保留规则、修改组织 | 稳定 ID、冲突拒绝、同一任务挂接原结果；加强事务唯一终态 |
| Server 执行后端分流 | execution_backend.rs、SweAgentSpec/CodeAgentSpec | 重写 | 统一 ExecutionPlan；删除以数据集判断 native/agent 的代码 |
| Server admission | admission.rs | 保留 | 队列/全局并发和 Worker 本地并发不同；统一计数收尾，测试取消等待 |
| Worker 注册、心跳、drain | control_plane.rs、scheduler/traits.rs | 保留并修改 | 删除 supported_env_types 的路由职责，按能力/组件版本匹配 |
| Worker 选择与 reservation | scheduler/mod.rs | 修改 | 保留容量防超配；输入换能力/资源，release 按 reservation identity 防重复 |
| lease/epoch/dispatch token | proto 与 dispatch/control plane | 保留并统一 | 旧 attempt 结果不覆盖新结果；不增加未经设计的多主 Server |
| Server SQLite/outbox/recovery | persistence/* | 保留有效逻辑、修改状态模型 | 删除 native/agent 各自恢复流程，统一 attempt 对账 |
| Server ResultFinalizer | result_finalizer.rs | 保留收口角色、重写字段处理 | 不补造评分/token，事务收口后再发布 |
| 观测事件与页面查询 | obs/*、admin_http.rs、trajectory/* | 保留并修改 | 页面以规范事件投影，删除分支手写补偿与同义字段猜测 |
| Worker 主执行循环 | episode/executor.rs | 重写 | Rust EpisodeSupervisor 保留唯一外层状态机；模型决策循环移入 Python AgentRunner，环境准备、冻结、评分、轨迹和清理由 Rust 强制排序 |
| Worker 模型请求 | episode/model_client.rs | 迁移有效传输逻辑 | Python Agent/SDK 组织模型消息，Rust Worker 强制端点配置、预算、取消并接收真实生成事件；移除隐式 target 回答 |
| Worker reward override | episode/reward_engine.rs | 删除旧入口 | Rust Supervisor 在每个进入评分的 attempt 中至多调用一次 Python Scorer，补全并校验 ScoreResult；Server 每个 episode 只接纳一个，评测与训练复用 |
| Python process-plugin 模板 | templates/process-plugin/* | 修改/迁移 | 复用简单生命周期思想，reset 配置进入协议，不再依赖 sidecar |
| 插件进程创建/健康/关闭 | plugin/host.rs、backend/process.rs | 保留底层能力、修改职责 | 改为 Rust ComponentHostProcess/ScorerHostProcess，强化取消、超时和子进程组回收 |
| 通用 Backend 与 SWE Backend | backend/*、swe/backend/* | 合并接口，复用驱动 | 抽出 dataset-neutral 的 Process/Docker/Podman 实现 |
| SWE Docker/Podman CLI 操作 | swe/backend/cli_container.rs | 保留并修改 | 去掉 SWE 类型依赖；错误、预算、exec/files 协议一致 |
| 镜像下载/缓存 | swe/image_cache.rs | 保留并移出 SWE | 按不可变 digest，保留镜像命名逻辑于数据包准备流程 |
| WarmupPool/SweInstancePool | pool/*、swe/instance_pool.rs | 修改并收敛为可选后端资源机制 | 只保留 `BackendSessionWarmPool`：按完整后端/session 兼容键 borrow/release/invalidated；任务 reset 仍由 Environment 实现，与 Agent 调度无关 |
| WarmupSizer | pool/warmup_sizer.rs | 延后默认启用 | 测量收益后开启，不是正确性的依赖 |
| Worker WAL | wal/mod.rs | 保留功能、收敛为 durable outbox | 只负责计算结果在 ACK 前可恢复，不与 Server 争权威状态 |
| 轨迹存储/上传 | swe/trajectory*.rs、uenv-common/trajectory.rs | 保留 Rust I/O 与可靠性能力、重写模型 | Python 只提交事件内容；Rust 统一身份、序号、原文、逐生成 trace、快照、封存、部分轨迹与 ACK/GC |
| SWE Runtime Gateway | runtime_gateway/mod.rs | 重写业务边界 | 保留远端 exec/read/write 的必要传输；解绑 SweInstancePool，submit 交 Worker 评分服务 |
| OpenHands SDK 接入 | integrations/openhands/* | 保留 SDK 与工具链、修改 adapter | 保留真实 Conversation/工具执行；去掉任务特判、最终评分控制和全局副作用扩散 |
| 现有 Agent 池选择与入队 | service/episode.rs:703、714、1002、1010；2026-09-05 远端只读核对 | 目标移除，仅迁移期兼容 | Worker 统一管理 Agent 生命周期；退役池身份解析、独立容量和任务队列，不再引入 Agent 池 |
| 数学/标签评分 | plugins/math/src/backends/* | 重写为 Python | 使用旧 scorer 和固定语料做差分；新增规则独立版本化 |
| DSCodeBench Python harness | plugins/code/scripts/* | 保留有效 harness | 由新 Python Scorer 使用 Worker 执行能力调用；重新区分候选错误和 harness 故障 |
| SWE 评分/harness | swe/grader.rs、*_eval.rs、plugins/swe/evaluator | Python 入口重写，保留官方 harness/依赖 | 不用多份日志解析器互为静默 fallback；按变体验收 |
| 数据集原始输入适配 | Bridge benchmark utils、swe/dataset.rs | 迁移到包并统一模板 | 题目、表格、repo、patch、测试计划都在扩展包，不在公共 Worker |
| 命令限制与隔离 profile | swe/command_policy.rs、sandbox_profiles/* | 重写为 Backend 内部策略 | 用户不配置 syscall/capability；环境包只声明 internet_access，Backend 统一落实平台安全底线 |
| Hub artifact/version/schema | hub package/repository/domain | 保留 | 版本、digest、发布校验和读取能力继续使用 |
| Hub env/EnvPackage/Stack | hub types/domain/stack.rs | 修改对外模型、重写特判 | 对用户统一 manifest + RunSpec，内部区分组件和运行组合；旧实体入口转换 |
| 评分对齐证据 gate | hub domain/rubric.rs | 保留必要验证 | 对照 corpus、scorer digest 与报告挂在评分包，不成为数据集路由 |
| 测试与压测工具 | tests/*、stress_test_refactored | 保留有价值用例，更新路径断言 | 必须测新公共链；已弃用 shortcut 不继续作为端到端验收 |

## 对本次补查的修正

当前系统协议没有单一来源：公共 episode 消息位于 `proto/uenv/v1/*.proto`，Worker RPC 又在 `uenv-worker/proto/worker_service.proto`，Hub RPC 在 `uenv-hub/proto/hub.proto`，Hub HTTP/存储 DTO 还在 `uenv-hub-types/src/lib.rs` 手写。它们表达的是同一系统的边界对象，却由不同目录和不同语言分别维护。目标重构必须把 UEnv 系统 message/service 收敛到 `contracts/proto/uenv/v1/`，再生成 Rust/Python 类型、RPC stub、核心 JSON schema 和字段文档。数据集新增业务字段不进入核心 proto，只在包内 models.py 定义并生成包 schema，从而避免新增数据集触发核心协议升级。

当前 `EpisodeRequest.payload` 是 bytes，注释允许它承载 question、dataset、SWE instance 等多种业务字段；源码没有一个面向作者、可查询的字段目录。Hub 的 `config_schema`、`interface` 和若干开放 JSON Value 又分散在另一组协议/DTO 中。因此当前用户无法可靠地区分系统字段与扩展字段，只能查源码和数据集样例。目标方案增加由统一契约驱动且按作者任务过滤的 `uenv describe`、Python 类型提示与发布前校验：数据集作者只看 PreparedSample/包模型，运行用户只看 RunSpec，内部字段不进入默认视图。这些都属于待实现能力，不能描述成现有命令。

之前审查强调了 `backend/mod.rs` 的薄接口，但不足以覆盖后端现状。补查确认 SweSessionBackend（生产源码 `uenv-worker/src/swe/backend/mod.rs:90`） 已定义 provision/exec/read/write/terminate/reconcile。因此后端目标不是从零重写底层执行能力，而是合并现有两组抽象，复用 CLI container 的底层操作，并清除其中的 SWE 类型耦合。

Bridge 当前也不只是一个 Python 转发器：还存在 Rust AdapterCore（生产源码 `uenv-bridge/core/src/core.rs:31`），其 gRPC 服务还实现批次流并发与背压；Python `UEnvAgentLoop` 承担请求构造、模型端点、重试、轨迹转换和框架适配。目标允许删除单独 AdapterCore 进程，但不会把流控和权威协议校验迁入 Python；这些逻辑并入 Rust Server。

Worker 当前也不只是资源启动器。EpisodeExecutor（生产源码 `uenv-worker/src/episode/executor.rs:210`） 在 Rust 中掌握 reset/step、超时失败、reward 和轨迹，插件宿主（生产源码 `uenv-worker/src/plugin/host.rs:54`） 在 Rust 中监管 Python/Rust 插件进程。目标重构会清除数据集分支，但应保留 Rust 对 attempt 生命周期的最终控制；Python ComponentHost 只执行用户组件回调。

## 迁移保护规则

保留现有代码的历史快照和评分语料作为行为对照；不同分数必须明确是修 bug、升级评分政策还是迁移错误。任何旧模块只有在其功能迁入新路径并有替代验收后才删除。实验目录可以保留 baseline，但不能从产品 API 隐式进入实验路径。本文不把任何尚未执行的迁移或测试标成完成。
