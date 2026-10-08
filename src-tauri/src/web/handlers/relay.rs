use crate::{
    app_state::AppState,
    relay::{
        commands,
        config::{RelayConfigUpdate, RelayConfigView},
        status::RelayStatus,
        RelayError,
    },
};
use axum::{Extension, Json};
use serde::Deserialize;
use std::sync::Arc;

pub async fn get_relay_config(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<RelayConfigView>, RelayError> {
    commands::get_relay_config_core(&state.db.conn)
        .await
        .map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateParams {
    pub config: RelayConfigUpdate,
}

pub async fn update_relay_config(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateParams>,
) -> Result<Json<RelayConfigView>, RelayError> {
    commands::update_relay_config_core(&state.db.conn, state.emitter.clone(), params.config)
        .await
        .map(Json)
}

pub async fn get_relay_status(Extension(state): Extension<Arc<AppState>>) -> Json<RelayStatus> {
    Json(commands::get_relay_status_core(&state.db.conn, state.emitter.clone()).await)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestParams {
    pub config: commands::RelayTestConfig,
}

pub async fn test_relay_connection(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<TestParams>,
) -> Result<Json<commands::RelayTestResult>, RelayError> {
    commands::test_relay_connection_core(&state.db.conn, params.config)
        .await
        .map(Json)
}
