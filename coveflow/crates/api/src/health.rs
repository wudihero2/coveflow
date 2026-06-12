use axum::Json;

#[derive(serde::Serialize)]
pub(crate) struct HealthResponse {
    status: &'static str,
}

pub(crate) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}
