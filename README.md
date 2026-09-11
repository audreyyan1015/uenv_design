# UEnv 设计交付包

这份目录同时包含设计文档、公共契约、九个数据集示例和一套可执行参考。远端源码基线为 `af675b20b91c66672b0517b378205603fe424bf3`；本轮没有修改远端仓库或本地 `source` 快照。

目标语言分工已经落实到参考代码：

| 责任 | 语言 | 本地参考 |
|---|---|---|
| Server 解析唯一 `ExecutionPlan`，锁定组件、工具、harness、镜像、能力与截止时间 | Rust | `reference-control/src/plan.rs` |
| Worker 管理预算、后端、工具路由、轨迹、评分调用、清理与结果 | Rust | `reference-control/src/runtime.rs`、`scoring.rs`、`supervisor.rs` |
| 数据集 Adapter、Environment、Scorer | Python | `reference/datasets/` |
| AgentRunner、ToolExecutor、UEnvModel 及 schema 生成 | Python | `reference/sdk/src/uenv/sdk/`、`reference/extension_templates.py` |
| schema、示例和模块清单生成 | Python 开发工具 | `scripts/build_*.py` |

Python 中原有的 `EpisodeRuntime`、计划解析器、工具权威路由和 `SandboxBackend` 模板已经删除，避免系统控制职责在两种语言各实现一遍。Python Scorer 填写 `success`、`metrics`、`reward`、`evidence` 和可选的 `generation_rewards`；Rust Worker 补全 `status`、`scorer`、`error`。

本仓库根目录对应原工程的 `design` 目录，包含设计文档、参考代码、生成契约和示例。文档中的生产源码位置用于说明审查依据；生产源码快照及原工程其他文件不包含在本仓库中。

## 目录结构

```text
design/
├── README.md              # 仓库入口与运行说明
├── docs/
│   ├── uenv_design.md      # 主方案
│   ├── guides/            # 用户操作与数据集模板（2 篇）
│   ├── development/       # 字段、重构、参考实现、验证（4 篇）
│   └── generated/         # 自动生成的字段字典与模块清单（2 篇）
├── scripts/               # 生成与验证工具，以及 requirements.txt
├── contracts/             # 生成的公共协议和内置组件 JSON Schema
├── reference/             # Python SDK、数据集、智能体与工具参考示例
├── reference-control/     # Rust 控制流程参考代码与测试
├── Cargo.toml             # Rust workspace 构建入口
├── Cargo.lock             # Rust 依赖版本锁定
├── .gitignore             # Git 忽略规则
└── .gitattributes          # 文本换行规则
```

文档中的代码路径以仓库根目录为基准，点击链接则按所在文档解析。`docs/generated/module_map.md` 描述未来生产系统的目录规划；上面列出的是当前设计仓库的实际组织。维护命令统一从仓库根目录运行。

## 阅读顺序

先读主方案了解系统；运行用户按需查 guides，开发维护者查 development。generated 只用于查询，不手工编辑。

| 文档 | 唯一负责的内容 |
|---|---|
| [系统设计方案](docs/uenv_design.md) | 系统职责、执行流程和对外行为 |
| [用户指南](docs/guides/user_guide.md) | 运行、查询、取消任务，以及自定义 Agent 和工具 |
| [数据集包模板](docs/guides/dataset_package_template.md) | 数据集文件、组件入口、样本转换、发布内容及内置示例 |
| [字段规范](docs/development/field_conventions.md) | 命名、归属、配置单一来源与字段用途要求 |
| [源码重构计划](docs/development/source_refactoring_plan.md) | 原有能力与源码证据、保留/修改/重写/删除、迁移和验收 |
| [参考实现说明](docs/development/reference_implementation.md) | 当前代码对应关系、实际消费者、实现缺口与内部接入目标 |
| [验证记录](docs/development/verification.md) | 运行了哪些检查、结果怎样、尚未验证什么 |
| [生成字段字典](docs/generated/field_dictionary.md) | 从当前参考契约生成的完整字段查询 |
| [生成模块清单](docs/generated/module_map.md) | 从目标模块清单生成的逐文件职责 |

相同规则只在负责它的文档维护。主方案引用字段规范；指南用步骤和示例说明如何使用，不复制完整协议；参考说明记录代码实现状态，验证记录只记录检查证据。修改规范时同步代码及生成器，重新生成 generated，不在生成文档中单独修正。

## 参考代码

