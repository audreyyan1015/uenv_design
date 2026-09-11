from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
MODULES={
"contracts/proto/uenv/v1": {
".":[
("common.proto","ArtifactRef / ErrorRecord / TypedConfig","跨领域公共值与动态业务数据的固定信封；字段约束也在 proto 注解中定义"),
("task.proto","DatasetRef / RuntimeSpec / TaskSpec","公开任务和数据身份"),
("run.proto","ComponentSpec / ScoringSpec / BackendSpec / ModelSpec / Limits / RunSpec","用户运行配置；每个选择只定义一次"),
("interaction.proto","ContentPart / Message / Observation","Agent、Environment 与模型交互对象"),
("scoring.proto","ScoreInput / ScoreResult","单条 episode 的统一评分输入和结果"),
("trajectory.proto","TrajectoryEvent / TrajectoryManifest","所有 Agent 和数据集共用的事件与分片清单"),
("execution.proto","EpisodeRequest / BatchRequest / ExecutionPlan / DispatchRequest / EpisodeResult","从提交、解析、派发到最终结果的系统对象"),
("package.proto","PackageManifest / AgentManifest / ToolSpec","Hub 发布、解析和组件运行要求"),
("server_service.proto","EpisodeService / AdminService","Bridge 与 Server 的 RPC"),
("worker_service.proto","WorkerService / ControlPlaneService","Server 与 Worker 的派发、取消、注册和心跳 RPC"),
("hub_service.proto","HubService","包、数据 revision、schema 和 artifact 的 Hub RPC")]
},
"python/uenv_bridge/src/uenv_bridge": {
"domain": [
("sample.py","FrameworkSample / SampleIdentity","保存框架样本与 episode 的身份关系，不存业务题目别名"),
("ports.py","EpisodeClient / ModelProvider","声明对 Server 与框架推理服务的窄接口")],
"application": [
("bridge_service.py","BridgeService","共用契约驱动的配置补值和校验；将一份 RunSpec 与多条 EpisodeRequest 作为 BatchRequest 一次提交并收集结果"),
("sample_preparation.py","prepare_samples","读取选定来源；原始行调用数据集包内 Adapter.normalize，标准化行直接校验；只组装 task/private_data，不解释数据集字段"),
("request_builder.py","build_episode_request / build_batch_request","从 TaskSpec、种子和配对 private_data 构造成员；与一份 RunSpec 组装 BatchRequest，配置只在 run_spec，不进入成员"),
("result_collector.py","ResultCollector","按身份收集、取消和恢复订阅，不重算评分"),
("training_sample_builder.py","TrainingSampleBuilder","将生成事件转成有版本的框架中间视图，校验 token 对齐"),
("legacy_request_adapter.py","LegacyRequestAdapter","迁移期唯一旧字段/旧 env_type 转换入口")],
"infrastructure": [
("episode_client.py","GrpcEpisodeClient","Server RPC 与同 request_id 的传输重试"),
("artifact_client.py","ArtifactClient","读取不可变轨迹和任务文件，验证 digest"),
("hub_client.py","HubClient","调用代码包与固定数据 revision 的发布/解析 API"),
("model_gateway.py","ModelGateway","转发模型请求与真实 token/version，不读私有评分材料"),
("adapter_host.py","DatasetAdapterHost","加载 dataset_package 中的 Adapter 类并调用 normalize，隔离其依赖；转换规则在包内 dataset_adapter.py")],
"interfaces": [
("client.py","UEnvClient","稳定 Python 用户入口"),
("cli.py","main","读取文件为对象并调用 UEnvClient；不自行补值、构造请求或发送 RPC，不提供执行配置覆盖"),
("verl_adapter.py","VerlAdapter","VeRL 的输入输出形态和版本兼容"),
("roll_adapter.py","RollAdapter","ROLL 适配；实验性能力保持实验标签，未实现不宣称支持")],
".":[("bootstrap.py","build_bridge","组装依赖，不含任务业务")]
},
"crates/uenv-server/src": {
"domain":[
("episode.rs","Episode / EpisodeState / Attempt","服务端状态与合法转移"),
("lease.rs","Lease / LeaseState","租约身份、任期和有效性"),
("worker.rs","Worker / WorkerCapabilities","Worker 可用资源和能力视图"),
("ports.rs","EpisodeRepository / WorkerClient / PackageResolver","应用层所需持久化/派发/解析接口"),
("error.rs","ServerError","稳定错误分类，区分任务错误和系统错误")],
"application":[
("episode_service.rs","EpisodeService","统一提交入口，禁止数据集分支"),
("request_validation.rs","validate_request","协议与包 schema 校验；幂等状态比较由 EpisodeService 在仓库事务中处理"),
("plan_resolver.rs","PlanResolver / validate_batch_submission","校验批次共享配置与身份，逐成员从同一 run_spec 生成 ExecutionPlan；原位置锁定组件，tools 表只一份，runtime.image 只一处生效"),
("admission_controller.rs","AdmissionController","有界排队与租户/作业并发配额"),
("placement_scheduler.rs","PlacementScheduler","按能力和资源 reserve，不读题目字段"),
("lease_service.rs","LeaseService","颁发/更新/失效 lease"),
("episode_coordinator.rs","EpisodeCoordinator","accept_result 校验结果、attempt 与 lease，并在同一仓库事务提交唯一终态及 outbox；管理取消和 attempt 切换"),
("recovery_service.rs","RecoveryService","重启后对账与 outbox 重放"),
("progress_ingestor.rs","ProgressIngestor","接收可丢失的进度投影；不编号、封存或保存第二份权威轨迹")],
"infrastructure":[
("sqlite_repository.rs","SqliteEpisodeRepository","SQL、事务、状态更新和结果 outbox"),
("worker_client.rs","GrpcWorkerClient","Worker start/cancel/query RPC"),
("worker_registry.rs","WorkerRegistry","心跳、drain 与节点过期"),
("package_resolver.rs","HubPackageResolver","Hub 协议和缓存，精确坐标校验"),
("file_store.rs","LocalFileStore","实现内部 FileStore 的本地文件存取与完整性校验；保存策略由 Server 决定"),
("result_publisher.rs","ResultPublisher","已提交结果通知重发"),
("observation_store.rs","ObservationStore","由事件生成的查询投影，可重建")],
"interfaces":[
("episode_rpc.rs","EpisodeRpc","用户任务/批次/取消/查询 RPC"),
("worker_rpc.rs","WorkerControlRpc","注册、心跳、事件和结果上报"),
("legacy_agent_control_rpc.rs","LegacyAgentControlRpc","仅迁移期对照现有 runner；旧池化入口随兼容层退役，不进入目标调度"),
("admin_http.rs","AdminHttp","只读状态与受控管理 API"),
("trajectory_http.rs","TrajectoryHttp","轨迹查询、下载与访问控制")],
".":[("config.rs","ServerConfig","启动配置，集中解析"),("bootstrap.rs","build_server","依赖组装"),("main.rs","main","进程入口与信号处理"),("lib.rs","公共导出","只导出稳定入口")]
},
"crates/uenv-worker/src": {
"domain":[
("session.rs","Session / SessionState","任务资源生命周期，独立于 task_id"),
("backend.rs","Backend / SnapshotBackend","通用运行接口和可选快照能力"),
("episode_scope.rs","EpisodeScope","一次 attempt 的资源所有权与清理义务"),
("ports.rs","AgentHost / EnvironmentHost / ScorerHost / ToolHost / ModelProvider / FileStore / ResultReporter / PackageCache","同一 ComponentHost 协议在 Worker 内按 Agent/Environment 权限拆成类型安全端口；ToolHost 由 ToolGateway 实现；另含评分、模型、产物、上报和包缓存边界"),
("error.rs","WorkerError","可重试性与故障阶段")],
"application":[
("episode_supervisor.rs","EpisodeSupervisor","唯一 attempt 状态机；统一 prepare/session/component run/freeze/score/cleanup/seal/report 顺序"),
("dispatch_validation.rs","validate_dispatch","lease、epoch、digest 校验；WorkerRpc 在调用 Supervisor 前维护活动 attempt 与重复派发账本"),
("session_manager.rs","SessionManager","创建、借用、重置、归还、作废环境资源"),
("agent_runtime.rs","AgentRuntime","generate 与系统 step(tool_call) 的唯一受控入口；step 校验、分发、计数和记录一次工具动作"),
("budget_enforcer.rs","BudgetEnforcer","权威截止时间、模型/工具调用计数和收尾预留"),
("tool_gateway.rs","ToolGateway / ToolHost","绑定 ExecutionPlan.tools 与 Backend session，按 scope 调用 Python 工具、Agent 原生工具或外部连接；预算和轨迹由 AgentRuntime 强制"),
("scoring.rs","run_score","仅 scoring.enabled=true 时调用数据集包内 Python Scorer 一次；无评分跳过该阶段，结果按计划核验；不包含数据集规则"),
("trajectory_writer.rs","TrajectoryWriter","接收 Rust 受控调用产生的事件，分配顺序、校验身份、创建评分前快照并封存最终轨迹；Python 只返回操作结果"),
("cancellation_service.rs","CancellationService","中止模型/工具/评分及传播取消"),
("cleanup_service.rs","CleanupService","统一清理与有界失败重试"),
("harness_executor.rs","HarnessExecutor","依据类型化请求在本 attempt 的冻结评分视图上执行 harness；沿用计划锁定的后端和剩余预算，不产生第二份后端配置"),
("result_reporter.rs","ResultReporter","持久化候选终态并重传直到 ACK"),
("package_service.rs","PackageService","确保精确组件和 runtime assets 可用")],
"infrastructure":[
("backend_registry.rs","BackendRegistry","按显式 implementation 取得后端"),
("process_backend.rs","ProcessBackend","工作区、进程组与本机 runtime profile"),
("docker_backend.rs","DockerBackend","Docker engine 操作"),
("podman_backend.rs","PodmanBackend","Podman engine 操作"),
("container_engine.rs","ContainerEngineClient","共享 OCI 引擎调用与通用参数，不包含 benchmark"),
("component_host_process.rs","ComponentHostProcess","用同一协议启动角色隔离的 Agent、Environment/沙箱工具进程实例；管理健康、终止和退出"),
("component_host_client.rs","ComponentHostClient","按 host 角色只暴露允许的 load/reset/run/state_snapshot/tool/close IPC"),
("scorer_host_process.rs","ScorerHostProcess","隔离启动 Python Scorer，控制私有材料、依赖和强制超时"),
("scorer_host_client.rs","ScorerHostClient","单次 ScoreInput/ScoreResult 类型化 IPC"),
("server_client.rs","ServerControlClient","注册、心跳、结果 RPC"),
("durable_outbox.rs","DurableOutbox","Worker 唯一结果待 ACK 存储，合并旧 WAL 功能"),
("trajectory_spool.rs","TrajectorySpool","事件暂存、分片上传、ACK 与 GC"),
("package_cache.rs","LocalPackageCache","按 digest 缓存代码、数据、依赖"),
("image_cache.rs","ImageCache","镜像缓存；不按数据集命名推导"),
("backend_session_pool.rs","BackendSessionWarmPool","可选预热后端 session，按完整兼容键复用；与 Agent 调度无关")],
"interfaces":[
("worker_rpc.rs","WorkerRpc","start/cancel/query 服务"),
("runtime_rpc.rs","RuntimeRpc","受 session 和角色约束的 exec/files/harness 能力"),
("metrics_http.rs","MetricsHttp","资源/队列/任务指标")],
".":[("config.rs","WorkerConfig","平台启动配置"),("bootstrap.rs","build_worker","组件注册与注入"),("main.rs","main","进程入口与关闭信号"),("lib.rs","公共导出","模块边界")]
},
"python/uenv_component_host/src/uenv_component_host": {
"domain":[
("context.py","受控 Context 实现","实现 SDK 的 EnvironmentContext / AgentContext / ScoringContext；不重定义同名协议，截止时间和取消状态由 Rust Worker 注入"),
("ports.py","RuntimeClient","按角色调用 Rust 受控运行入口；不直连模型服务或自行提交权威事件")],
"application":[
("component_host.py","ComponentHost","按启动角色加载 AgentRunner，或加载 Environment 与沙箱 Python 工具；不同权限使用不同进程实例"),
("scorer_host.py","ScorerHost","从唯一锁定的数据集包加载 Scorer，读取 scoring.config 并执行一次单条 episode 评分；不补全系统字段或封存轨迹"),
],
"infrastructure":[
("entrypoint_loader.py","EntryPointLoader","加载指定角色的版本化组件，校验接口和参数 schema；不管理模型循环或数据集业务"),
("runtime_client.py","RuntimeClient","按角色调用 Rust Worker 的 generate/step/exec/files/harness 能力；计量与事件由 Rust 产生"),
("artifact_client.py","ArtifactClient","读写产物引用与校验摘要"),
("legacy_openhands_adapter.py","LegacyOpenHandsAdapter","仅迁移期连接旧 runner 做结果对照，迁移后删除；不提供 Agent 池或独立容量调度")],
"interfaces":[("mcp_server.py","MCP 服务接口","将唯一授权工具表包装为 MCP，调用经 RuntimeClient 委托 Rust step；Worker 管理连接生命周期，不直接执行工具或重建预算"),("component_host_rpc.py","ComponentHostRpc","接收角色限定的 load/reset/run/state_snapshot/tool/close 命令；不接收完整 ExecutionPlan"),("scorer_host_rpc.py","ScorerHostRpc","接收一次单条 ScoreInput 和取消命令")],
".":[("bootstrap.py","build_host","加载 host 配置并注入客户端"),("__main__.py","main","按 component/scorer 模式启动受管 Python host")]
},
"python/uenv_sdk/src/uenv/sdk": {
"domain":[
("model.py","UEnvModel","数据集业务模型基类及共享内容辅助函数 structured_part；作者使用类型标注，SDK 在边界自动封装 TypedConfig"),
("dataset.py","DatasetAdapter / PreparedSample","SDK 定义基类及返回类型；normalize 的数据集实现位于包内 dataset_adapter.py"),
("environment.py","Environment / Observation","SDK 定义接口；Environment 初始化、持有状态、提供评分状态与清理，不注册工具；操作定义在包内 tools.py"),
("agent.py","AgentRunner","智能体开发接口；run 返回 ContentPart[]"),
("tool.py","ToolExecutor / ToolSpec","工具开发接口；各包 tools.py 使用同一 @tool/ToolExecutor 规范，按需注入环境 Context；SDK call_tool 委托系统 step"),
("scoring.py","Scorer / ScoreInput / ScoreResult","SDK 定义单条评分接口和结果类型；具体规则在包内 Scorer，可显式复用公共函数")],
"application":[("package_validator.py","PackageValidator","用户包入口与 schema 检查；确定的字段遮蔽/已登记别名报错，疑似同义字段警告"),("contract_inspector.py","describe_author_type / describe_package","按数据集作者或运行用户过滤同一契约；默认不展示内部协议字段")],
"infrastructure":[("schema_registry.py","SchemaRegistry","按摘要注册包 schema，绑定 TypedConfig，拒绝未知类型和内容冲突")],
"interfaces":[("package_cli.py","package_main","init/validate/test/publish/describe 的统一 SDK CLI")]
},
"crates/uenv-hub/src": {
"domain":[("package.rs","Package / ComponentVersion","不可变代码包发布身份"),("data_revision.rs","DataRevision / DataShard / SampleIndexEntry","不可变数据版本、分片和样本索引元数据"),("ports.rs","PackageRepository / DataRepository / FileStore","代码包、数据索引端口；引用共享内部 FileStore，不重复定义存储接口")],
"application":[("package_service.rs","PackageService","代码包发布、校验和精确版本解析"),("data_service.rs","DataService","发布不可变数据 revision，并按固定 revision 和样本选择解析标准化记录"),("compatibility.rs","validate_compatibility","比较声明的接口与能力；由 PackageService 调用，不持有独立状态或包含 SWE 特判")],
"infrastructure":[("sqlite_repository.rs","SqliteRegistryRepository","保存代码包与数据 revision/索引元数据"),("file_store.rs","LocalFileStore","装配内部 FileStore 的本地文件实现，按 digest 存取字节；包版本与权限由 Hub 服务处理")],
"interfaces":[("package_http.rs","PackageHttp","代码包发布/查询/下载 API"),("data_http.rs","DataHttp","数据 revision 发布、样本查询和获准分片下载 API")],
".":[("config.rs","HubConfig","部署配置"),("bootstrap.rs","build_hub","注入依赖"),("main.rs","main","启动服务")]
}}

