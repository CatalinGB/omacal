use std::sync::Arc;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

async fn app() -> axum::Router {
    let pool = omacal_store::connect_memory().await.unwrap();
    let state = Arc::new(omacal_view::AppState::headless(pool));
    omacal_server::api::router(state, std::path::PathBuf::from("ui/dist"))
}

#[tokio::test]
async fn healthz_answers_ok() {
    let res = app()
        .await
        .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn get_settings_answers_the_settings_shape() {
    let res = app()
        .await
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/get_settings")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        value.get("syncIntervalMs").is_some(),
        "the settings payload lost its fields: {value}"
    );
    assert!(
        value.get("listMode").is_some(),
        "the settings payload lost its fields: {value}"
    );
}

#[tokio::test]
async fn get_week_accepts_the_camel_case_args_the_ui_sends() {
    for body in [
        r#"{"weekStartMs":1785715200000,"pad":0}"#,
        r#"{"week_start_ms":1785715200000,"pad":0}"#,
    ] {
        let res = app()
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/get_week")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "body {body} was rejected");
    }
}

#[tokio::test]
async fn an_unimplemented_command_is_refused_cleanly() {
    let res = app()
        .await
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/sign_in")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_IMPLEMENTED);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("not available in the web build"), "got: {text}");
}
