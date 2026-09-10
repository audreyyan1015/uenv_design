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

Python 中原有的 `EpisodeRuntime`、计划解析器、工具权威路由和 `SandboxBackend` 模板已经删除，避免系统控制职责在两种语言各实现一遍。Python Scorer 只填写 `success`、`metrics`、`reward`、`evidence`；Rust Worker 补全 `status`、`scorer`、`error`。

本仓库根目录对应原工程的 `design` 目录，包含设计文档、参考代码、生成契约和示例。文档中的生产源码位置用于说明审查依据；生产源码快照及原工程其他文件不包含在本仓库中。

## 目录结构

```text
design/
├── README.md              # 仓库入口与运行说明
├── docs/                  # 设计方案、用户指南、字段规范、重构计划与验证记录
├── scripts/               # 生成与验证工具，以及 requirements.txt
├── contracts/             # 生成的公共协议和内置组件 JSON Schema
├── reference/             # Python SDK、数据集、智能体与工具参考示例
├── reference-control/     # Rust 控制流程参考代码与测试
├── Cargo.toml             # Rust workspace 构建入口
├── Cargo.lock             # Rust 依赖版本锁定
├── .gitignore             # Git 忽略规则
└── .gitattributes          # 文本换行规则
```

文档中的代码路径以仓库根目录为基准，点击链接则按所在文档解析。`docs/module_map.md` 描述未来生产系统的目录规划；上面列出的是当前设计仓库的实际组织。维护命令统一从仓库根目录运行。

## 阅读顺序

| 文档 | 内容 |
|---|---|
| [完整 UEnv 设计方案](docs/uenv_design.md) | 按目标、用户输入输出、部署、执行流程、数据、扩展、后端、评分轨迹、Hub、恢复组织的十章主方案 |
| [当前功能与处置清单](docs/current_capabilities.md) | 基于当前源码判断保留、修改、重写与删除 |
| [源码重构计划](docs/source_refactoring_plan.md) | 从当前源码迁移到目标架构的阶段、保护措施、回退和删除条件 |
| [目标代码组织](docs/module_map.md) | Bridge、Server、Worker、Python 组件 host、SDK、Hub 的逐文件职责 |
| [字段与配置审计](docs/configuration_audit.md) | 一个含义一个字段名、一个执行配置一个生效来源 |
| [核心维护者字段字典](docs/field_dictionary.md) | 当前参考生成器导出的完整内部协议与扩展 schema；普通用户无需阅读 |
| [字段统一规则](docs/field_conventions.md) | 字段命名、归属、转换和校验边界 |
| [用户手册与模板](docs/user_guide.md) | 新增数据集、自定义 Agent 和工具 |
| [数据集统一包模板](docs/dataset_package_template.md) | 所有数据集使用同一包结构，并声明自己的 Adapter、Environment、Scorer 三个职责入口 |
| [参考实现说明](docs/reference_sdk_vnext3.md) | 参考实现边界、待决事项与内部进程接入图 |
| [验证结果](docs/verification.md) | 已验证内容与未验证边界 |
| [逐条问题答复](docs/responses.md) | 前期问题的简明答复 |

## 参考代码

- [Rust 控制参考](reference-control/src/lib.rs)：以 DispatchRequest 为唯一执行入口，配置只读其中的 ExecutionPlan，并统一处理预算、工具、轨迹、评分与清理。
- [Python SDK](reference/sdk/src/uenv/sdk/__init__.py)：用户可实现的接口和公共数据对象；[类型模型](reference/sdk/src/uenv/sdk/modeling.py)从 `models.py` 类型标注生成包 schema。
- [扩展示例](reference/extension_templates.py)：`PlainAgent` 和 Python 工具示例；不包含后端。
- [数据集示例](reference/datasets)：九个包都包含 `dataset.yaml`、`pyproject.toml`、`src/<package>/models.py`、三个专属入口和本地测试，共 27 个直接子类。
- [公开运行示例](reference/runs)：独立于数据集包的 `run.yaml`，只含公开参数，不含 `schema_ref`。
- [生成夹具](reference/generated)：发布后 manifest、包 schema、内部请求、RunSpec 和 ExecutionPlan；用户不编辑这些文件。
- [有状态环境示例](reference/examples/counter_environment.py)：说明 `Observation`、`Transition` 和 `Outcome`。
- [机器可校验契约](contracts/uenv.schema.json)：当前本地参考生成物。目标生产协议以 `contracts/proto/uenv/v1/*.proto` 为唯一可编辑来源，并生成 Rust/Python 类型、JSON schema 和字段字典；数据集新增业务字段只在包内 `models.py` 定义，由发布工具生成包 schema。

后续不引入 Agent 池。Agent 由 Worker 为当前 attempt 管理；模型服务端点来自最终 `ExecutionPlan.model`，Agent 不加载本地模型权重。Process、Docker、Podman 是 Rust `Backend` trait 的实现；Environment 不为不同后端编写组合适配器。

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

`scripts/validate_design.py` 会离线运行 Rust 控制测试和 Python 契约/评分规则测试。当前参考使用合成数据、模拟模型与内存产物存储；它没有完成真实 Docker/Podman/Process 驱动、OpenHands、MCP、官方 benchmark、RPC、持久化或生产部署。