def main():
    lines=['# UEnv 目标模块与逐文件职责','', '本文件由 `scripts/build_module_map.py` 自动生成，禁止手工编辑；修改目标模块时编辑生成器后重新生成。','',
           '本文件是目标组织清单，不是现有代码目录，也不表示这些类已经实现。每个组件采用相同四层命名；用户扩展包采用统一的小型模板，无需复制服务端四层结构。层数由组件角色决定，不是各目录任意选择风格。','',
           '公共根目录：`contracts/proto/uenv/v1/`（系统结构和 RPC 的唯一可编辑来源），`contracts/generated/schema/`、`crates/uenv-contracts/` 和 `python/uenv_contracts/`（全部生成，不手改），`crates/`（Rust 控制服务与资源驱动），`python/`（Python SDK、Bridge 和受管组件 host），`packages/datasets/`、`packages/agents/`、`packages/tools/`（用户同款扩展），`deploy/`、`docs/`、`tests/system/`。','',
           '所有 Rust mod.rs 与 Python __init__.py 只导出模块；下面不逐一列这些样板文件。Rust Cargo.toml 与 Python pyproject.toml 各包一份，锁定依赖。数据集新增业务字段只在包内 models.py 定义，发布工具生成校验 schema；系统 proto 和包模型的职责不重叠。','']
    count=0
    for root,layers in MODULES.items():
        lines += [f'## {root}', '', '| 文件 | 类/trait/主要函数 | 职责 |','|---|---|---|']
        for layer,items in layers.items():
            for filename,symbol,description in items:
                path=filename if layer=='.' else layer+'/'+filename
                lines.append(f'| `{path}` | {symbol} | {description} |'); count+=1
        lines += ['', '测试目录：本包 `tests/unit/` 检查本地逻辑，`tests/contract/` 检查边界协议，`tests/integration/` 检查真实外部依赖。跨 Bridge/Server/Worker 的测试仅放根 `tests/system/`，避免每层维护一份略有差异的全链路。','']
    lines += ['## 用户扩展包统一模板','',
              '| 文件 | 责任 |','|---|---|',
              '| `dataset.yaml` | 用户唯一编辑的本地发布声明；发布工具解析它，不把原文件作为运行配置上传 |',
              '| `PackageManifest` API 对象 | 发布工具结合声明与产物 digest 组装；Hub 存结构化记录并在查询时返回 JSON，不要求独立 manifest.json 文件 |',
              '| `pyproject.toml` | 本地构建输入；构建工具把 src 生成一个版本化 Python wheel |',
              '| `src/<package_name>/models.py` | 按需定义本包新增业务字段；构建进 wheel，发布工具自动生成只读 schema |',
              '| `src/<package_name>/dataset_adapter.py` | 必需：声明 DatasetNameAdapter；构建进同一个 wheel |',
              '| `src/<package_name>/environment.py` | 必需：声明 DatasetNameEnvironment；构建进同一个 wheel |',
              '| `src/<package_name>/scorer.py` | 提供评分时必需：声明 DatasetNameScorer；仅采集可省略 |',
              '| `src/<package_name>/tools.py` | 可选：本包工具唯一声明与实现入口；通用和状态工具使用同一规范，构建进同一个 wheel |',
              '| 发布工具生成的 schema | 不属于用户工程源码；从 models.py 生成，按 digest 发布并由 manifest 索引 |',
              '| `tests/cases.jsonl` | 可选的本地/CI 转换与评分案例；不作为发布条件，默认不上传 Hub |',
              '| `tests/test_contract.py` | 可选的本地/CI 契约测试；不作为发布条件，默认不上传 Hub |',
              '| `tests/test_scoring.py` | 可选的本地/CI 评分测试；不作为发布条件，默认不上传 Hub |','',
              '每个数据集的 dataset_adapter.py、environment.py 与专属类必需；提供评分时再声明 scorer.py 与专属 Scorer，只有采集需求的包可省略；其余辅助文件按需创建。已提供的入口类分别直接继承系统基类，并实现 normalize、reset、score；Scorer 只处理单条 episode。公共逻辑通过函数或组合复用，不复制算法。run.yaml 属于训练、评测、轨迹采集或其他实验项目，不是数据集包文件。九个 `reference/datasets/*` 示例已经使用同一 `dataset.yaml + pyproject.toml + src/<package> + tests` 布局；公开运行文件位于 `reference/runs`，生成对象位于 `reference/generated`。产品侧作者 CLI 尚未在生产源码实现。','',
              '基础组件包：packages/agents/plain、packages/agents/openhands 各含 runner.py、配置模型和 contract tests；Agent 接入代码在本包内统一注册工具并转交调用，不逐工具协商接口。发布工具生成内部 AgentManifest，包含组件身份、实现入口、配置 schema、原生导出 provided_tools 和确实必需的 required_tool_names。packages/tools/terminal、packages/tools/file_editor 等各含 src/<package>/tools.py、类型模型和 contract tests；ToolSpec 和 input/output schema 由发布工具生成。MCP 包装在系统 ComponentHost 的 interfaces/mcp_server.py 统一实现，按执行授权把同一工具表暴露给框架并委托 Rust step；Agent runner.py 只配置连接或直接注册。原生工具由 Agent 接入代码读取框架已有定义，不重复放入 tools.py，不在 AgentRunner 或 Environment 定义工具业务，不另建逐工具适配器包。共享呈现函数和评分辅助放 packages/shared，按版本依赖，禁止复制源文件到各数据集。','',
              f'共列出 {count} 个服务/SDK目标源码文件。实现时允许合并职责紧密且很小的模块，但不能改变依赖方向或把不同领域职责塞入一个大文件。文件数不是验收指标。']
    (ROOT/'docs/generated').mkdir(parents=True, exist_ok=True)
    (ROOT/'docs/generated/module_map.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')
    print('Module map:',count,'files')

if __name__=='__main__': main()
