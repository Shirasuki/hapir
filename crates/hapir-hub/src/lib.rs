pub mod config;
pub mod notifications;
pub mod store;
pub mod sync;
pub mod telegram;
pub mod web;
pub mod ws;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::{Notify, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use config::{HubConfiguration, VapidKeys};
use store::Store;
use sync::SyncEngine;
use telegram::bot::HappyBot;
use web::AppState;
use ws::WsState;
use ws::connection_manager::ConnectionManager;
use ws::terminal_registry::TerminalRegistry;

const SESSION_EXPIRE_INTERVAL: Duration = Duration::from_secs(5);
const TERMINAL_IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(60);
const SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

struct HubCore {
    config: HubConfiguration,
    store: Arc<Store>,
    jwt_secret: Vec<u8>,
    vapid_keys: Option<VapidKeys>,
}

struct HubEngine {
    conn_mgr: Arc<ConnectionManager>,
    sync_engine: Arc<SyncEngine>,
    happy_bot: Option<Arc<HappyBot>>,
}

// --- 后台任务生命周期管理 ---

struct BackgroundTasks {
    handles: Vec<(&'static str, JoinHandle<()>)>,
}

impl BackgroundTasks {
    fn new() -> Self {
        Self {
            handles: Vec::new(),
        }
    }

    fn spawn(&mut self, name: &'static str, fut: impl Future<Output = ()> + Send + 'static) {
        self.handles.push((name, tokio::spawn(fut)));
    }

    async fn shutdown(self) {
        for (name, handle) in self.handles {
            handle.abort();
            match handle.await {
                Ok(()) => {}
                Err(e) if e.is_cancelled() => {}
                Err(e) => warn!(task = name, error = %e, "background task panicked"),
            }
        }
    }
}

fn init_core(config: HubConfiguration) -> anyhow::Result<HubCore> {
    let db_path_str = config.db_path.to_string_lossy().to_string();
    let store = Arc::new(Store::new(&db_path_str)?);
    let jwt_secret = config::jwt_secret::get_or_create_jwt_secret(&config.data_dir)?;
    let vapid_keys = config::vapid_keys::get_or_create_vapid_keys(&config.data_dir).ok();
    Ok(HubCore {
        config,
        store,
        jwt_secret,
        vapid_keys,
    })
}

async fn init_engine(core: &HubCore) -> HubEngine {
    let conn_mgr = Arc::new(ConnectionManager::new());
    let sync_engine = Arc::new(SyncEngine::new(core.store.clone(), conn_mgr.clone()));

    let setup = notifications::setup::build(
        &core.config,
        core.vapid_keys.clone(),
        core.store.clone(),
        sync_engine.clone(),
    );
    setup
        .notification_hub
        .clone()
        .start(sync_engine.clone())
        .await;

    HubEngine {
        conn_mgr,
        sync_engine,
        happy_bot: setup.happy_bot,
    }
}

fn build_app(core: &HubCore, engine: &HubEngine) -> (Router, Arc<RwLock<TerminalRegistry>>) {
    let vapid_public_key = core.vapid_keys.as_ref().map(|k| k.public_key.clone());

    let app_state = AppState {
        jwt_secret: core.jwt_secret.clone(),
        cli_api_token: core.config.cli_api_token.clone(),
        sync_engine: engine.sync_engine.clone(),
        store: core.store.clone(),
        vapid_public_key,
        telegram_bot_token: core.config.telegram_bot_token.clone(),
        data_dir: core.config.data_dir.clone(),
        cors_origins: core.config.cors_origins.clone(),
    };

    let terminal_registry = Arc::new(RwLock::new(TerminalRegistry::new(
        ws::DEFAULT_IDLE_TIMEOUT_MS,
    )));
    let ws_state = WsState {
        store: core.store.clone(),
        sync_engine: engine.sync_engine.clone(),
        conn_mgr: engine.conn_mgr.clone(),
        terminal_registry: terminal_registry.clone(),
        cli_api_token: core.config.cli_api_token.clone(),
        jwt_secret: core.jwt_secret.clone(),
        max_terminals_per_socket: ws::DEFAULT_MAX_TERMINALS,
        max_terminals_per_session: ws::DEFAULT_MAX_TERMINALS,
    };

    let web_router = web::build_router(app_state);
    let ws_router = ws::ws_router(ws_state);
    (web_router.merge(ws_router), terminal_registry)
}

fn spawn_background_tasks(
    sync_engine: &Arc<SyncEngine>,
    conn_mgr: &Arc<ConnectionManager>,
    terminal_registry: &Arc<RwLock<TerminalRegistry>>,
) -> BackgroundTasks {
    let mut tasks = BackgroundTasks::new();

    let se = sync_engine.clone();
    tasks.spawn("session-expire", async move {
        let mut interval = tokio::time::interval(SESSION_EXPIRE_INTERVAL);
        loop {
            interval.tick().await;
            se.expire_inactive().await;
        }
    });

    let tr = terminal_registry.clone();
    let cm = conn_mgr.clone();
    tasks.spawn("terminal-idle", async move {
        let mut interval = tokio::time::interval(TERMINAL_IDLE_CHECK_INTERVAL);
        loop {
            interval.tick().await;
            let actions = tr.write().await.drain_idle();
            for action in actions {
                cm.send_to(&action.socket_id, &action.err_msg).await;
                cm.send_to(&action.cli_socket_id, &action.close_msg).await;
            }
        }
    });

    let se2 = sync_engine.clone();
    tasks.spawn("sse-heartbeat", async move {
        let mut interval = tokio::time::interval(SSE_HEARTBEAT_INTERVAL);
        loop {
            interval.tick().await;
            if se2.sse_connection_count() > 0 {
                se2.send_heartbeats();
            }
        }
    });

    tasks
}

// --- 入口 ---

pub async fn run_hub() -> anyhow::Result<()> {
    let config = HubConfiguration::new().await?;
    info!(
        port = config.listen_port,
        host = %config.listen_host,
        public_url = %config.public_url,
        telegram = config.telegram_enabled,
        "starting hub"
    );

    let core = init_core(config)?;
    let engine = init_engine(&core).await;
    let (app, terminal_registry) = build_app(&core, &engine);
    let bg_tasks =
        spawn_background_tasks(&engine.sync_engine, &engine.conn_mgr, &terminal_registry);

    if core.config.cli_api_token_is_new {
        info!(token = %core.config.cli_api_token, "generated new CLI API token");
    }

    let addr = format!("{}:{}", core.config.listen_host, core.config.listen_port);
    let listener = TcpListener::bind(&addr).await?;
    info!(addr = %addr, "listening");

    if let Some(ref bot) = engine.happy_bot {
        bot.start();
    }
    info!(url = %core.config.public_url, "hub ready");

    let shutdown_notify = Arc::new(Notify::new());
    let shutdown_notify_srv = shutdown_notify.clone();
    let server_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_notify_srv.notified().await;
            })
            .await
    });

    shutdown_signal().await;

    // 先停止接受新连接，再关闭已有连接
    shutdown_notify.notify_one();
    info!("closing WebSocket connections");
    engine.conn_mgr.close_all().await;

    if tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, server_task)
        .await
        .is_err()
    {
        info!("graceful shutdown timed out, forcing exit");
    }

    bg_tasks.shutdown().await;

    if let Some(ref bot) = engine.happy_bot {
        bot.stop();
    }

    info!("hub stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("shutdown signal received");
}
