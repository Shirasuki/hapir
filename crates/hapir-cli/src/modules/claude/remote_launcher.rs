use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::Value;
use tokio::select;
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, info, warn};

use hapir_shared::session::{
    AgentContent, ClaudeMessageBody, ClaudeOutputData, RoleWrappedMessage,
};

use crate::agent::loop_base::LoopResult;
use crate::modules::claude::permission_handler::PermissionHandler;
use crate::modules::claude::sdk::query;
use crate::modules::claude::sdk::types::{
    PermissionResult, QueryOptions, SdkMessage, StreamDelta, StreamEventBody,
};
use crate::modules::claude::session::ClaudeSession;

use super::run::ClaudeEnhancedMode;

/// 从 assistant 消息中跟踪的 tool_use 块，用于在收到
/// control_request 时解析 tool_use.id。
struct TrackedToolCall {
    id: String,
    name: String,
    input: Value,
    used: bool,
}

/// 通过匹配工具名和输入参数，从已跟踪的 tool call 中（最近的优先）
/// 解析权限请求对应的 tool_use.id。
fn resolve_tool_call_id(
    tracked: &mut Vec<TrackedToolCall>,
    tool_name: &str,
    input: &Option<Value>,
) -> Option<String> {
    let input_val = input.as_ref().cloned().unwrap_or(Value::Null);
    for call in tracked.iter_mut().rev() {
        if !call.used && call.name == tool_name && call.input == input_val {
            call.used = true;
            return Some(call.id.clone());
        }
    }
    None
}