- [Rust 控制参考](reference-control/src/lib.rs)：以 DispatchRequest 为唯一执行入口，配置只读其中的 ExecutionPlan，并统一处理预算、工具、轨迹、评分与清理。
- [Python SDK](reference/sdk/src/uenv/sdk/__init__.py)：用户可实现的接口和公共数据对象；[类型模型](reference/sdk/src/uenv/sdk/modeling.py)从 `models.py` 类型标注生成包 schema。
- [扩展示例](reference/extension_templates.py)：`PlainAgent` 和 Python 工具示例；不包含后端。
- [数据集示例](reference/datasets)：九个包都包含 `dataset.yaml`、`pyproject.toml`、`src/<package>/models.py`、三个专属入口和本地测试，共 27 个直接子类。
- [公开运行示例](reference/runs)：独立于数据集包的 `run.yaml`，只含公开参数，不含 `schema_ref`。
- [生成夹具](reference/generated)：发布后 manifest、包 schema、批次提交 batch_request.json 和 ExecutionPlan；run_spec.json、episode_request.json 是批次内容的独立阅读视图，用户不编辑这些文件。
- [有状态环境示例](reference/examples/counter_environment.py)：演示统一 `Observation.content`、有状态动作以及独立的评分状态读取。
- [机器可校验契约](contracts/uenv.schema.json)：当前本地参考生成物。公共字段已以 `contracts/proto/uenv/v1/*.proto` 为唯一可编辑来源，生成 Rust/Python 类型、JSON schema 和字段字典；数据集新增业务字段只在包内 `models.py` 定义，由发布工具生成包 schema。

工具参考已统一到 Rust `AgentRuntime.step`：`RunSpec.tools` 选择普通工具或 Agent 包内原生导出，原生格式由 [NativeToolAdapter](reference/sdk/src/uenv/sdk/tools.py)转换，原执行器保留。`Environment` 不再实现动作解析/执行，九个数据集与状态工具示例已同步。OpenHands 1.15 真实会话、MCP HTTP 服务与 Host RPC 已接通；复现命令和验证范围见[真实 Agent 接入](docs/development/reference_implementation.md#12-真实-agentmcp-与跨进程-rpc)。

后续不引入 Agent 池。Agent 由 Worker 为当前 attempt 管理；模型服务端点来自最终 `ExecutionPlan.model`，Agent 不加载本地模型权重。Process、Docker、Podman 的目标驱动统一实现 Rust `Backend` trait；参考另含有限命令后端实现与 Linux 专用测试，验证范围见验证记录；Environment 不为不同后端编写组合适配器。

## 谁需要运行哪些命令

普通数据集作者不需要运行 `scripts/build_contracts.py`、`scripts/build_examples.py` 或 `scripts/build_module_map.py`。这些是维护本设计包的开发命令。普通作者使用产品化后的 `uenv package init/validate/test/publish`；该 CLI 仍是目标设计，尚未在远端生产系统实现。

设计维护者从本仓库根目录（即原工程的 `design` 目录）运行；需要 Python 3.11+ 和能够编译本参考项目的 Rust 工具链。首次在新机器上验证前，先下载 Cargo.lock 中锁定的依赖：

```text
cargo fetch --locked
```

然后运行：

```text
python -m pip install --target .dependencies -r scripts/requirements.txt
python scripts/build_contracts.py
python scripts/build_examples.py
python scripts/build_module_map.py
python scripts/validate_design.py
```

`scripts/validate_design.py` 会离线运行 Rust 控制测试和 Python 契约/评分规则测试。当前测试使用合成数据和脚本模型响应，覆盖独立 Host 进程以及内存/本地持久文件存储；本机检查不执行 Linux 专用测试；有限后端命令实现及历史 Linux 测试范围见验证记录。真实 OpenHands/MCP 仅覆盖验证记录中的场景；本地事务、重启恢复和 Host RPC 已有测试，官方 benchmark、完整后端角色隔离与生产部署仍未验收。

轨迹采集复用相同执行链：purpose=trajectory_collection，scoring.enabled=false 表示不评分。参见[主方案第 8.2 节](docs/uenv_design.md#82-评测训练与轨迹采集)、[用户指南第 2.6 节](docs/guides/user_guide.md#26-轨迹采集怎样配置)，以及[只采集](reference/runs/gsm8k_collection.yaml)和[采集并评分](reference/runs/gsm8k_collection_scored.yaml)配置。对应生成请求和计划位于 reference/generated/episodes 下的同名示例目录。
