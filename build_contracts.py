"""Generate reviewable vNext contract draft and its complete field dictionary."""
from pathlib import Path
import json
import copy

ROOT = Path(__file__).resolve().parent
D = {}

def ref(name, description=None):
    x = {"$ref": f"#/$defs/{name}"}
    if description:
        x["description"] = description
    return x

def string(description, **kw):
    return {"type": "string", "description": description, **kw}

def integer(description, minimum=0, **kw):
    return {"type": "integer", "minimum": minimum, "description": description, **kw}

def number(description, **kw):
    return {"type": "number", "description": description, **kw}

def boolean(description):
    return {"type": "boolean", "description": description}

def enum(description, *values):
    return string(description, enum=list(values))

def array(item, description):
    return {"type": "array", "items": item, "description": description}

def obj(name, description, fields, optional=()):
    D[name] = {"type": "object", "description": description,
               "properties": copy.deepcopy(fields), "required": [k for k in fields if k not in optional],
               "additionalProperties": False}

ID = string("不透明身份标识；不得用其他实体的 ID 代填", minLength=1, maxLength=256)
SHA = string("内容 SHA-256；必须对应实际字节", pattern=r"^sha256:[0-9a-f]{64}$")
TIME = integer("UTC Unix 毫秒；跨机器用于记录，超时执行使用本机单调时钟")

obj("ComponentRef", "作者指定的组件坐标；提交时解析为 ResolvedComponent", {
    "id": ID, "version": string("精确版本或提交时解析的版本约束"),
    "digest": SHA}, ("digest",))
obj("ResolvedComponent", "不可变组件引用；执行期间不得重新解析 latest", {
    "id": ID, "version": string("已解析精确版本"), "digest": SHA})
obj("ArtifactRef", "不可变产物引用；URI 不得含凭据", {
    "uri": string("支持的存储 URI；访问凭据由身份系统提供"), "digest": SHA,
    "size_bytes": integer("字节数"), "media_type": string("MIME 类型")})
obj("ErrorRecord", "统一结构化错误；任务答错不属于基础设施错误", {
    "code": string("稳定大写错误码，如 SCORER_FAILED"),
    "phase": enum("发生阶段", "validation", "queue", "prepare", "agent", "model", "tool", "environment", "score", "persist", "cleanup"),
    "message": string("可读说明，不放密钥或完整私有评分输入"),
    "retryable": boolean("由错误分类决定的重试资格；不是立即重试命令"),
    "operation_id": ID,
    "diagnostic_ref": ref("ArtifactRef", "完整诊断材料引用")}, ("operation_id", "diagnostic_ref",))
D["ErrorRecord"]["properties"]["operation_id"]["description"] = (
    "已经接纳的外部操作身份；按 phase 引用同一次操作已有的 generation_id、"
    "tool_call_id 或 environment_step_index 字符串，不另造第二套身份"
)
obj("ContentPart", "消息内容段；文本与外部产物二选一，由 kind 指定", {
    "kind": enum("内容种类", "text", "artifact"), "text": string("原始文本，不截断"),
    "artifact": ref("ArtifactRef")}, ("text", "artifact"))
D["ContentPart"]["oneOf"] = [
    {"properties": {"kind": {"const": "text"}}, "required": ["text"], "not": {"required": ["artifact"]}},
    {"properties": {"kind": {"const": "artifact"}}, "required": ["artifact"], "not": {"required": ["text"]}}]
obj("Message", "模型可见消息；工具调用详情通过 ToolCall 单独记录", {
    "role": enum("消息来源", "system", "user", "assistant", "tool"),
    "content": array(ref("ContentPart"), "有序内容段"),
    "tool_call_id": ID}, ("tool_call_id",))

# Built-in component schemas are reference catalog inputs. Dataset-owned
# business schemas are generated from each package's models.py by build_examples.py.
obj("EmptyConfig", "无额外参数", {})
obj("PlainAgentConfig", "轻量智能体配置", {
    "history_policy": enum("模型上下文策略", "full", "last_generation"),
    "system_prompt": string("智能体系统提示词；空表示不额外添加")})
obj("OpenHandsAgentConfig", "OpenHands adapter 的明确可调参数", {
    "history_policy": enum("由 SDK 管理上下文", "sdk"),
    "system_prompt": string("用户附加的系统提示"),
    "sdk_iteration_limit": integer("SDK 自身迭代上限；不能替代 max_generations", 1)})
obj("ProcessBackendConfig", "进程任务后端；无容器镜像", {
    "runtime_profile": string("管理员预注册的本机依赖配置名；不得改变平台安全底线或计划中的 internet_access")})
obj("ContainerBackendConfig", "Docker/Podman 后端配置；引擎种类独立于镜像", {
    "runtime_profile": string("管理员预注册的引擎连接配置名；不得携带网络、挂载或身份权限")})
obj("ToolConfig", "公共命令/文件工具参数", {
    "timeout_ms": integer("工具执行预算，受 episode 总 deadline 限制", 1),
    "max_preview_bytes": integer("返回消息预览长度；原始输出另存 artifact", 1)})
