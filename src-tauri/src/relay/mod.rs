pub mod client;
pub mod commands;
pub mod config;
pub mod serve;
pub mod status;
pub mod transport;

use crate::web::event_bridge::EventEmitter;
use axum::Router;
use config::{RelayConfig, RelayConfigUpdate, RelayConfigView};
use sea_orm::DatabaseConnection;
use status::{RelayState, StatusStore};
use std::sync::{Arc, LazyLock};
use tokio::{sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub static MANAGER: LazyLock<Arc<RelayManager>> =
    LazyLock::new(|| Arc::new(RelayManager::default()));

#[derive(Default)]
struct Runtime {
    router: Option<Router>,
    config: RelayConfig,
    active_config: Option<RelayConfig>,
    task: Option<(CancellationToken, JoinHandle<()>)>,
    restart: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub struct RelayManager {
    runtime: Mutex<Runtime>,
    pub status: Arc<StatusStore>,
}

impl RelayManager {
    async fn stop_task(runtime: &mut Runtime) {
        runtime.active_config = None;
        if let Some((cancel, task)) = runtime.task.take() {
            cancel.cancel();
            let _ = task.await;
        }
    }

    async fn apply(&self, runtime: &mut Runtime, conn: &DatabaseConnection) {
        Self::stop_task(runtime).await;
        match config::load(conn).await {
            Ok(config) => runtime.config = config,
            Err(error) => {
                self.status.transition(
                    RelayState::Error,
                    None,
                    Some(error.message.into()),
                    None,
                    None,
                );
                return;
            }
        }
        let config = &runtime.config;
        let url = (!config.url.is_empty()).then(|| config.url.clone());
        if !config.enabled {
            self.status
                .transition(RelayState::Disabled, url, None, None, None);
        } else if let Some(router) = &runtime.router {
            runtime.active_config = Some(config.clone());
            let cancel = CancellationToken::new();
            let task = tokio::spawn(client::run(
                config.clone(),
                router.clone(),
                self.status.clone(),
                cancel.clone(),
            ));
            runtime.task = Some((cancel, task));
        } else {
            self.status
                .transition(RelayState::WaitingForWebService, url, None, None, None);
        }
    }

    pub async fn web_started(
        &self,
        conn: &DatabaseConnection,
        emitter: EventEmitter,
        router: Router,
    ) {
        let mut runtime = self.runtime.lock().await;
        if let Some(task) = runtime.restart.take() {
            task.abort();
        }
        self.status.set_emitter(emitter);
        runtime.router = Some(router);
        self.apply(&mut runtime, conn).await;
    }

    pub async fn web_stopped(&self) {
        let mut runtime = self.runtime.lock().await;
        if let Some(task) = runtime.restart.take() {
            task.abort();
        }
        Self::stop_task(&mut runtime).await;
        runtime.router = None;
        self.status.transition(
            if runtime.config.enabled {
                RelayState::WaitingForWebService
            } else {
                RelayState::Disabled
            },
            (!runtime.config.url.is_empty()).then(|| runtime.config.url.clone()),
            None,
            None,
            None,
        );
    }

    pub async fn refresh_if_offline(&self, conn: &DatabaseConnection, emitter: EventEmitter) {
        let mut runtime = self.runtime.lock().await;
        self.status.set_emitter(emitter);
        if runtime.router.is_none() {
            self.apply(&mut runtime, conn).await;
        }
    }

    pub async fn update(
        self: &Arc<Self>,
        conn: &DatabaseConnection,
        emitter: EventEmitter,
        update: RelayConfigUpdate,
    ) -> Result<RelayConfigView, RelayError> {
        let mut runtime = self.runtime.lock().await;
        if config::RelayEnv::capture().is_present() {
            return Err(RelayError::new(
                "relay_config_from_env",
                "Relay configuration is controlled by environment variables",
            ));
        }
        let mut config = config::load(conn).await?;
        config.apply(update)?;
        config::save(conn, &config).await?;
        self.status.set_emitter(emitter);
        runtime.config = config.clone();
        if let Some(task) = runtime.restart.take() {
            task.abort();
        }
        if runtime.router.is_none() {
            self.apply(&mut runtime, conn).await;
        } else {
            // Let a save submitted through the tunnel flush its acknowledgement
            // before replacing that transport. Repeated saves debounce to latest DB state.
            let manager = self.clone();
            let conn = conn.clone();
            runtime.restart = Some(tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let mut runtime = manager.runtime.lock().await;
                manager.apply(&mut runtime, &conn).await;
            }));
        }
        Ok(config.view())
    }

    pub async fn test_existing(&self, config: &RelayConfig) -> Option<commands::RelayTestResult> {
        let runtime = self.runtime.lock().await;
        let current = runtime.active_config.as_ref()?;
        let status = self.status.snapshot();
        if status.state != RelayState::Connected
            || current.url != config.url
            || current.key != config.key
            || current.fingerprint != config.fingerprint
        {
            return None;
        }
        Some(commands::RelayTestResult {
            ok: true,
            fingerprint: status.relay_cert_fingerprint,
            rtt_ms: status.last_rtt_ms,
            error: None,
            error_code: None,
        })
    }
}

#[cfg(test)]
mod tests;

#[derive(Debug, serde::Serialize, thiserror::Error)]
#[error("{message}")]
pub struct RelayError {
    pub code: &'static str,
    pub message: &'static str,
}

impl RelayError {
    pub fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
    pub fn database<T>(_: T) -> Self {
        Self::new("database_error", "Could not access relay configuration")
    }
}

impl axum::response::IntoResponse for RelayError {
    fn into_response(self) -> axum::response::Response {
        let status = match self.code {
            "relay_config_from_env" => axum::http::StatusCode::CONFLICT,
            "database_error" => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            _ => axum::http::StatusCode::BAD_REQUEST,
        };
        (status, axum::Json(self)).into_response()
    }
}
