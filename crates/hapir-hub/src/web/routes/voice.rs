use std::collections::HashMap;
use std::sync::LazyLock;

use axum::{Json, Router, extract::State, routing::post};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use hapir_shared::frontend::api::{ApiError, ApiResponse};
use hapir_shared::frontend::response_types::VoiceTokenData;
use hapir_shared::hub::voice::{ELEVENLABS_API_BASE, VOICE_AGENT_NAME, build_voice_agent_config};

use crate::web::AppState;

static AGENT_ID_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

pub fn router() -> Router<AppState> {
    Router::new().route("/voice/token", post(voice_token))
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct VoiceTokenRequest {
    custom_agent_id: Option<String>,
    custom_api_key: Option<String>,
}

fn cache_key(api_key: &str) -> String {
    if api_key.len() >= 8 {
        format!("{}...{}", &api_key[..4], &api_key[api_key.len() - 4..])
    } else {
        api_key.to_string()
    }
}

async fn find_agent(client: &reqwest::Client, api_key: &str) -> Option<String> {
    let url = format!("{ELEVENLABS_API_BASE}/convai/agents");
    let resp = client
        .get(&url)
        .header("xi-api-key", api_key)
        .header("Accept", "application/json")
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let data: Value = resp.json().await.ok()?;
    let agents = data.get("agents")?.as_array()?;
    agents.iter().find_map(|a| {
        if a.get("name")?.as_str()? == VOICE_AGENT_NAME {
            Some(a.get("agent_id")?.as_str()?.to_string())
        } else {
            None
        }
    })
}

async fn create_agent(client: &reqwest::Client, api_key: &str) -> Option<String> {
    let url = format!("{ELEVENLABS_API_BASE}/convai/agents/create");
    let config = build_voice_agent_config();
    let body = serde_json::to_value(&config).ok()?;

    let resp = client
        .post(&url)
        .header("xi-api-key", api_key)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        let status = resp.status();
        let error_data: Value = resp.json().await.unwrap_or_default();
        let error_message = error_data
            .get("detail")
            .and_then(|d| {
                d.as_str().map(|s| s.to_string()).or_else(|| {
                    d.get("message")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
            })
            .unwrap_or_else(|| format!("API error: {status}"));
        tracing::error!("[Voice] Failed to create agent: {error_message}");
        return None;
    }

    let data: Value = resp.json().await.ok()?;
    data.get("agent_id")?.as_str().map(|s| s.to_string())
}

async fn get_or_create_agent_id(client: &reqwest::Client, api_key: &str) -> Option<String> {
    let key = cache_key(api_key);

    {
        let cache = AGENT_ID_CACHE.lock().await;
        if let Some(id) = cache.get(&key) {
            return Some(id.clone());
        }
    }

    tracing::info!("[Voice] No agent ID configured, searching for existing agent...");
    let mut agent_id = find_agent(client, api_key).await;

    if let Some(ref id) = agent_id {
        tracing::info!("[Voice] Found existing agent: {id}");
    } else {
        tracing::info!("[Voice] No existing agent found, creating new one...");
        agent_id = create_agent(client, api_key).await;
        if let Some(ref id) = agent_id {
            tracing::info!("[Voice] Created new agent: {id}");
        }
    }

    if let Some(ref id) = agent_id {
        let mut cache = AGENT_ID_CACHE.lock().await;
        cache.insert(key, id.clone());
    }

    agent_id
}

async fn voice_token(
    State(_state): State<AppState>,
    body: Option<Json<VoiceTokenRequest>>,
) -> Result<Json<ApiResponse<VoiceTokenData>>, ApiError> {
    let req = body.map(|Json(b)| b).unwrap_or_default();

    let api_key = req
        .custom_api_key
        .or_else(|| std::env::var("ELEVENLABS_API_KEY").ok());

    let api_key = match api_key {
        Some(k) if !k.is_empty() => k,
        _ => {
            return Err(ApiError::BadRequest(
                "ElevenLabs API key not configured".into(),
            ));
        }
    };

    let client = &*HTTP_CLIENT;

    let mut agent_id = req
        .custom_agent_id
        .or_else(|| std::env::var("ELEVENLABS_AGENT_ID").ok())
        .filter(|s| !s.is_empty());

    if agent_id.is_none() {
        agent_id = get_or_create_agent_id(client, &api_key).await;
        if agent_id.is_none() {
            return Err(ApiError::Internal(
                "Failed to create ElevenLabs agent automatically".into(),
            ));
        }
    }

    let agent_id = agent_id.unwrap();

    let token_url = format!(
        "https://api.elevenlabs.io/v1/convai/conversation/token?agent_id={}",
        urlencoding::encode(&agent_id)
    );

    let resp = match client
        .get(&token_url)
        .header("xi-api-key", &api_key)
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[Voice] Error fetching token: {e}");
            return Err(ApiError::Internal(e.to_string()));
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let error_data: Value = resp.json().await.unwrap_or_default();
        let msg = error_data
            .pointer("/detail/message")
            .or_else(|| error_data.get("error"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("ElevenLabs API error: {status}"));
        tracing::error!("[Voice] Failed to get token from ElevenLabs: {msg}");
        return Err(ApiError::Internal(msg));
    }

    let data: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return Err(ApiError::Internal(e.to_string()));
        }
    };

    match data.get("token").and_then(|t| t.as_str()) {
        Some(token) => Ok(Json(ApiResponse::ok(VoiceTokenData {
            token: token.to_string(),
            agent_id,
        }))),
        None => Err(ApiError::Internal("No token in ElevenLabs response".into())),
    }
}