obj("TerminalArgs", "终端工具参数", {"argv": array(string("单个参数"), "显式 argv；需要 shell 时明确给出 shell -c 参数"), "cwd": string("沙箱内目录")})
obj("ReadFileArgs", "读取工具参数", {"path": string("工作区内路径")})
obj("WriteFileArgs", "写入工具参数", {"path": string("工作区内路径"), "content": string("完整写入内容")})
obj("AnswerAction", "问答环境动作", {"answer": string("完整回答")})
obj("EvaluationPlan", "包拥有的评测执行计划；不能给 Agent 读取", {
    "harness": ref("ResolvedComponent", "唯一 harness 选择，固定精确版本和 digest"), "setup_argv": array(array(string("参数"), "一次命令"), "准备步骤"),
    "test_argv": array(string("参数"), "测试入口命令"), "cwd": string("评分沙箱目录"),
    "timeout_ms": integer("评测预算",1)})

CONFIGS = ["EmptyConfig", "PlainAgentConfig", "OpenHandsAgentConfig", "ProcessBackendConfig",
           "ContainerBackendConfig", "ToolConfig", "TerminalArgs", "ReadFileArgs",
           "WriteFileArgs", "AnswerAction", "EvaluationPlan"]
for n in CONFIGS:
    obj(n+"Envelope", "具备确定 schema 的扩展内容", {
        "schema_ref": {"const": f"uenv://schemas/vnext/{n}", "description": "精确 schema 标识"},
        "data": ref(n, "字段递归定义见引用的结构")})
obj("TypedConfig", "扩展信封；必须由 SchemaRegistry 绑定 schema_ref 对应的已注册 schema 后校验 data，禁止仅做信封校验", {
    "schema_ref": string("精确 schema URI，未知 URI 必须拒绝", minLength=1),
    "data": {"not": {}, "description": "未绑定前禁止执行验证通过；SchemaRegistry 按已注册版本化 schema 替换为 oneOf/$ref"}})
obj("DatasetRef", "数据来源身份", {
    "id": ID, "revision": string("不可变数据版本或内容 digest"), "split": string("数据划分名"),
    "subset": string("数据子集名；无子集为空")})
obj("RuntimeSpec", "镜像候选；按 run、task、package 优先级选择，字段始终叫 image", {
    "image": string("非空 OCI 镜像引用；执行计划必须锁定内容 digest", minLength=1)})
obj("TaskSpec", "唯一公开任务定义；可传给 Environment/Agent；不含评分材料及引用；runtime 仅声明样本运行资源候选，不选择后端", {
    "schema_version": {"const": "vnext.3", "description": "契约版本"}, "task_id": ID,
    "dataset": ref("DatasetRef"), "sample_id": string("数据集内稳定样本 ID"),
    "input": ref("TypedConfig", "公开业务字段，必须由 dataset package 声明其 schema"),
    "input_digest": SHA, "runtime": ref("RuntimeSpec", "可选样本镜像候选，不是最终执行镜像")}, ("runtime",))
obj("ComponentSpec", "选择一个实现及其 schema 验证后的参数", {"implementation": ref("ComponentRef"), "config": ref("TypedConfig")})
obj("ToolBinding", "用户明确授权的一项模型可见工具；包含 SDK 原生工具，不自动合并默认工具", {
    "name": string("本次会话唯一工具名；绑定 SDK 工具时使用适配器声明的名字", minLength=1),
    "implementation": ref("ComponentRef", "实际执行实现；原生状态工具也发布为有版本的包装组件"),
    "config": ref("TypedConfig")})
obj("ToolInterface", "工具实现可使用的一种标准接入接口；Agent 按自己声明的优先顺序选择首个兼容项", {
    "interface": string("版本化接口标识，例如 mcp.v1 或 openhands_native.v1", minLength=1),
    "adapter": ref("ComponentRef", "把该工具接口接入 Agent 的受管适配器"),
    "execution_scope": enum("工具由哪个受控执行入口承接；不是底层访问授权", "agent_state", "sandbox", "external_service"),
    "required_capabilities": array(string("版本化运行能力标识"), "该接入方式需要的 Worker 能力；不会授予文件、网络或宿主权限")})
obj("ResolvedToolBinding", "预检后固定的实际工具绑定；Worker 初始化后必须核验 SDK 实际工具表与它一致", {
    "name": string("本次会话唯一模型可见工具名", minLength=1),
    "implementation": ref("ResolvedComponent"), "adapter": ref("ResolvedComponent"),
    "config": ref("TypedConfig"), "interface": string("Agent 与工具共同支持并已锁定的版本化接入接口"),
    "execution_scope": enum("工具由哪个受控执行入口承接；不是底层访问授权", "agent_state", "sandbox", "external_service"),
    "required_capabilities": array(string("版本化运行能力标识"), "仅用于 Worker 兼容性匹配；不会授予文件、网络或宿主权限")})