/// Claude 远程启动器。
///
/// 以交互模式（--input-format stream-json）启动 `claude` CLI 进程，
/// 实现双向通信，支持流式输出和权限请求处理。
pub async fn claude_remote_launcher(
    session: &Arc<ClaudeSession<ClaudeEnhancedMode>>,
) -> LoopResult {
    let working_directory = session.base.path.clone();
    debug!("[claudeRemoteLauncher] Starting in {}", working_directory);

    let permission_handler = Arc::new(Mutex::new(PermissionHandler::new()));

    // 流式输出的打字机效果状态
    let mut streaming_message_id: Option<String> = None;

    // 跟踪每轮最后一条 assistant 消息，以便在收到 Result 时
    // 发送包含 thinking 块和 usage 的完整消息。
    let mut last_assistant_content: Option<Vec<Value>> = None;
    let mut last_assistant_usage: Option<Value> = None;

    // 跟踪 assistant 消息中的 tool_use 块，以便在收到 control_request 时
    // 解析 tool_use.id（SDK 的 request_id 与 tool_use.id 不同）。
    let mut tracked_tool_calls: Vec<TrackedToolCall> = Vec::new();
    // tool_use.id → SDK request_id 的映射（发送 control_response 时需要）
    let mut tool_use_to_request_id: HashMap<String, String> = HashMap::new();

    // 交互式查询句柄（首条消息时惰性启动）
    let mut query_handle: Option<query::InteractiveQuery> = None;
    let mut current_mode_hash: Option<String> = None;

    loop {
        info!("[claudeRemoteLauncher] Waiting for messages from queue...");

        // 等待队列消息或切换请求
        let batch = select! {
            batch = session.base.queue.wait_for_messages() => {
                match batch {
                    Some(b) => b,
                    None => {
                        debug!("[claudeRemoteLauncher] Queue closed, exiting");
                        if let Some(ref qh) = query_handle {
                            qh.close_stdin().await;
                        }
                        return LoopResult::Exit;
                    }
                }
            }
            _ = session.base.switch_notify.notified() => {
                info!("[claudeRemoteLauncher] Switch requested, stopping remote launcher");
                if let Some(ref qh) = query_handle {
                    qh.kill().await;
                }
                session.active_pid.store(0, Ordering::Relaxed);
                return LoopResult::Switch;
            }
        };

        let prompt = batch.message;
        let mode = batch.mode;
        let is_isolate = batch.isolate;

        // 如果 mode hash 变了（如模型或权限模式切换），
        // 杀掉现有进程以便用新参数重新启动。
        let mode_changed = current_mode_hash.as_ref().is_some_and(|h| *h != batch.hash);
        if mode_changed && query_handle.is_some() {
            info!(
                "[claudeRemoteLauncher] Mode changed ({} -> {}), restarting process",
                current_mode_hash.as_deref().unwrap_or("?"),
                batch.hash
            );
            if let Some(ref qh) = query_handle {
                qh.kill().await;
            }
            query_handle = None;
            session.active_pid.store(0, Ordering::Relaxed);
            streaming_message_id = None;
            tracked_tool_calls.clear();
            tool_use_to_request_id.clear();
        }
        current_mode_hash = Some(batch.hash.clone());

        debug!(
            "[claudeRemoteLauncher] Processing message (isolate={}): {}",
            is_isolate,
            if prompt.len() > 100 {
                format!("{}...", &prompt[..prompt.floor_char_boundary(100)])
            } else {
                prompt.clone()
            }
        );

        // 重置隔离消息的状态（如 /clear）
        if is_isolate {
            permission_handler.lock().await.reset();
            streaming_message_id = None;
            tracked_tool_calls.clear();
            tool_use_to_request_id.clear();
            // 关闭现有进程并强制重新启动
            if let Some(ref qh) = query_handle {
                qh.close_stdin().await;
            }
            query_handle = None;
            session.active_pid.store(0, Ordering::Relaxed);
        }

        // 启动或复用交互式 Claude 进程
        let needs_spawn = query_handle.is_none();
        if needs_spawn {
            let mut resume_id = session.base.session_id.lock().await.clone();

            // 如果还没有 session_id，检查 claude_args 中是否有 --resume 参数
            // （runner 恢复非活跃会话时传入）
            if resume_id.is_none() {
                if let Some(ref args) = *session.claude_args.lock().await {
                    if let Some(pos) = args.iter().position(|a| a == "--resume") {
                        resume_id = args.get(pos + 1).cloned();
                    }
                }
            }

            info!(
                "[claudeRemoteLauncher] Spawning: resume_id={:?}, mode_changed={}",
                resume_id, mode_changed
            );
            let query_options = QueryOptions {
                cwd: Some(working_directory.clone()),
                model: mode.model.clone(),
                fallback_model: mode.fallback_model.clone(),
                custom_system_prompt: mode.custom_system_prompt.clone(),
                append_system_prompt: mode.append_system_prompt.clone(),
                permission_mode: mode.permission_mode.clone(),
                allowed_tools: mode.allowed_tools.clone(),
                disallowed_tools: mode.disallowed_tools.clone(),
                continue_conversation: false,
                resume: resume_id,
                mcp_servers: if session.mcp_servers.is_empty() {
                    None
                } else {
                    Some(serde_json::to_value(&session.mcp_servers).unwrap_or_default())
                },
                settings_path: Some(session.hook_settings_path.clone()),
                ..Default::default()
            };

            info!(
                "[claudeRemoteLauncher] Spawning interactive claude SDK: resume={:?}, model={:?}",
                query_options.resume, query_options.model
            );
            match query::query_interactive(query_options) {
                Ok(qh) => {
                    session
                        .active_pid
                        .store(qh.pid().unwrap_or(0), Ordering::Relaxed);
                    query_handle = Some(qh);
                }
                Err(e) => {
                    warn!("[claudeRemoteLauncher] Failed to spawn claude: {}", e);
                    session
                        .base
                        .ws_client
                        .send_typed_message(&RoleWrappedMessage {
                            role: "assistant".into(),
                            content: AgentContent::Output {
                                data: ClaudeOutputData::Assistant {
                                    message: ClaudeMessageBody {
                                        role: "assistant".into(),
                                        content: serde_json::json!([{
                                            "type": "text",
                                            "text": format!("Failed to spawn claude SDK: {}", e),
                                        }]),
                                        usage: None,
                                    },
                                    parent_uuid: None,
                                },
                            },
                            meta: None,
                        })
                        .await;
                    continue;
                }
            }
        }

        let qh = query_handle.as_ref().unwrap();

        // 通过 stdin 发送用户消息
        if let Err(e) = qh.send_user_message(&prompt).await {
            warn!("[claudeRemoteLauncher] Failed to send message: {}", e);
            query_handle = None;
            session.active_pid.store(0, Ordering::Relaxed);
            continue;
        }

        session.base.on_thinking_change(true).await;

        // 处理 SDK 消息直到收到 Result（轮次结束）
        let mut got_result = false;
        let should_switch = false;

        while let Some(qh) = query_handle.as_mut() {
            let msg = match qh.next_message().await {
                Some(m) => m,
                None => {
                    debug!("[claudeRemoteLauncher] Process exited");
                    query_handle = None;
                    session.active_pid.store(0, Ordering::Relaxed);
                    break;
                }
            };

            match msg {
                SdkMessage::System {
                    subtype,
                    session_id,
                    tools,
                    slash_commands,
                    ..
                } => {
                    debug!("[claudeRemoteLauncher] System: subtype={}", subtype);
                    if let Some(ref sid) = session_id {
                        session.base.on_session_found(sid).await;
                    }
                    if tools.is_some() || slash_commands.is_some() {
                        let tools_clone = tools.clone();
                        let slash_clone = slash_commands.clone();
                        let _ = session
                            .base
                            .ws_client
                            .update_metadata(move |mut metadata| {
                                if let Some(t) = tools_clone {
                                    metadata.tools = Some(t);
                                }
                                if let Some(sc) = slash_clone {
                                    metadata.slash_commands = Some(sc);
                                }
                                metadata
                            })
                            .await;
                    }
                }
                SdkMessage::Assistant {
                    message,
                    parent_tool_use_id,
                } => {
                    debug!(
                        "[claudeRemoteLauncher] Assistant: blocks={}, parent={:?}",
                        message.content.len(),
                        parent_tool_use_id
                    );

                    // 始终跟踪最新的 content + usage 用于最终消息
                    last_assistant_content = Some(message.content.clone());
                    if message.usage.is_some() {
                        last_assistant_usage = message.usage.clone();
                    }

                    // 跟踪 tool_use 块用于权限 ID 解析
                    for block in &message.content {
                        if block.get("type").and_then(|v| v.as_str()) == Some("tool_use")
                            && let Some(id) = block.get("id").and_then(|v| v.as_str())
                        {
                            let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            let input = block.get("input").cloned().unwrap_or(Value::Null);
                            // 仅在未跟踪时添加
                            if !tracked_tool_calls.iter().any(|tc| tc.id == id) {
                                tracked_tool_calls.push(TrackedToolCall {
                                    id: id.to_string(),
                                    name: name.to_string(),
                                    input,
                                    used: false,
                                });
                            }
                        }
                    }

                    // 含非文本内容（tool_use 等）时，结束流并发送完整消息
                    let has_non_text = message.content.iter().any(|block| {
                        block.get("type").and_then(|v| v.as_str()) != Some("text")
                            && block.get("type").and_then(|v| v.as_str()) != Some("thinking")
                    });

                    if has_non_text {
                        if let Some(mid) = streaming_message_id.take() {
                            session
                                .base
                                .ws_client
                                .send_message_delta(&mid, "", true)
                                .await;
                        }

                        session
                            .base
                            .ws_client
                            .send_typed_message(&RoleWrappedMessage {
                                role: message.role.clone(),
                                content: AgentContent::Output {
                                    data: ClaudeOutputData::Assistant {
                                        message: ClaudeMessageBody {
                                            role: message.role.clone(),
                                            content: serde_json::to_value(&message.content)
                                                .unwrap_or_default(),
                                            usage: None,
                                        },
                                        parent_uuid: parent_tool_use_id.clone(),
                                    },
                                },
                                meta: None,
                            })
                            .await;
                    }
                    // 纯文本 assistant 消息不再在此处发送，
                    // 流式输出由 StreamEvent delta 处理
                }
                SdkMessage::User {
                    message,
                    parent_tool_use_id,
                } => {
                    debug!(
                        "[claudeRemoteLauncher] User: parent={:?}",
                        parent_tool_use_id
                    );
                    // 外层 role 用 "assistant" 以便前端通过
                    // normalizeAgentRecord → normalizeUserOutput 处理
                    // tool_result 内容块。内层 data.type="user" 保留原始语义。
                    session
                        .base
                        .ws_client
                        .send_typed_message(&RoleWrappedMessage {
                            role: "assistant".into(),
                            content: AgentContent::Output {
                                data: ClaudeOutputData::User {
                                    message: ClaudeMessageBody {
                                        role: message.role.clone(),
                                        content: serde_json::to_value(&message.content)
                                            .unwrap_or_default(),
                                        usage: None,
                                    },
                                    parent_uuid: parent_tool_use_id.clone(),
                                },
                            },
                            meta: None,
                        })
                        .await;
                }
                SdkMessage::Result {
                    subtype,
                    result,
                    num_turns,
                    total_cost_usd,
                    duration_ms,
                    is_error,
                    session_id: result_session_id,
                    usage: result_usage,
                    ..
                } => {
                    info!(
                        "[claudeRemoteLauncher] Result: subtype={}, turns={}, cost={}, duration={}ms, error={}",
                        subtype, num_turns, total_cost_usd, duration_ms, is_error
                    );

                    // 结束进行中的流
                    if let Some(mid) = streaming_message_id.take() {
                        session
                            .base
                            .ws_client
                            .send_message_delta(&mid, "", true)
                            .await;
                    }

                    // 优先使用 Result 的 usage（含 cache_* 字段），回退到最后一条 assistant 的 usage
                    let final_usage = result_usage
                        .and_then(|u| serde_json::to_value(u).ok())
                        .or(last_assistant_usage.take());

                    // 发送包含完整 content + usage 的最终消息。
                    // 包括流式 delta 跳过的 thinking 块。
                    if let Some(content) = last_assistant_content.take() {
                        let has_content = content
                            .iter()
                            .any(|b| b.get("type").and_then(|v| v.as_str()) != Some("thinking"));

                        if has_content || !is_error {
                            session
                                .base
                                .ws_client
                                .send_typed_message(&RoleWrappedMessage {
                                    role: "assistant".into(),
                                    content: AgentContent::Output {
                                        data: ClaudeOutputData::Assistant {
                                            message: ClaudeMessageBody {
                                                role: "assistant".into(),
                                                content: serde_json::to_value(&content)
                                                    .unwrap_or_default(),
                                                usage: final_usage.clone(),
                                            },
                                            parent_uuid: None,
                                        },
                                    },
                                    meta: None,
                                })
                                .await;
                        }
                    }

                    if is_error && let Some(ref error_text) = result {
                        session
                            .base
                            .ws_client
                            .send_typed_message(&RoleWrappedMessage {
                                role: "assistant".into(),
                                content: AgentContent::Output {
                                    data: ClaudeOutputData::Assistant {
                                        message: ClaudeMessageBody {
                                            role: "assistant".into(),
                                            content: serde_json::json!([{
                                                "type": "text",
                                                "text": error_text,
                                            }]),
                                            usage: None,
                                        },
                                        parent_uuid: None,
                                    },
                                },
                                meta: None,
                            })
                            .await;
                    }

                    if !result_session_id.is_empty() {
                        session.base.on_session_found(&result_session_id).await;
                    }

                    // 清理轮次状态
                    tracked_tool_calls.clear();
                    tool_use_to_request_id.clear();
                    last_assistant_content = None;
                    last_assistant_usage = None;

                    // 清除 agent state 中的 completedRequests，避免页面刷新后
                    // 残留的条目产生幽灵工具卡片。
                    let _ = session
                        .base
                        .ws_client
                        .update_agent_state(|mut state| {
                            if let Some(obj) = state.as_object_mut() {
                                obj.remove("completedRequests");
                            }
                            state
                        })
                        .await;

                    // 轮次结束 - 跳出循环等待下一条用户消息
                    got_result = true;
                    break;
                }
                SdkMessage::ControlRequest {
                    request_id,
                    request,
                } => {
                    let tool_name = request.tool_name.clone().unwrap_or_default();
                    let tool_input = request.input.clone();

                    // 从已跟踪的 assistant 消息中解析 tool_use.id。
                    // 前端通过 tool_use.id（而非 SDK 的 request_id）匹配权限和工具卡片。
                    let permission_key =
                        resolve_tool_call_id(&mut tracked_tool_calls, &tool_name, &tool_input)
                            .unwrap_or_else(|| request_id.clone());

                    info!(
                        "[claudeRemoteLauncher] Permission request: sdk_id={}, resolved_key={}, tool={}",
                        request_id, permission_key, tool_name
                    );

                    // 存储映射以便用 SDK 的 request_id 发送 control_response
                    tool_use_to_request_id.insert(permission_key.clone(), request_id.clone());

                    // 创建 oneshot channel 接收响应
                    let (tx, rx) =
                        oneshot::channel::<(bool, Option<Value>)>();
                    session
                        .pending_permissions
                        .lock()
                        .await
                        .insert(permission_key.clone(), tx);

                    // 更新 agent state 中的权限请求（以 tool_use.id 为 key）
                    let key_for_state = permission_key.clone();
                    let tool_name_for_state = tool_name.clone();
                    let tool_input_for_state = tool_input.clone();
                    let requested_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as f64;

                    let _ = session
                        .base
                        .ws_client
                        .update_agent_state(move |mut state| {
                            if let Some(obj) = state.as_object_mut() {
                                let requests = obj
                                    .entry("requests")
                                    .or_insert_with(|| serde_json::json!({}));
                                if let Some(req_map) = requests.as_object_mut() {
                                    req_map.insert(
                                        key_for_state,
                                        serde_json::json!({
                                            "tool": tool_name_for_state,
                                            "arguments": tool_input_for_state,
                                            "createdAt": requested_at,
                                        }),
                                    );
                                }
                            }
                            state
                        })
                        .await;

                    // 等待来自 Web UI 的权限响应
                    let sdk_rid = request_id;
                    match rx.await {
                        Ok((approved, updated_input)) => {
                            let qh_ref = query_handle.as_ref().unwrap();
                            if approved {
                                let input = updated_input
                                    .or(tool_input)
                                    .unwrap_or(serde_json::json!({}));
                                if let Err(e) = qh_ref
                                    .send_control_response(
                                        &sdk_rid,
                                        PermissionResult::Allow {
                                            updated_input: input,
                                        },
                                    )
                                    .await
                                {
                                    warn!(
                                        "[claudeRemoteLauncher] Failed to send permission response: {}",
                                        e
                                    );
                                }
                            } else {
                                if let Err(e) = qh_ref
                                    .send_control_error(&sdk_rid, "Permission denied by user")
                                    .await
                                {
                                    warn!(
                                        "[claudeRemoteLauncher] Failed to send permission denial: {}",
                                        e
                                    );
                                }
                            }
                        }
                        Err(_) => {
                            // channel 被丢弃（会话关闭中），拒绝请求
                            if let Some(qh_ref) = query_handle.as_ref() {
                                let _ =
                                    qh_ref.send_control_error(&sdk_rid, "Session closing").await;
                            }
                        }
                    }
                }
                SdkMessage::ControlResponse { response } => {
                    debug!(
                        "[claudeRemoteLauncher] Control response: id={}",
                        response.request_id
                    );
                }
                SdkMessage::ControlCancelRequest { request_id } => {
                    debug!("[claudeRemoteLauncher] Control cancel: id={}", request_id);
                    // 取消请求可能携带 SDK 的 request_id；找到我们映射的
                    // permission_key（tool_use.id）。
                    let permission_key = tool_use_to_request_id
                        .iter()
                        .find(|(_, v)| **v == request_id)
                        .map(|(k, _)| k.clone())
                        .unwrap_or_else(|| request_id.clone());

                    session
                        .pending_permissions
                        .lock()
                        .await
                        .remove(&permission_key);
                    tool_use_to_request_id.remove(&permission_key);

                    // 从 agent state 中移除
                    let _ = session
                        .base
                        .ws_client
                        .update_agent_state(move |mut state| {
                            if let Some(requests) =
                                state.get_mut("requests").and_then(|v| v.as_object_mut())
                            {
                                requests.remove(&permission_key);
                            }
                            state
                        })
                        .await;
                }
                SdkMessage::StreamEvent { event, .. } => match event {
                    StreamEventBody::ContentBlockDelta {
                        delta: StreamDelta::TextDelta { text },
                        ..
                    } => {
                        if streaming_message_id.is_none() {
                            streaming_message_id = Some(uuid::Uuid::new_v4().to_string());
                        }
                        let mid = streaming_message_id.as_ref().unwrap();
                        session
                            .base
                            .ws_client
                            .send_message_delta(mid, &text, false)
                            .await;
                    }
                    StreamEventBody::ContentBlockStop { .. } => {
                        // content block 结束，但不 finalize 流——
                        // 等 Result 或下一个非文本 Assistant 消息来 finalize
                    }
                    _ => {}
                },
                SdkMessage::Log { log } => {
                    debug!(
                        "[claudeRemoteLauncher] SDK log [{}]: {}",
                        log.level, log.message
                    );
                }
            }
        }

        session.base.on_thinking_change(false).await;
        // 仅在成功从 Claude 进程收到 session ID 后才消费 --resume。
        // 如果进程在发送 System 消息前被杀掉，session_id 仍为 None，
        // 重新启动时还需要 --resume。
        if session.base.session_id.lock().await.is_some() {
            session.consume_one_time_flags().await;
        }

        if should_switch {
            if let Some(ref qh) = query_handle {
                qh.close_stdin().await;
            }
            return LoopResult::Switch;
        }

        // 如果进程意外退出（没有 result），清除句柄以便下条消息重新启动
        if !got_result {
            query_handle = None;
            session.active_pid.store(0, Ordering::Relaxed);
        }

        if session.base.queue.is_closed().await {
            if let Some(ref qh) = query_handle {
                qh.close_stdin().await;
            }
            return LoopResult::Exit;
        }
    }
}
