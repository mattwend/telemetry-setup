// SPDX-License-Identifier: MIT

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::reload::{ReloadState, read_lock, write_lock};

#[derive(Debug, Deserialize)]
struct FilterUpdate {
    filter: String,
}

#[derive(Debug, Serialize)]
struct FiltersResponse {
    stdout: String,
    otlp: Option<String>,
}

/// Builds the runtime log-control HTTP router.
///
/// # Arguments
///
/// * `state` - Shared reload state used by all filter endpoints.
///
/// # Returns
///
/// An Axum router with stdout and OTLP filter endpoints registered.
pub(super) fn build_router(state: ReloadState) -> Router {
    Router::new()
        .route("/filters", get(get_filters))
        .route("/filters/stdout", put(update_stdout_filter))
        .route("/filters/otlp", put(update_otlp_filter))
        .with_state(state)
}

/// Returns the current stdout and OTLP filter strings.
async fn get_filters(State(state): State<ReloadState>) -> Json<FiltersResponse> {
    Json(FiltersResponse {
        stdout: read_lock(&state.stdout_filter),
        otlp: read_lock(&state.otlp_filter),
    })
}

/// Replaces the stdout filter with the user-provided `update`.
async fn update_stdout_filter(
    State(state): State<ReloadState>,
    Json(update): Json<FilterUpdate>,
) -> Result<Json<FiltersResponse>, (StatusCode, String)> {
    let _guard = state
        .stdout_update_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    (state.stdout_reload)(update.filter.clone())
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    write_lock(&state.stdout_filter, update.filter);
    Ok(Json(FiltersResponse {
        stdout: read_lock(&state.stdout_filter),
        otlp: read_lock(&state.otlp_filter),
    }))
}

/// Replaces the OTLP filter with the user-provided `update` when OTLP is enabled.
async fn update_otlp_filter(
    State(state): State<ReloadState>,
    Json(update): Json<FilterUpdate>,
) -> Result<Json<FiltersResponse>, (StatusCode, String)> {
    let Some(reload) = &state.otlp_reload else {
        return Err((
            StatusCode::NOT_FOUND,
            "OTLP filtering is not enabled for this process".to_string(),
        ));
    };

    let _guard = state
        .otlp_update_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reload(update.filter.clone()).map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    write_lock(&state.otlp_filter, Some(update.filter));
    Ok(Json(FiltersResponse {
        stdout: read_lock(&state.stdout_filter),
        otlp: read_lock(&state.otlp_filter),
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{ReloadState, build_router};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::util::ServiceExt;

    fn test_state(with_otlp: bool) -> ReloadState {
        ReloadState::new(
            "info".to_string(),
            with_otlp.then(|| "warn".to_string()),
            std::sync::Arc::new(|_| Ok(())),
            with_otlp.then(|| std::sync::Arc::new(|_| Ok(())) as _),
        )
    }

    #[tokio::test]
    async fn get_filters_returns_current_values() {
        let app = build_router(test_state(true));
        let response = app
            .oneshot(Request::get("/filters").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("\"stdout\":\"info\""));
        assert!(body.contains("\"otlp\":\"warn\""));
    }

    #[tokio::test]
    async fn updating_stdout_filter_invokes_reload_callback() {
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let callback_calls = calls.clone();
        let state = ReloadState::new(
            "info".to_string(),
            None,
            std::sync::Arc::new(move |filter| {
                assert_eq!(filter, "debug");
                callback_calls.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }),
            None,
        );
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::put("/filters/stdout")
                    .header("content-type", "application/json")
                    .body(Body::from("{\"filter\":\"debug\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn updating_otlp_filter_without_otlp_returns_not_found() {
        let app = build_router(test_state(false));
        let response = app
            .oneshot(
                Request::put("/filters/otlp")
                    .header("content-type", "application/json")
                    .body(Body::from("{\"filter\":\"debug\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn updating_stdout_filter_maps_reload_errors_to_bad_request() {
        let state = ReloadState::new(
            "info".to_string(),
            None,
            std::sync::Arc::new(|_| Err("invalid filter".to_string())),
            None,
        );
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::put("/filters/stdout")
                    .header("content-type", "application/json")
                    .body(Body::from("{\"filter\":\"debug\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn updating_otlp_filter_invokes_reload_callback_and_updates_state() {
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let callback_calls = calls.clone();
        let observed_filter = std::sync::Arc::new(Mutex::new(None));
        let callback_observed_filter = observed_filter.clone();
        let state = ReloadState::new(
            "info".to_string(),
            Some("warn".to_string()),
            std::sync::Arc::new(|_| Ok(())),
            Some(std::sync::Arc::new(move |filter| {
                callback_calls.fetch_add(1, Ordering::Relaxed);
                *callback_observed_filter.lock().unwrap() = Some(filter);
                Ok(())
            })),
        );
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::put("/filters/otlp")
                    .header("content-type", "application/json")
                    .body(Body::from("{\"filter\":\"debug\"}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(observed_filter.lock().unwrap().as_deref(), Some("debug"));
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("\"otlp\":\"debug\""));
    }

    #[tokio::test]
    async fn malformed_json_returns_bad_request() {
        let app = build_router(test_state(true));
        let response = app
            .oneshot(
                Request::put("/filters/stdout")
                    .header("content-type", "application/json")
                    .body(Body::from("not-json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