obj("Resources", "每个 episode 的资源上限；Worker 还需套管理员上限", {
    "cpu_cores": number("CPU 核数", exclusiveMinimum=0), "memory_bytes": integer("内存字节数", 1),
    "process_limit": integer("最大进程数", 1), "disk_bytes": integer("工作区字节预算", 1)})
obj("BackendSpec", "由用户明确选择的任务后端；不读取数据集名称做选择", {
    "implementation": ref("ComponentRef"), "config": ref("TypedConfig"), "resources": ref("Resources")})
obj("GenerationConfig", "规范模型生成参数；额外 provider 参数须另注册 schema", {
    "temperature": number("采样温度", minimum=0), "top_p": number("核采样概率", exclusiveMinimum=0, maximum=1),
    "max_output_tokens": integer("单次生成最大 token 数", 1),
    "stop": array(string("停止串"), "停止字符串列表")})
obj("ModelSpec", "统一模型端点；训练时可指向 Bridge ModelGateway", {
    "endpoint": string("端点地址，不包含密钥"), "credential_ref": string("凭据引用；允许空表示不需要凭据"),
    "model_id": string("模型身份"), "generation": ref("GenerationConfig"),
    "source": enum("模型来源，模拟必须显式声明", "real", "simulated"),
    "max_transport_retries": integer("仅尚未产生可用生成结果时的网络重试上限")})
obj("Limits", "预算命名不可混用；所有值在 RunSpec 中显式提交", {
    "total_timeout_ms": integer("从 Server 接收起的总预算，包含排队及评分", 1),
    "score_reserve_ms": integer("为结果收集、冻结及最终评分预留预算；三者共享总截止时间"),
    "max_generations": integer("成功或部分完成的模型生成调用预算", 1),
    "max_tool_calls": integer("接受执行的工具调用预算"),
    "max_environment_steps": integer("环境转移次数预算；无状态任务可为 0"),
    "max_total_output_tokens": integer("episode 累计生成 token 上限", 1)})
obj("TrainingSpec", "训练接入规则，不是数据集参数", {
    "parallel_mode": enum("提交/消费策略", "sync", "one_step_off_policy", "fully_async"),
    "require_token_trace": boolean("为 true 时缺少真实 token/logprob 必须拒绝进入训练"),
    "version_policy": enum("模型版本规则", "fixed_episode", "per_generation"),
    "requested_policy_version": string("期望版本，fixed_episode 必填；per_generation 可空"),
    "max_policy_lag": integer("允许的版本落后量；需模型注册表提供可比较序号")})
obj("RetryPolicy", "episode 失败重试仅 Server 决定", {
    "max_attempts": integer("含首次在内的最大 attempt 数", 1),
    "initial_backoff_ms": integer("初始退避毫秒", 1),
    "max_backoff_ms": integer("退避上限毫秒", 1)})
obj("RunSpec", "用户创建作业时提供的完整配置", {
    "schema_version": {"const": "vnext.3", "description": "契约版本"}, "run_id": ID,
    "purpose": enum("作业用途", "evaluation", "training"), "environment": ref("ComponentSpec"),
    "agent": ref("ComponentSpec", "智能体实现与参数；不提供 Agent 池或 placement；按 agent 角色校验接口和配置"), "tools": array(ref("ToolBinding"), "完整显式工具绑定；空数组要求实际无模型可见工具，不兼容的 Agent 必须拒绝"),
    "scorer": ref("ComponentSpec", "本次运行唯一评分器；评测与后训练复用它产生的同一个 ScoreResult"), "backend": ref("BackendSpec"), "model": ref("ModelSpec"),
    "limits": ref("Limits"), "retry": ref("RetryPolicy"),
    "trajectory_retention_days": integer("Server 保存权威轨迹的天数；只影响保留期，不改变记录内容", 1),
    "training": ref("TrainingSpec", "仅 purpose=training 时必填；purpose=evaluation 时禁止出现"),
    "runtime": ref("RuntimeSpec", "用户显式镜像选择，优先于 task 和 package")}, ("training", "runtime"))
D["RunSpec"]["allOf"] = [{
    "if": {"properties": {"purpose": {"const": "training"}}},
    "then": {"required": ["training"]},
    "else": {"not": {"required": ["training"]}},
}]
obj("EpisodeRequest", "Bridge -> Server；不接受客户端指定 attempt 或 lease", {
    "request_id": ID, "run_id": ID, "episode_id": ID, "task": ref("TaskSpec"),
    "private_data": ref("TypedConfig", "可选评分依据，随受控请求配对提交；大型测试使用内部 ArtifactRef；不得交给 Agent/Environment 或公开轨迹"),
    "seed": integer("本次 episode 种子"), "sample_index": integer("批次内位置，从 0 开始"),
    "batch_id": ID}, ("private_data",))
obj("BatchRequest", "提交一组 episode；batch ID 必须和各项一致", {
    "run_id": ID, "batch_id": ID,
    "episodes": array(ref("EpisodeRequest"), "非空任务列表，数量受服务器限制")})
