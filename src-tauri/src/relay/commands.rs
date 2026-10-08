use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use super::{
    config::{self, RelayConfigUpdate, RelayConfigView},
    status::RelayStatus,
    transport, RelayError, MANAGER,
};
use crate::web::event_bridge::EventEmitter;

pub async fn get_relay_config_core(
    conn: &DatabaseConnection,
) -> Result<RelayConfigView, RelayError> {
    Ok(config::load(conn).await?.view())
}

pub async fn update_relay_config_core(
    conn: &DatabaseConnection,
    emitter: EventEmitter,
    config: RelayConfigUpdate,
) -> Result<RelayConfigView, RelayError> {
    MANAGER.update(conn, emitter, config).await
}

pub async fn get_relay_status_core(
    conn: &DatabaseConnection,
    emitter: EventEmitter,
) -> RelayStatus {
    MANAGER.refresh_if_offline(conn, emitter).await;
    MANAGER.status.snapshot()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayTestConfig {
    pub url: String,
    pub key: Option<String>,
    pub fingerprint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RelayTestResult {
    pub ok: bool,
    pub fingerprint: Option<String>,
    pub rtt_ms: Option<u64>,
    pub error: Option<String>,
    pub error_code: Option<&'static str>,
}

pub async fn test_relay_connection_core(
    conn: &DatabaseConnection,
    params: RelayTestConfig,
) -> Result<RelayTestResult, RelayError> {
    let key = match params.key {
        Some(key) => key,
        None => config::load(conn).await?.key,
    };
    let mut config = config::RelayConfig {
        enabled: true,
        url: params.url,
        key,
        fingerprint: params.fingerprint,
        ..Default::default()
    };
    config.validate()?;
    if let Some(result) = MANAGER.test_existing(&config).await {
        return Ok(result);
    }
    Ok(match transport::connect(&config).await {
        Ok(mut connection) => {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                connection.socket.close(None),
            )
            .await;
            RelayTestResult {
                ok: true,
                fingerprint: connection.fingerprint,
                rtt_ms: Some(connection.rtt_ms),
                error: None,
                error_code: None,
            }
        }
        Err(error) => RelayTestResult {
            ok: false,
            fingerprint: error.fingerprint,
            rtt_ms: None,
            error: Some(error.message.into()),
            error_code: Some(error.code),
        },
    })
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn get_relay_config(
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<RelayConfigView, RelayError> {
    get_relay_config_core(&db.conn).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn update_relay_config(
    app: tauri::AppHandle,
    db: tauri::State<'_, crate::db::AppDatabase>,
    config: RelayConfigUpdate,
) -> Result<RelayConfigView, RelayError> {
    update_relay_config_core(&db.conn, EventEmitter::Tauri(app), config).await
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn get_relay_status(
    app: tauri::AppHandle,
    db: tauri::State<'_, crate::db::AppDatabase>,
) -> Result<RelayStatus, RelayError> {
    Ok(get_relay_status_core(&db.conn, EventEmitter::Tauri(app)).await)
}

#[cfg(feature = "tauri-runtime")]
#[tauri::command]
pub async fn test_relay_connection(
    db: tauri::State<'_, crate::db::AppDatabase>,
    config: RelayTestConfig,
) -> Result<RelayTestResult, RelayError> {
    test_relay_connection_core(&db.conn, config).await
}
