# UEnv 用户手册规划与模板说明

本手册定义目标产品体验；标为“目标”的 CLI/SDK 尚未部署。reference 是本次可在本地执行的接口样例，不能拿其中占位镜像/模型地址作为可运行生产配置。

## 1. 普通用户实际填写什么

数据集作者只填写 `dataset.yaml`、`models.py` 和 Adapter/Environment/Scorer 中的业务逻辑。运行用户只填写严格的 `RunSpec`，文件形式是 `run.yaml`，其中固定分组选择 Environment、Agent、Backend、Model、Tools、Scorer 和 Limits。更换智能体或 Docker/Podman 只修改 RunSpec，不编辑数据集文件。

TaskSpec、PackageManifest 由准备和发布工具根据上述输入生成；EpisodeRequest、ExecutionPlan、episode_id、attempt_id、lease、input_digest、plan_digest 等属于系统内部协议。普通用户的模板、默认 CLI 输出和入门手册不展示这些内部字段，也不要求用户填写。核心维护文档仍完整定义它们，以便实现 Server、Worker、恢复和审计。

样本有两种输入方式：用户提供原始/标准化数据，或指定 Hub 的数据 id/revision/split/subset 与 sample_id/样本范围。两者在 prepare/Bridge 准备入口统一，不能同时提供 Hub 引用和另一份样本内容让系统合并。默认标准化文件是 UTF-8 JSONL，每行只保存 task 和可选 private_data，不保存运行配置；含评分材料的文件不交给 Agent。Hub 的存储内容、文件格式与 Worker 读取规则统一见 [第 3.4 节](uenv_design.md#34-hub-与数据存储规范)，当前实现差异见第 13.2 节。

## 2. 新增数据集需要哪些文件

| 用户操作 | 必需文件/变化 | 核心服务是否修改 |
|---|---|---|
| 使用已支持数据集的新样本 | 原始数据文件，运行配置 | 否 |
| 新增数据集，复用现有逻辑 | dataset_adapter.py、environment.py、scorer.py 三个专属类；声明入口与样例；公共逻辑通过函数或组合复用 | 否 |
| 新增复杂转换或评分规则 | 在本数据集对应专属类覆盖方法并补业务案例；新类型仅在包内定义一次 | 否 |
| 新增交互环境 | environment.py、models.py 中新增的 action/observation/config 类型、生命周期测试 | 否 |
| 新增 Agent | runner.py、AgentManifest、参数声明和测试；只声明 supported_interfaces/required_tool_names，不逐个列工具 | 否 |
| 新增工具 | 可选 tools.py 或独立工具包：Python 函数/ToolExecutor、ToolSpec、类型声明和业务测试；复用已有接口时不修改 Agent | 否 |
| 新增平台后端 | 平台 backend 驱动、配置 schema、兼容性/清理测试及 Worker 部署 | 是，属于平台扩展而非普通数据集接入 |

目标 package init 必须生成三个专属类及对应入口文件；命名遵循 snake_case/PascalCase。辅助文件按需增加，三个入口不能省略或改成共享类的 import 别名。所有数据集使用同一个包模板，详细设计见 [数据集统一包模板](dataset_package_template.md)。

## 3. 统一包模板

```text
my_task/
  dataset.yaml           # 用户唯一编辑的本地发布声明
  pyproject.toml         # 本地 Python 构建输入
  src/my_task/
    __init__.py          # 构建进版本化 Python wheel
    models.py            # 按需：本包新增的业务字段模型
    dataset_adapter.py           # MyTaskAdapter
    environment.py       # MyTaskEnvironment
    scorer.py            # MyTaskScorer
```

这是所有数据集共用的目标作者模板，产品侧脚手架尚未实现。每个数据集的三个入口类必须明确声明并继承系统对应基类；Adapter 与 Environment 直接继承系统基类并实现 normalize、reset；所有 Scorer 直接继承 Scorer 并实现单条 episode 的 score，公共评分行为通过函数或组合复用。共享实现放入系统公共模块，manifest 指向本数据集类，不用导入别名替代专属类。tools.py 和本地 tests/ 按需增加，不要求名为 cases.jsonl 或 test_contract.py 的文件。普通运行统计由系统根据已保存结果计算；需要联合多条结果的特殊报表由评测框架或分析程序计算，不增加数据集入口。

发布时，`src/` 构建为一个版本化 Python wheel；发布工具解析 `dataset.yaml`，从 dataset.yaml.models 登记的 Python 类型及其嵌套类型自动生成校验 schema，并结合 wheel/schema digest 组装 PackageManifest API 对象。Hub 把 PackageManifest 存为结构化数据库记录，查询时返回 JSON，不要求用户生成或维护 manifest.json 文件。wheel、生成 schema 和资源字节保存在 Hub 管理的文件存储。`dataset.yaml` 与 `pyproject.toml` 是本地构建输入，`tests/` 默认不上传。用户不创建或编辑 `schemas/` 目录；系统类型直接使用 SDK 生成类，数据集业务字段只在本包 `models.py` 定义一次。manifest 的 `provided_tools` 只登记这个包能提供哪些工具，本次实际启用项只写在 `RunSpec.tools`。脚手架默认在 dataset.yaml 生成 `internet_access: false`；环境确实需要公共互联网时才改为 true，并在发布时经过平台准入检查。操作系统和容器安全参数由平台处理。通用契约测试由工具提供，特殊行为补充 Python 测试。

UEnv 的 TaskSpec、RunSpec、EpisodeRequest、ExecutionPlan、ScoreResult、轨迹和 Hub/Server/Worker RPC 都只在 `contracts/proto/uenv/v1/*.proto` 定义。Rust/Python 类型、RPC stub、核心 JSON schema 和字段文档从 proto 生成，不手写第二份。`models.py` 只描述某个包独有的题目、私有评分材料、配置或动作字段，因此新增数据集不用修改 UEnv 核心 proto。两者的完整关系见[数据集包模板第 4 节](dataset_package_template.md#4-schema-与-proto-的唯一来源)。

### 3.1 字段设计直接采用 HUD 与 Harbor 各自合适的部分

- 数据集字段采用 HUD 的方式：作者在 Python `models.py` 中用类型标注定义，发布工具自动生成 schema。作者只看到 `PreparedSample`、自己的模型和三个入口方法，不浏览完整系统协议。
- 运行配置采用 Harbor 的方式：固定为严格的 RunSpec，按 Environment、Agent、Backend、Model、Tools 和 Limits 分组；Scorer 作为同级必选配置。CLI 在提交前完成类型、必填项和组合兼容性校验。
- 包声明不提供 metadata、display_name、description、tags；这些字段当前没有用户功能，不作为扩展配置保留。
- `uenv package validate` 对确定重复的系统控制字段和已登记常见别名直接报错；对疑似同义名称给出警告。例如 `timeout` 可以明确提示改用 `RunSpec.limits.total_timeout_ms`，但程序无法百分之百判断 `allowed_time` 是否表达同一含义，不能自动改名或声称已经消除全部语义重复。

普通用户只查询当前需要填写的类型：

```text
uenv describe PreparedSample
uenv describe RunSpec
uenv describe my-org/my-dataset@1.0.0:MyInput
uenv package validate ./my_task
```

`PreparedSample` 视图面向数据集作者；`RunSpec` 视图面向运行用户；包类型视图只展开本包业务字段。默认视图不列出 EpisodeRequest、ExecutionPlan、episode_id、attempt_id、lease、digest 或 `TypedConfig.schema_ref`。IDE 类型提示和 CLI 从同一类型定义生成，不维护另一份字段清单。完整协议只出现在核心维护文档和生成物中。

`run.yaml` 中各组件的 `config` 直接写该组件公开的参数。例如先选择 PlainAgent，再在它的 config 下填写 system_prompt；模型调用次数只在 limits.max_generations 中设置，CLI 根据 implementation 查询对应 config schema，校验后自动生成内部类型信封。运行用户也不填写 config 的 schema_ref。`run.yaml`、CLI 参数和环境变量不能同时提供同一个执行配置；目标 CLI 只接受这一个文件或等价的 SDK RunSpec 对象。

数据集作者只需判断：题目、状态、动作和私有评分材料写进 `models.py`；Agent、Backend、Model、Tools、Scorer、预算等执行选择只写进 RunSpec。已有 `ArtifactRef`、`ContentPart`、`EvaluationPlan` 等公共值类型直接 import。需要改变所有数据集共有执行行为的新字段必须修改核心协议，不能藏在 input、config 或 metadata 中。

上述原则来自 [HUD 环境指南](https://www.hud.ai/resources/rl-environments-what-they-are-how-to-build) 的 Python 类型定义方式，以及 [Harbor Task Structure](https://www.harborframework.com/docs/tasks) 和 [Harbor Agents](https://www.harborframework.com/docs/agents) 的严格分组配置和提前校验。UEnv 不照搬 HUD 将任务与评分合并在一个生成器中的结构，也不允许 Harbor 式自由 metadata 参与执行。

数据集声明不包含默认 backend/agent。`run.yaml` 属于训练、评测或实验项目，不放在数据集包模板中，也不是必需文件；需要使用文件配置时，它提供用户选择的 agent/backend/model/tools/预算，提交时展开为完整 RunSpec。调用方也可以直接构造 RunSpec。依赖有明确版本，包发布后内容不可变；修正一个评分函数也要发新版本。当前 `reference/datasets` 的九个示例已经采用上述包布局，公开运行配置位于 `reference/runs`，系统生成对象位于 `reference/generated`。

`purpose=training` 时必须填写 `training`，`purpose=evaluation` 时必须省略它。purpose 只决定结果交给训练器还是评测汇总器；training 只描述训练所需的版本和 token 轨迹条件，不会选择另一套 Scorer 或 reward。

## 4. 数据转换

DatasetAdapter 只把源字段放到标准位置，并将答案/测试放入可选 private_data，和公开输入一同返回。业务字段先在 `models.py` 定义一次：

```python
from uenv.sdk import UEnvModel

class MyInput(UEnvModel):
    instruction: str

class MyPrivateData(UEnvModel):
    answer: str
```

Adapter 直接返回这些模型；发布工具和 SDK 负责生成、引用并校验 schema，用户不再手写 `typed("MyInput", {...})` 信封：

```python
class MyAdapter(DatasetAdapter):
    def normalize(self, row):
        return PreparedSample(
            sample_id=str(row["id"]),
            input=MyInput(instruction=row["question"]),
            private_data=MyPrivateData(answer=row["answer"]),
        )
```

prepare 工具负责数据集 revision、task_id 和 input_digest。SDK 在进程边界自动把 Python 模型封装为 TypedConfig 并填入已发布的 schema_ref；这个信封是内部传输格式，不是用户编程接口。prepare 用它生成公开 task.json；task 与 private_data 配对进入受控 episode_request.json。系统不再为评分依据生成额外包装文件。无参考依据时省略 private_data；大型测试内容通过其内部的 ArtifactRef 传递。

源数据无 ID 时，Adapter 应从规范化后的公开输入计算稳定内容摘要作为 sample_id；完全相同但必须区分的重复行，数据作者需要在源数据中补稳定 ID，prepare 会拒绝重复 sample_id。不能依赖临时行号、文件绝对路径或随机值。PubMedQA 的外层 JSON key 在导入时作为 id，OlymMATH 的语言/难度从原记录或明确的数据发布配置写入，不能由 Worker 猜测。配对正确性由受信 Adapter/prepare 负责；同类型材料并不意味着适用于同一道题。

Worker 在执行开始前校验 private_data 的已注册 schema，并在交互阶段保持隔离；得到最终 Outcome 后才将它放入 ScoreInput。Environment 和 Agent 只接收公开 TaskSpec；隐藏测试的读取能力只提供给评分器。整个 episode_request.json 含有私有数据，不能作为公共任务文件给 Agent 或写入公开日志。本地验证的是接口与数据副本边界，生产进程和存储隔离仍需实现。

已有原始脚本到新字段的映射（“私有字段”指 episode_request.json 中 private_data.data 的字段）：

| 数据集 | 原始身份 | 模型可见 input.data | 私有字段 |
|---|---|---|---|
| GSM8K | 稳定行 ID | question -> instruction | answer（从参考解的 #### 后提取最终答案） |
| PubMedQA | JSON 外层 key | QUESTION -> instruction；CONTEXTS -> contexts[] | final_decision -> answer |
| SciTab | id | claim/paper/paper_id/table_id；table_caption -> caption；table_column_names -> columns[]；table_content_values -> rows[][] | label -> answer |
| OlymMATH | unique_id | problem -> instruction、language、difficulty、subject | answer |
| DSCodeBench | problem_id | code_problem -> instruction、library、entry_point | tests artifact、reference_solution（按需）、num_tests、test_seed、evaluation_plan |
| SWE Verified/Lite/Pro/Smith | instance_id | problem_statement -> instruction、repo、base_commit、workspace_path、runtime_assets[] | test_patch、reference_patch（按需）、FAIL_TO_PASS/PASS_TO_PASS、evaluation_plan（其中选择 harness） |

Code/SWE 原始数据中的测试路径、镜像标签、测试命令先由包的 prepare/build 逻辑转换为有 digest 的产物和类型化 EvaluationPlan。不能把本机绝对路径当成跨机器稳定输入。SWE 变体身份只来自所选数据集包；原始记录中的 `benchmark_variant` 只用于 Adapter 校验数据是否进了正确的包，不再复制为公开输入中的第二个 `variant` 字段。SWE 的具体初始化与官方测试准备仍属于各包 Environment/Scorer，本次参考样例没有实现这些完整操作。

OlymMATH easy/hard 和 en/zh 用字段组合表示；它们共享同一个 adapter/environment/scorer。字段字典把每一层数组和对象都展开，不以“JSON，任意内容”代替定义。

## 5. 编写 Python 评分器

参考 SDK 已实现 `score(ScoreInput, ScoringContext) -> ScoreResult`。ScoreInput 只有 task、最终 outcome、评分前 trajectory_ref 和可选 private_data；它不接收 evaluation/training 用途、step/final 阶段或另一份超时配置。当前剩余时间和取消状态只由 ScoringContext 提供。Scorer 填 success、metrics、reward 和可选 evidence；Rust Worker 补全 status、scorer、error。评分器不能填写这些系统字段。

`RunSpec.scorer` 是用户选择；Server 将它锁定为 `ExecutionPlan.scorer`，Worker 每个得到最终 Outcome 的 attempt 至多调用后者一次。Server 只接纳一个 attempt 的 `EpisodeResult.score` 作为 episode 权威评分。进入评分前失败没有 score；一旦调用，status=ok 或 status=error 的结果都保留。评分成功必须有 reward：评测展示或聚合这个 ScoreResult，后训练复用同一个 reward 和模型轨迹。运行成功但答错时 status=ok、success=false、reward=0；评分程序出错时 status=error、success/reward=null。

DSCodeBench/SWE Scorer 通过 `ScoringContext.run_harness()` 请求 Worker 在配置的后端执行评测，而非直接在 Agent 中算分。harness 正常完成但候选程序错了，属于合法评分失败；harness 缺依赖/崩溃属于评分系统错误。

`reference/shared/src/uenv_reference_rules` 提供四种文本评分规则函数；九个数据集 Scorer 全部直接继承 Scorer。公共 ScoreInput 不要求输出为字符串，也不强制有标准答案；需要文本和参考材料的规则显式调用 read_reference_text。Scorer 必须明确返回本条 episode 的 reward；`ScoreResult.binary()` 同时生成二元 success、accuracy 和 reward。ScoringContext 支持受控 read_artifact 与 run_harness；调用示例和字段变更见 [参考 SDK vNext.3](reference_sdk_vnext3.md)。目前仍是本地参考包，不是生产部署。

注意：reference-corrected-v1 对 OlymMATH 的未知 LaTeX 命令归一化做了显式修复，避免 sqrt(33) 匹配 33。当前参考实现仍需完整官方语料对照，不能以几个单元案例宣称官方评分等价。

## 6. 自定义智能体

设计约束：后续不引入 Agent 池。RunSpec.agent 只指定 implementation 和 config，不提供 pool_id、agent_pool_id 或 placement。Agent 由本次任务的 Worker 管理；用户不注册 Agent 池，也不配置独立 Agent 容量或调度。模型服务端点通过运行配置由 Bridge 传入，Agent 不加载模型权重。

实现异步 `AgentRunner.run(context)`。context 只提供公开 TaskSpec、观测、获准工具描述和 `generate/call_tool/step` 能力；三个操作统一使用 `await`，另有只读 seed。不同 Agent 的模型消息由其自行构造。模型调用必须通过 Worker 的 AgentRuntime/ModelProvider，以便在实际调用前强制预算并保留真实生成信息；Agent 不直连端点。Agent 不接收 private_data，也不能直接写 ScoreResult。

参考 extension_templates.py 中的 PlainAgent 是无工具单轮例。正式 PlainAgent 还提供明确的历史策略与工具循环；OpenHandsAdapter 将 SDK Conversation 事件转换成同样的模型、工具和终态记录。用户可以让数学选择 OpenHands，或让仓库修复选择 PlainAgent+工具，只要能力满足。

**自定义 Agent 不等于自定义工具，也不需要逐个编写工具适配。**AgentManifest 只填写实现入口、配置 schema、有序 supported_interfaces 和 required_tool_names。自己写 Agent 循环时，获取 UEnv 提供的获准工具描述，通过 call_tool 调用；基于已支持框架时，复用 UEnv 的框架接入或该框架的 MCP 支持。只有引入未支持的工具接口或框架专属语义时，才需补一次接口适配；普通工具可复用这份接入。

## 7. 自定义工具

用户只写一份 Python 工具：在数据集可选 tools.py 或独立工具包中定义函数、参数/返回类型和说明；发布工具生成或校验 ToolSpec 的 entrypoint、config_schema、input/output schema 和 interfaces。需要状态或资源时实现 ToolExecutor。

运行用户在 RunSpec.tools 选择工具及配置，不填写 adapter。PlanResolver 按 AgentManifest.supported_interfaces 的顺序，从 ToolSpec.interfaces 选择第一个共同接口，再锁定 interface/adapter。UEnv 据此直接注册或包装成 MCP；自定义 Agent 不逐个适配工具。Agent 自带工具也发布为 ToolSpec 并进入同一列表，由 UEnv 限制执行并记录参数和结果。选择工具只开放该工具接口，不会开放任意网络或宿主目录。

代码示例和调用图统一见 [工具设计第 4.1 节](uenv_design.md#41-工具怎么写怎么用怎么管)。上述为目标用法；当前参考格式、尚未实现的接口和验收要求集中见 [迁移说明第 13.1 节](uenv_design.md#131-工具调用迁移说明)。

## 8. 指定后端和智能体

在 run.yaml 中分别设置 agent、backend、tools。Docker 与 Podman 可使用相同兼容镜像；更换引擎只改 backend 实现与对应参数。Process 使用 runtime_profile，不能直接照搬 OCI image 参数。预检给出具体缺失能力/依赖，不按数据集名字决定是否可用。

Environment 包作者只回答一个问题：这个 Environment 是否需要访问公共互联网。环境包用 `internet_access: true | false` 表达；false 表示不需要，true 表示需要。运行用户通过 RunSpec 选择 Environment，不再填写第二个网络开关。文件和进程怎样隔离、容器怎样限制、内部控制通道怎样连接，都由平台自动处理。即使设为 true，也只能使用平台提供的受控互联网连接，不能接触其他任务或评分私有材料。Server 将这个值锁定为 `ExecutionPlan.internet_access`，Backend 无法满足时直接说明原因并拒绝任务。RunSpec、数据行、Agent、Tool 和 backend config 都不能改写它。包作者填写 required_capabilities 声明已登记的运行需求，无需求时为 []；系统汇总为调度信息，运行用户不在 RunSpec 再填一份。

```text
# 以下为目标 CLI 示例
uenv run --tasks tasks.jsonl --config run.yaml
```

`run.yaml` 是 Environment、Agent、Backend、Model、Tools、Scorer 和预算的唯一用户配置入口；CLI 不再提供这些字段的覆盖参数。要重用既有题目，仅改这份运行配置。不要编辑 dataset adapter 来改变 Agent，也不要在 scorer 里写 docker/podman 选择。

### 镜像填写位置与优先级

下面字段已写入本地 vNext.3 schema，真实后端接入仍属目标实现：

| 谁指定 | 位置 | 用途 |
|---|---|---|
| 数据集作者 | dataset.yaml 的 runtime.image，构建为 PackageManifest.runtime.image | 数据集默认镜像，可引用公共镜像 |
| Adapter/prepare | TaskSpec.runtime.image | 样本专用镜像，例如 SWE 每条实例不同 |
| 运行用户 | RunSpec.runtime.image | 本次 run 下所有任务的显式镜像覆盖 |
| 系统生成 | ExecutionPlan.runtime.image 和 image_source | 当前 episode 最终使用的 digest 镜像及其来源 |

优先级为 **用户覆盖 > 样本镜像 > 数据集默认镜像**。缺镜像报错，显式覆盖不兼容也报错，不自动回退。同一个 RunSpec 下可以有不同样本镜像，最终结果分别写入各自 ExecutionPlan，不改写共享 RunSpec。选择 Process 时使用本机 runtime_profile，不能同时显式设置 image。

QA 选 Docker 也实际使用环境容器；数据集可以引用公共 Python 运行镜像，无需单独构建。SWE 的实例镜像由 Adapter 从源字段转换，用户通常不需要逐条填写。目标字段片段与兼容性要求见 [主方案第 6.1—6.5 节](uenv_design.md)。

本地 schema 和九个示例已统一使用 runtime.image，旧 backend.config.data.image 被拒绝。每个数据集对应的 `reference/generated/episodes/<dataset>/execution_plan.json` 展示最终生效配置，Rust `PlanResolver` 验证优先级与冲突；Python `fixture_plan.py` 只生成稳定示例，不执行真实镜像拉取或容器创建。样本镜像由 Adapter 返回 PreparedSample.runtime，再由 prepare 写入 TaskSpec.runtime。

## 9. 设计维护者验证参考包

普通数据集作者不运行下面这些脚本，也不修改它们。它们只供 UEnv 设计/核心维护者从 `design` 目录重新生成中央参考契约、内置示例和模块图：

```text
python build_contracts.py
python build_examples.py
python build_module_map.py
python validate_design.py
```

校验依赖 jsonschema 和 PyYAML 安装于本次 `design/.dependencies`，验证脚本只在当前进程增加该路径。九个 `reference/datasets/<dataset>` 子目录采用完全相同的作者结构：`dataset.yaml`、`pyproject.toml`、`src/<package>/{__init__,models,dataset_adapter,environment,scorer}.py`、`tests/cases.jsonl` 和 `tests/test_contract.py`。需要数据集专用工具时才增加 `tools.py`。

公开运行配置统一位于 `reference/runs/<dataset>.yaml`。`manifest.json` 和包 schema 由发布输入生成到 `reference/generated/packages/<dataset>`；TaskSpec、EpisodeRequest、RunSpec 与 ExecutionPlan 示例生成到 `reference/generated/episodes/<dataset>`。这些都是设计夹具，普通用户不编辑，也不放回数据集作者目录。真实接入由 prepare、请求构建和计划解析处理。

示例中的任务文本、测试 artifact、镜像和模型端点是明确的合成/占位内容。可执行部分覆盖输入转换、提示生成、四个 Python 文本评分器、评分错误边界和 schema 验证。未运行真实容器、OpenHands、DSCodeBench 或 SWE 官方 harness。代码类 scorer 没有 Worker harness 时返回结构化错误，不返回虚构的成功分数。

生产发布前还要补齐每个包的完整资源准备、官方评测依赖及真实样本对照；这些是设计中列出的迁移工作。这样用户既能看到同一模板如何表达已有数据集，也能清楚区分参考示例与已完成上线的功能。

## 统一字段约定

同一业务含义沿用同一字段名，规范输入不接受源数据别名；完整规则见 [字段统一规则](field_conventions.md)。字段重命名应先修改规范源，再同步示例与校验，不在 Worker 增加兼容分支。

正式测试的唯一选择位于 private_data.data.evaluation_plan.harness。EvaluationPlan 直接内嵌在私有输入中，不使用另一个文件引用；Scorer 不硬编码 harness 名字。回调接收 HarnessRequest，其中 outcome 是完整 Outcome，private_data 沿用原对象结构，remaining_timeout_ms 是推导后的预算。


## 配置如何进入执行

运行用户只提交 RunSpec 和准备好的任务；SDK 在内部构造 EpisodeRequest 并配对可选 private_data。系统生成 ExecutionPlan，Rust Worker 的 EpisodeSupervisor 只执行该 plan，并通过受管 Python host 调用 Environment、AgentRunner、Scorer 和工具。用户不填写 EpisodeRequest、ExecutionPlan 或其中的内部身份与摘要；变更配置应生成新的 RunSpec 和计划。

用户接触的扩展接口统一使用 Python。调度、租约、attempt 状态、预算、取消、Process/Docker/Podman、工具准入、评分调用顺序、轨迹封存和结果重传由 Rust 服务实现，用户无需了解或修改 Rust。Python 扩展返回业务数据，Rust 系统依据同一公共契约执行最终校验。

新增数据集仍只实现本包 Adapter、Environment、Scorer；有新业务字段时在 models.py 定义一次。发布工具生成 schema 和 manifest 中的类型引用。组件工厂由宿主按锁定引用加载，接收该角色的 TypedConfig；工厂必须验证所支持的配置值，不能从另一个文件或环境变量覆盖本次执行参数。模型端点由 Bridge 的模型配置进入计划，Agent 通过受控模型入口使用它。

本地 Rust 参考覆盖完整标准轨迹、每个已评分 attempt 的单次评分、固定计划和预算入口；未实现的生产能力在验证文档中逐项列出。轨迹统一返回指向 TrajectoryManifest 的 ArtifactRef：scoring_checkpoint 用于评分，final_complete/final_partial 用于最终读取。用户只配置 trajectory_retention_days，无需配置轨迹内容级别或 max_inline_bytes。新增数据集无需修改 Rust 控制代码或任何 build_*.py 文件。


## Agent 与 Environment 共用的 Outcome

Agent 返回 Outcome(final_answer=..., artifacts=..., termination_reason=...)；state 留空，由 Environment.finalize 收集。finalize 输入和输出均为 Outcome，不能再导入 AgentOutcome 或 FrozenOutcome。默认结束原因是 final_answer；正式评分只接受 finalize 产生的终态 Outcome，不能使用 termination_reason=in_progress 的快照。系统取消或失败不填写 termination_reason，分别读取 EpisodeResult.execution_status 与 error.code；EpisodeResult 不再复制交互结束原因。

Environment.reset 返回 Observation，step 返回包含 observation 的 Transition。EnvironmentContext.deadline/seed 是只读的计划派生值，用户实现不得修改。

Environment 只写任务规则，Backend 只决定任务在哪里运行。二者没有继承关系，也不按数据集名称绑定。同一个 Environment 可以与 Process、Docker 或 Podman 组合；作者不编写 `Gsm8kDockerAdapter`、`SwePodmanAdapter` 之类的组合代码。Environment 需要文件或命令时只使用 EnvironmentContext 提供的统一操作入口，UEnv 将这些操作交给本次计划已经选定的 Backend。若后端缺少所需依赖或能力，系统在执行前报告组合不兼容，不要求作者补适配代码，也不偷偷换后端。DatasetAdapter 只负责转换数据，与后端适配无关。

## 交互数据与自动轨迹

作者只需要掌握 Observation、Transition、Outcome 三个返回类型。问答任务返回初始 Observation，由 Agent 提交 Outcome；需要环境动作的任务再实现 step 并返回 Transition。不要求作者填写 episode_id、时间戳、动作序号，或手工构造 EnvironmentTransition/TrajectoryEvent。

Runtime 自动记录每次动作。目标轨迹内容为 environment_step_index、observation_before、action、transition，其中 transition 直接保存已经校验的 Transition 副本。读取动作后观测使用 payload.transition.observation，结束标志和环境奖励也从该 transition 读取；不在轨迹顶层再保存同一组字段。

当前生成 schema 和 Rust 参考都已经使用嵌套 `transition`，不再接受平铺的动作后字段。三个用户返回类型和方法无需额外变化；读轨迹的程序只按当前协议读取，不同时猜测两套字段。完整的事件类型、写入、封存、上传和读取规则见[主设计 4.2 节](uenv_design.md#42-轨迹谁记录记录什么如何保存)。