obj("BatchReceipt", "提交确认不是执行成功", {"batch_id": ID, "episode_ids": array(ID, "已接纳的 episode ID")})
obj("Lease", "Server 颁发，Worker 核验；不能由用户任务参数携带", {
    "lease_id": ID, "epoch": integer("服务实例任期"), "expires_at_ms": TIME,
    "token": string("租约授权值；不得写入用户轨迹")})
# Reuse the authoring structures, strengthening reference constraints in place.
# No second set of role-specific Resolved*Spec wrapper classes.
def resolved_schema(name):
    def walk(value):
        if isinstance(value, dict):
            if value.get("$ref") == "#/$defs/ComponentRef":
                return ref("ResolvedComponent", value.get("description"))
            if "$ref" in value and value["$ref"].split("/")[-1] in ("ComponentSpec",):
                return walk(copy.deepcopy(D[value["$ref"].split("/")[-1]]))
            return {k: walk(v) for k, v in value.items()}
        if isinstance(value, list):
            return [walk(v) for v in value]
        return value
    return walk(copy.deepcopy(D[name]))

plan_fields = {k: copy.deepcopy(v) for k, v in D["EpisodeRequest"]["properties"].items()
               if k not in ("request_id", "batch_id", "sample_index")}
plan_fields.update({k: copy.deepcopy(D["RunSpec"]["properties"][k])
                    for k in ("purpose", "model", "limits", "training")})
plan_fields.update({k: resolved_schema(t) for k, t in (
    ("environment", "ComponentSpec"), ("agent", "ComponentSpec"),
    ("backend", "BackendSpec"), ("scorer", "ComponentSpec"))})
plan_fields.update({
    "attempt_id": integer("Server 生成；正常首次执行为 1，只有基础设施重试才递增，且不重选配置", 1),
    "tools": array(ref("ResolvedToolBinding"), "唯一生效工具表；保留 RunSpec.tools 的字段名，锁定版本和执行路由"),
    "required_capabilities": array(string("版本化运行能力标识"), "完整运行能力需求，仅供 Worker 调度与兼容性核验；不是访问授权"),
    "internet_access": boolean("Environment 是否需要访问公共互联网；Server 锁定，Backend 受控落实"),
    "runtime": {**copy.deepcopy(D["RuntimeSpec"]), "description": "本条计划唯一生效的容器资源，不是待选择的候选", "properties": {
        "image": string("唯一生效镜像；task.runtime 只是原始任务候选，Worker 不再消费它", pattern=r"^[^@\s]+@sha256:[0-9a-f]{64}$"),
        "image_source": enum("最终镜像的来源，用于审计", "run", "task", "package")},
        "required": ["image", "image_source"]},
    "plan_digest": SHA, "deadline_at_ms": {**TIME, "description": "首次接纳时间加 limits.total_timeout_ms，重试不续期；Worker 唯一总截止时间"}})
obj("ExecutionPlan", "由请求与配置转换而来，不嵌套 EpisodeRequest/RunSpec；每项执行配置仅一处生效", plan_fields,
    ("private_data", "training", "runtime"))
D["ExecutionPlan"]["allOf"] = copy.deepcopy(D["RunSpec"]["allOf"])
obj("DispatchRequest", "Server -> Worker；派生预算字段只能收紧 ExecutionPlan 中的限制", {
    "plan": ref("ExecutionPlan"), "lease": ref("Lease"),
    "remaining_timeout_ms": integer("派发时距离 plan.deadline_at_ms 的剩余上限；Worker 转为本机单调时钟", 1),
    "consumed_usage": ref("Usage", "此前 attempt 已确认消耗的 episode 累计用量；首次派发全为 0")})
obj("SessionRef", "资源 session 身份；不等于 task_id 或 sample_id", {"session_id": ID, "backend": ref("ResolvedComponent")})

obj("Observation", "模型可见观测；文本、多模态 artifact 与结构化数据可同时存在", {
    "content": array(ref("ContentPart"), "有序原始可见内容"),
    "data": ref("TypedConfig", "结构化可见状态，不含评分私有材料")}, ("data",))
obj("Transition", "一次环境动作产生的状态转移，不等于模型调用或工具调用", {
    "observation": ref("Observation"), "terminated": boolean("环境到达自然终态"),
    "episode_truncated": boolean("因外部预算等限制截断"),
    "environment_reward": {"type":["number","null"], "description":"可选环境原生信号，不自动成为训练 reward"}},
    ("environment_reward",))
obj("Outcome", "统一执行结果；Agent 提交与环境收集共用，state 仅由 Environment 填写", {
    "final_answer": array(ref("ContentPart"), "可以为空；不要求所有任务产生文本答案"),
    "artifacts": array(ref("ArtifactRef"), "冻结的文件、补丁或其他产物"),
    "state": ref("TypedConfig", "环境提供的评分状态快照，不发送给 Agent"),
    "termination_reason": enum("交互为何结束；in_progress 只用于过程快照，系统取消或失败由 EpisodeResult.execution_status 与 ErrorRecord 表达", "in_progress", "final_answer", "environment_terminal", "budget_exhausted")}, ("state",))
