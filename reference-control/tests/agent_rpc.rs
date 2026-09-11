//! Opt-in dependency integration. Model HTTP responses are scripted in Python;
//! Conversation, MCP discovery/call and Rust/Python communication are real.
use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;
use uenv_reference_control::contracts::ContractSchema;
use uenv_reference_control::ports::{AgentHost, NEVER_CANCELLED, SystemClock, ToolHost};
use uenv_reference_control::rpc::{RpcAgentHost, RpcModelProvider, RpcProcess, RpcToolHost};
use uenv_reference_control::runtime::{AgentRuntime, BudgetEnforcer, TrajectoryWriter};

fn run_session(openhands: bool, max_tools: u64) {
    let python = std::env::var("UENV_INTEGRATION_PYTHON")
        .expect("Set UENV_INTEGRATION_PYTHON to the pinned venv Python");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let paths = std::env::join_paths([
        root.join("reference/sdk/src"),
        root.join("reference/sdk/tests"),
        root.join("reference"),
    ])
    .unwrap();
    let mut command = Command::new(python);
    command
        .args([
            "-m",
            "uenv.sdk.component_host",
            "integration_components:create_host",
        ])
        .env("PYTHONPATH", paths.clone())
        .env("PYTHONUNBUFFERED", "1")
        .env("LITELLM_LOCAL_MODEL_COST_MAP", "True")
        .env("DO_NOT_TRACK", "1");
    let process = RpcProcess::spawn(&mut command).unwrap();
    let mut model_command = Command::new(std::env::var("UENV_INTEGRATION_PYTHON").unwrap());
    model_command
        .args([
            "-m",
            "uenv.sdk.component_host",
            "integration_components:create_model_host",
        ])
        .env("PYTHONPATH", paths)
        .env("LITELLM_LOCAL_MODEL_COST_MAP", "True");
    let model_process = RpcProcess::spawn(&mut model_command).unwrap();
    let info = process.call("fixture.info", json!({}), 60000).unwrap();
    let mut plan: Value = serde_json::from_str(
        &std::fs::read_to_string(
            root.join("reference/generated/episodes/gsm8k/execution_plan.json"),
        )
        .unwrap(),
    )
    .unwrap();
    plan["model"]["endpoint"] = info["endpoint"].clone();
    plan["limits"]["max_generations"] = json!(4);
    plan["limits"]["max_tool_calls"] = json!(max_tools);
    plan["limits"]["finalize_reserve_ms"] = json!(1000);
    let digest = format!("sha256:{}", "a".repeat(64));
    let owner = json!({"id":"agents/openhands","version":"1.0.0","digest":digest});
    if openhands {
        plan["agent"] = json!({"implementation":owner,"config":{"schema_ref":"uenv://schemas/vnext/OpenHandsAgentConfig","data":{"system_prompt":"Use only the selected tools."}}});
    }
    let add = json!({"name":"add","implementation":{"id":"tools/add","version":"1.0.0","digest":digest},
        "config":{"schema_ref":"uenv://schemas/vnext/EmptyConfig","data":{}},"execution_scope":"agent_state","required_capabilities":[]});
    let finish = json!({"name":"finish","implementation":{"id":"agents/openhands/tools/finish","version":"1.0.0","digest":digest},
        "config":{"schema_ref":"uenv://schemas/vnext/EmptyConfig","data":{}},"execution_scope":"agent_state","required_capabilities":[],"native_agent":owner});
    plan["tools"] = if openhands {
        json!([add, finish])
    } else {
        json!([add])
    };
    let bindings = plan["tools"].as_array().unwrap();
    let mut tools = RpcToolHost(process.clone());
    let mut agent = RpcAgentHost(process.clone());
    let mut model = RpcModelProvider(model_process);
    assert_eq!(
        tools.prepare(bindings, &json!({}), 60000).unwrap(),
        *bindings
    );
    assert_eq!(
        agent
            .prepare(
                &plan["agent"],
                bindings,
                plan["model"]["model_id"].as_str().unwrap(),
                60000
            )
            .unwrap(),
        *bindings
    );
    let schema = ContractSchema::bundled();
    let clock = SystemClock;
    let dispatch = json!({"plan":plan,"remaining_timeout_ms":60000,
        "consumed_usage":{"generation_count":0,"tool_call_count":0,"output_token_count":0}});
    let mut budget = BudgetEnforcer::from_dispatch(&dispatch, &clock).unwrap();
    let mut trajectory = TrajectoryWriter::from_plan(&plan, &schema).unwrap();
    let observation = json!({"content":[{"kind":"text","text":"Add 2 and 3, then report the result."}],"terminated":false,"episode_truncated":false});
    let mut runtime = AgentRuntime::new(
        &plan,
        &schema,
        &clock,
        &NEVER_CANCELLED,
        &mut tools,
        &mut model,
        &mut budget,
        &mut trajectory,
        &observation,
    )
    .unwrap();
    let answer = agent
        .run_agent(&plan["task"], &observation, &mut runtime, 60000)
        .unwrap();
    let usage = runtime.usage().to_value();
    drop(runtime);
    let stats = process.call("fixture.stats", json!({}), 5000).unwrap();
    assert_eq!(
        stats["executions"],
        if max_tools > 0 {
            json!([[2, 3]])
        } else {
            json!([])
        }
    );
    assert_eq!(
        stats["model_requests"][0]["model"],
        plan["model"]["model_id"]
    );
    assert_eq!(
        stats["model_requests"][0]["max_tokens"],
        plan["model"]["generation"]["max_output_tokens"]
    );
    if openhands {
        assert_eq!(stats["mcp_calls"], 1);
        assert_eq!(stats["unauthorized_status"], 401);
        assert_eq!(
            stats["native_executions"],
            if max_tools > 1 { 1 } else { 0 }
        );
        assert_eq!(stats["native_returns"], stats["native_executions"]);
        assert_eq!(stats["native_result_cache"], 0);
    }
    if max_tools > 1 {
        assert_eq!(answer, json!([{"kind":"text","text":"5"}]));
        assert_eq!(usage["generation_count"], 2);
    }
    let calls = trajectory
        .events()
        .iter()
        .filter(|e| e["kind"] == "tool_call")
        .count();
    let results = trajectory
        .events()
        .iter()
        .filter(|e| e["kind"] == "tool_result")
        .count();
    assert_eq!(calls, results);
    assert_eq!(
        calls as u64,
        if openhands {
            max_tools.min(2)
        } else {
            max_tools.min(1)
        }
    );
    tools.freeze(5000).unwrap();
    agent.close().unwrap();
    println!(
        "{}",
        json!({"openhands":openhands,"max_tool_calls":max_tools,"usage":usage,
        "mcp_calls":stats["mcp_calls"],"native_executions":stats["native_executions"],"events":trajectory.events().len()})
    );
}

#[test]
#[ignore = "requires pinned OpenHands/MCP Python environment"]
fn real_openhands_mcp_native_and_worker_rpc() {
    run_session(true, 4);
}

#[test]
#[ignore = "requires pinned OpenHands/MCP Python environment"]
fn real_openhands_mcp_denial_does_not_execute() {
    run_session(true, 0);
}

#[test]
#[ignore = "requires pinned OpenHands/MCP Python environment"]
fn real_openhands_native_denial_does_not_execute() {
    run_session(true, 1);
}

#[test]
#[ignore = "requires pinned integration Python environment"]
fn real_plain_agent_uses_same_worker_rpc() {
    run_session(false, 4);
}