obj("ScoreInput", "Worker -> Python Scorer；只在 episode 结束后创建，不能发送到 Agent", {
    "task": ref("TaskSpec"), "outcome": ref("Outcome"),
    "trajectory_ref": ref("ArtifactRef"), "private_data": ref("TypedConfig")}, ("private_data",))
D["ScoreInput"]["allOf"] = [{"properties":{"outcome":{"properties":{
    "termination_reason":{"not":{"const":"in_progress"}}}}}}]
obj("Metric", "一条 episode 的单个具名指标", {
    "name": string("如 accuracy/resolved/tests_passed"), "value": number("有限数，拒绝 NaN/Infinity"),
    "unit": string("如 ratio/count"), "direction": enum("优化方向", "higher", "lower", "none")})
obj("ScoreResult", "统一评分结果；Python Scorer 返回同名 SDK 类的业务字段，Rust Worker 补全系统字段后才满足本传输 schema", {
    "status": enum("评分是否成功完成", "ok", "error"),
    "success": {"type": ["boolean", "null"], "description": "任务是否成功；无法确定时为 null"},
    "metrics": array(ref("Metric"), "命名指标列表，名称唯一"),
    "reward": {"type": ["number", "null"], "description": "本条 episode 的唯一奖励；评分成功时必须为有限数，评测展示与后训练复用该值"},
    "evidence": array(ref("ArtifactRef"), "评分证据"), "scorer": ref("ResolvedComponent"),
    "error": ref("ErrorRecord")}, ("error",))
for name in ('status', 'scorer', 'error'):
    prop = D['ScoreResult']['properties'][name]
    prop['description'] = prop.get('description', '') + '；系统维护，评分器不得填写；SDK 构造时为 None，最终传输按必填/可选规则校验'
D["ScoreResult"]["allOf"] = [
    {"if":{"properties":{"status":{"const":"error"}}},"then":{"properties":{"success":{"type":"null"},"reward":{"type":"null"}},"required":["error"]}},
    {"if":{"properties":{"status":{"const":"ok"}}},"then":{"properties":{"reward":{"type":"number"}},"not":{"required":["error"]}}}]
obj("Usage", "本 episode 截至当前 attempt 的累计用量；不同操作独立计数，基础设施重试不清零", {
    "generation_count": integer("模型生成调用次数"), "tool_call_count": integer("工具执行次数"),
    "environment_step_count": integer("环境转移次数"), "output_token_count": integer("实际生成 token 数")})
obj("EpisodeResult", "Worker 产生候选结果，Server 校验租约后形成唯一权威终态", {
    "run_id": ID, "episode_id": ID, "attempt_id": integer("有效 attempt", 1), "task_id": ID,
    "execution_status": enum("执行是否完成", "completed", "failed", "timeout", "cancelled"),
    "score": ref("ScoreResult", "episode 结束后产生的唯一正式评分结果"), "outcome": ref("Outcome"),
    "trajectory_ref": ref("ArtifactRef"), "usage": ref("Usage"),
    "started_at_ms": TIME, "finished_at_ms": TIME, "error": ref("ErrorRecord"),
    "cleanup_status": enum("清理可独立重试，不改变任务评分", "completed", "pending", "failed")}, ("score", "outcome", "error", "started_at_ms", "trajectory_ref"))
D["EpisodeResult"]["allOf"]=[{"if":{"properties":{"execution_status":{"const":"completed"}}},
    "then":{"required":["started_at_ms","trajectory_ref","outcome","score"]}}]

obj("GenerationEvent", "一次模型调用的原始记录；逐调用保留版本和 token 对齐", {
    "generation_id": ID, "model_id": string("实际模型"), "policy_version": string("实际策略版本"),
    "parameter_version": integer("实际参数版本序号"), "tokenizer": ref("ResolvedComponent"),
    "source": enum("真实或模拟", "real", "simulated"),
    "messages": array(ref("Message"), "该次真实输入消息"),
    "response": array(ref("ContentPart"), "该次原始输出"),
    "output_token_count": integer("本次实际生成 token 数；预算统计的唯一来源"),
    "input_token_ids": array(integer("token ID"), "真实输入 token 序列"),
    "output_token_ids": array(integer("token ID"), "真实生成 token 序列；存在时长度必须等于 output_token_count"),
    "output_logprobs": array(number("token 对数概率"), "长度必须等于 output_token_ids"),
    "loss_mask": array({"type": "integer", "enum": [0, 1], "description": "是否参与训练损失"}, "与生成 token 一一对应"),
    "finish_reason": enum("模型停止原因", "stop", "tool_calls", "length", "cancelled", "error"),
    "duration_ms": integer("模型调用耗时")}, ("policy_version", "parameter_version", "tokenizer", "input_token_ids", "output_token_ids", "output_logprobs", "loss_mask"))
D["GenerationEvent"]["dependentRequired"] = {
    "output_logprobs": ["output_token_ids"],
    "loss_mask": ["output_token_ids"],
}
obj("ToolCall", "规范工具调用；动态 arguments 使用工具声明的 schema 递归验证", {
    "tool_call_id": ID, "generation_id": ID, "implementation": ref("ResolvedComponent"),
    "name": string("ExecutionPlan.tools 中的唯一工具名；implementation 必须与该项一致"),
    "arguments": ref("TypedConfig"), "timeout_ms": integer("允许执行时间", 1)})
obj("ToolResult", "工具结果不等于 episode 结果", {
    "tool_call_id": ID, "status": enum("工具状态", "ok", "error", "timeout", "cancelled"),
    "content": array(ref("ContentPart"), "返回给 Agent 的内容"),
    "output_truncated": boolean("仅标记工具返回预览是否截断"),
    "raw_output_ref": ref("ArtifactRef"), "error": ref("ErrorRecord")}, ("raw_output_ref", "error"))
obj("EnvironmentTransition", "动作前事实与 Environment.step 返回值；动作后字段只在 Transition 中定义一次", {
    "environment_step_index": integer("从 0 开始"),
    "observation_before": ref("Observation", "动作前观测"),
    "action": ref("TypedConfig"),
    "transition": ref("Transition", "Environment.step 返回值的独立副本")})
obj("StateEvent", "基础生命周期事件", {"phase": enum("执行阶段", "preparing", "running", "scoring", "finalizing", "cleaning"), "session": ref("SessionRef")}, ("session",))
obj("TrajectoryEvent", "所有数据集共享事件信封，事件 payload 按 kind 绑定", {
    "schema_version": {"const": "vnext.3", "description": "契约版本"},
    "event_id": ID, "run_id": ID, "episode_id": ID, "attempt_id": integer("attempt 身份", 1),
    "task_id": ID, "sequence": integer("由 Worker 事件收集器分配的连续序号，从 0 开始"),
    "occurred_at_ms": TIME, "parent_event_id": ID,
    "kind": enum("事件类型", "state", "observation", "generation", "tool_call", "tool_result", "environment_transition", "score", "error", "terminal"),
    "payload": {"description": "由 kind 决定的确定类型"}}, ("parent_event_id",))
D["TrajectoryEvent"]["allOf"] = []
for kind, typ in {"state":"StateEvent", "observation":"Observation", "generation":"GenerationEvent", "tool_call":"ToolCall", "tool_result":"ToolResult", "environment_transition":"EnvironmentTransition", "score":"ScoreResult", "error":"ErrorRecord", "terminal":"EpisodeResult"}.items():
    D["TrajectoryEvent"]["allOf"].append({"if":{"properties":{"kind":{"const":kind}}},"then":{"properties":{"payload":ref(typ)}}})
obj("TrajectoryManifest", "attempt 的轨迹索引；评分前快照与最终封存使用同一结构，终态事件不反向包含此索引", {
    "schema_version": {"const": "vnext.3", "description": "契约版本"}, "run_id": ID, "episode_id": ID,
    "attempt_id": integer("attempt 身份", 1), "task_id": ID,
    "event_segments": array(ref("ArtifactRef"), "按 sequence 有序的 JSONL 事件分片"),
    "event_count": integer("事件总数"),
    "trajectory_status": enum("清单所处阶段及最终记录完整性；不表示任务执行成功或失败", "scoring_checkpoint", "final_complete", "final_partial"),
    "created_at_ms": TIME})
# Terminal trace payload must not contain its own final trajectory reference.
obj("TerminalEvent", "轨迹内终态摘要，避免与 EpisodeResult.trajectory_ref 形成 digest 环", {
    "execution_status": enum("终态", "completed", "failed", "timeout", "cancelled"),
    "usage": ref("Usage"), "error": ref("ErrorRecord")}, ("error",))
D["TrajectoryEvent"]["allOf"][-1]["then"]["properties"]["payload"] = ref("TerminalEvent")
obj("ToolSpec", "用户工具发布声明", {
    "implementation": ref("ComponentRef"), "entrypoint": string("Python 函数或 ToolExecutor 的 module:symbol 入口"),
    "config_schema": string("ToolExecutor 构造配置的精确 schema 标识"),
    "description": string("给 Agent 的说明"), "interfaces": array(ref("ToolInterface"), "至少一种标准接入方式，接口名在本工具内唯一"),
    "input_schema": ref("ArtifactRef"), "output_schema": ref("ArtifactRef"),
    "side_effect": enum("副作用与重试依据", "read_only", "idempotent", "non_idempotent")})
D["ToolSpec"]["properties"]["interfaces"].update(minItems=1)
obj("AgentManifest", "自定义 Agent 的发布声明；按接口复用工具，不逐个登记所有工具实现", {
    "implementation": ref("ComponentRef"), "entrypoint": string("AgentRunner 的 module:Class 入口"),
    "config_schema": string("AgentRunner 构造配置的精确 schema 标识"),
    "supported_interfaces": array(string("版本化工具接口；数组顺序就是兼容接口选择优先级"), "至少一项且不重复"),
    "required_tool_names": array(string("Agent 必须可见的内置工具名"), "允许为空；空数组表示无必需工具")})
D["AgentManifest"]["properties"]["supported_interfaces"].update(minItems=1, uniqueItems=True)
D["AgentManifest"]["properties"]["required_tool_names"].update(uniqueItems=True)
obj("EntryPoints", "环境包 Python 入口，均为 module:Class", {
    "dataset_adapter": string("原始行 -> PreparedSample"), "environment": string("Environment 实现"),
    "scorer": string("唯一单条 episode Scorer 实现；评测与后训练共用")})
obj("PackageManifest", "统一环境包，不绑定 agent 或 backend", {
    "runtime": ref("RuntimeSpec", "数据集包默认镜像候选"), "id": ID,
    "version": string("精确版本"), "entrypoints": ref("EntryPoints"),
    "task_schema": string("任务业务字段 schema 标识"), "private_schema": string("私有评分字段 schema 标识"),
    "config_schemas": {
        "type": "object",
        "description": "同一包内两个运行角色各自接受的配置 schema；角色名就是唯一索引",
        "properties": {
            "environment": string("Environment.config 接受的 schema 标识"),
            "scorer": string("Scorer.config 接受的 schema 标识"),
        },
        "required": ["environment", "scorer"],
        "additionalProperties": False,
    },
    "internet_access": boolean("Environment 是否需要公共互联网；数据行、RunSpec、Agent 和 Tool 不得覆盖"),
    "required_capabilities": array(string("运行能力名"), "部署所需的 Worker 功能；不写 Docker/OpenHands 名称，也不授予访问权限"),
    "artifacts": array(ref("ArtifactRef"), "代码 wheel 和显式声明的运行文件；镜像只由 runtime.image 引用"),
    "provided_tools": array(ref("ToolSpec"), "包可提供的工具声明；本次启用项只由 RunSpec.tools 决定"),
    "schemas": array(ref("ArtifactRef"), "包发布的带 $id 的 JSON Schema，注册前校验 digest")}, ("private_schema", "runtime"))
obj("WorkerRegistration", "Worker 声明能力，供 Server 匹配", {
    "worker_id": ID, "endpoint": string("Worker 地址"), "capacity": integer("可同时承载的 episode 槽位总数；不同于 CPU/内存/存储 resource_capacity", 1),
    "capabilities": array(string("能力名"), "运行能力"),
    "components": array(ref("ResolvedComponent"), "已安装可执行组件；包括后端，不另设后端清单"),
    "resource_capacity": ref("Resources", "Worker 可管理的 CPU/内存/存储总量；BackendSpec.resources 是单个 episode 的资源申请")})
obj("Heartbeat", "控制面健康与容量快照", {
    "worker_id": ID, "timestamp_ms": TIME, "active_leases": array(ID,"活跃 lease ID"),
    "available_slots": integer("当前可用 episode 槽位；由 WorkerRegistration.capacity 减去已占用槽位得到，不是第二份总容量"), "draining": boolean("不再接受新任务")})
obj("CancelRequest", "Server 或客户端取消请求；取消意图具有幂等性", {"request_id": ID, "episode_id": ID, "reason": string("取消原因")})
obj("ResultReport", "Worker 重传结果；Server 按 attempt+lease 去重和排除旧结果", {"lease": ref("Lease"), "result": ref("EpisodeResult")})
obj("Ack", "提交/上报/取消的确认", {"accepted": boolean("是否接受"), "code": string("OK/STALE_ATTEMPT/CONFLICT 等稳定码"), "message": string("说明")})
obj("CursorRequest", "查询或订阅结果/事件", {"run_id": ID, "episode_id": ID, "after_sequence": integer("已接收的最后序号"), "limit": integer("返回上限",1)}, ("episode_id","after_sequence"))
obj("ExecRequest", "任务后端执行命令，不包含数据集字段", {
    "session_id": ID, "argv": array(string("单个参数，不经过隐式 shell 拼接"), "参数列表"),
    "cwd": string("沙箱内路径"), "environment_variables": {"type":"object","additionalProperties":{"type":"string"},"description":"命令环境变量映射；密钥由受控注入，不入轨迹"},
    "timeout_ms": integer("命令预算",1)})
obj("ExecResult", "底层命令结果", {"exit_code": integer("进程退出码；信号退出另记 error",0),
    "stdout_ref": ref("ArtifactRef"), "stderr_ref": ref("ArtifactRef"), "duration_ms": integer("耗时"),
    "error": ref("ErrorRecord")}, ("error",))
obj("FrameworkSample", "Bridge 将统一轨迹映射为训练框架输入的规范中间结构", {
    "episode_id": ID, "attempt_id": integer("attempt",1),
    "generation_event_ids": array(ID,"使用的模型生成事件，保持顺序"),
    "reward": number("从唯一 ScoreResult.reward 复制的训练标量，不重新计算"),
    "policy_versions": array(string("策略版本"),"与 generation_event_ids 一一对应"),
    "trajectory_ref": ref("ArtifactRef")})
obj("HarnessRequest", "Scorer -> Worker 提供的 harness 执行能力", {
    "outcome": ref("Outcome", "与 ScoreInput.outcome 相同的完整评分快照，不另命名或只传 artifacts"),
    "private_data": ref("TypedConfig", "原样传入；唯一评测配置在 data.evaluation_plan，不新增顶层副本"),
    "remaining_timeout_ms": integer("剩余预算",1)})
obj("HarnessResult", "harness 结果，候选失败和 harness 错误不同", {
    "status": enum("评测程序是否正常完成", "ok", "error"),
    "success": {"type":["boolean","null"],"description":"正常完成时必须为布尔值"},
    "tests_run": integer("实际测试数"), "tests_passed": integer("通过数量"),
    "report_ref": ref("ArtifactRef"), "metrics":array(ref("Metric"),"可选的详细指标"),
    "error": ref("ErrorRecord")}, ("error","metrics"))
D['HarnessResult']['allOf']=[{'if':{'properties':{'status':{'const':'ok'}}},'then':{'properties':{'success':{'type':'boolean'}},'not':{'required':['error']}}},
    {'if':{'properties':{'status':{'const':'error'}}},'then':{'properties':{'success':{'type':'null'}},'required':['error']}}]

def main():
    target=ROOT/'contracts'; target.mkdir(parents=True,exist_ok=True)
    # Transitional local reference generator. Production target: core types come
    # from contracts/proto and package schemas are generated from package models.py.
    # Built-in examples remain separate from core dispatch types.
    core=copy.deepcopy(D)
    extensions=target/'extensions'; extensions.mkdir(exist_ok=True)
    for stale in extensions.glob('*.schema.json'):
        stale.unlink()
    def external_refs(value):
        if isinstance(value,dict):
            return {k:(f'uenv://schemas/vnext/{v.split("/")[-1]}' if v.split('/')[-1] in CONFIGS else 'urn:uenv:vnext:contracts'+v)
                    if k=='$ref' and v.startswith('#/$defs/') else external_refs(v) for k,v in value.items()}
        if isinstance(value,list): return [external_refs(x) for x in value]
        return value
    for name in CONFIGS:
        document={"$schema":"https://json-schema.org/draft/2020-12/schema", "$id":f"uenv://schemas/vnext/{name}", **external_refs(core.pop(name))}
        core.pop(name+'Envelope')
        (extensions/(name+'.schema.json')).write_text(json.dumps(document,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    schema={"$schema":"https://json-schema.org/draft/2020-12/schema", "$id":"urn:uenv:vnext:contracts", "$defs":external_refs(core)}
    (target/'uenv.schema.json').write_text(json.dumps(schema,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    lines=['# UEnv vNext 字段字典','', '本文件由本地过渡生成器 build_contracts.py 生成。目标生产版本改由 contracts/proto/uenv/v1/*.proto 生成；本文件不是另一处可编辑协议。所有对象默认拒绝未知字段；必填字段没有隐式默认值。run.yaml 展开后的完整 RunSpec 是唯一用户配置输入。', '',
           '嵌套字段的必填是指其父对象已提供时；可选父对象省略时无需补子字段。', '',
           'JSON 数字不得为 NaN/Infinity；时间统一毫秒。未提供的可选字段省略，不用空字符串代替 null；有明确允许空字符串的字段以定义为准。', '',
           'TypedConfig 的 data 不是任意 JSON：必须递归满足 schema_ref 指向的版本化 schema。表中 $ref 继续展开到同名结构。新扩展只在包内 models.py 定义，由发布工具生成并注册 schema，不在主流程增加名称分支。', '']
    for n,s in {**core, **{n:D[n] for n in CONFIGS}}.items():
        lines.extend([f'## {n}', '', s.get('description',''), ''])
        if 'properties' not in s:
            lines.extend(['联合类型：'+', '.join(v['$ref'].split('/')[-1] for v in s.get('oneOf',[])), '']); continue
        lines.extend(['| 字段 | 类型/嵌套结构 | 必填 | 含义/约束 |','|---|---|---|---|'])
        def field_rows(schema, prefix=''):
            for field, value in schema.get('properties', {}).items():
                yield prefix+field, value, field in schema.get('required', [])
                if value.get('type') == 'object':
                    yield from field_rows(value, prefix+field+'.')
        for k,v,required in field_rows(s):
            typ=v.get('$ref','').split('/')[-1] or str(v.get('type','按 discriminator 选择'))
            if v.get('type')=='array': typ='array<'+(v['items'].get('$ref','').split('/')[-1] or str(v['items'].get('type','value')))+'>'
            detail=v.get('description','')
            if 'enum' in v: detail+='；枚举：'+', '.join(map(str,v['enum']))
            if 'const' in v: detail+='；固定值：'+str(v['const'])
            if 'minimum' in v: detail+='；最小值：'+str(v['minimum'])
            lines.append(f"| `{k}` | {typ} | {'是' if required else '否'} | {detail.replace('|','/')} |")
        lines.append('')
    (ROOT/'field_dictionary.md').write_text('\n'.join(lines)+'\n',encoding='utf-8')
    print(f'Generated {len(core)} core types and {len(CONFIGS)} separately registered extension schemas')

if __name__=='__main__': main()
