use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("not found")]
    NotFound,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("request timeout")]
    Timeout,

    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("too many requests: {0}")]
    TooManyRequests(String),

    #[error("internal: {0}")]
    Internal(String),

    #[error(transparent)]
    Db(#[from] sqlx::Error),

    #[error(transparent)]
    Jwt(#[from] jsonwebtoken::errors::Error),
}

impl From<coveflow_queue::QueueError> for ApiError {
    fn from(e: coveflow_queue::QueueError) -> Self {
        match e {
            coveflow_queue::QueueError::QuotaExceeded(msg) => ApiError::Conflict(msg),
            coveflow_queue::QueueError::Db(e) => ApiError::Db(e),
            coveflow_queue::QueueError::Other(msg) => ApiError::Internal(msg),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            ApiError::Forbidden(_) => (StatusCode::FORBIDDEN, self.to_string()),
            ApiError::NotFound => (StatusCode::NOT_FOUND, self.to_string()),
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            ApiError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            ApiError::Timeout => (StatusCode::REQUEST_TIMEOUT, self.to_string()),
            ApiError::ServiceUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
            ApiError::TooManyRequests(_) => (StatusCode::TOO_MANY_REQUESTS, self.to_string()),
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            ApiError::Db(e) => {
                tracing::error!(error = %e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
            ApiError::Jwt(_) => (StatusCode::UNAUTHORIZED, "invalid token".to_string()),
        };

        let body = serde_json::json!({ "error": message });
        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn status_of(err: ApiError) -> StatusCode {
        err.into_response().status()
    }

    #[test]
    fn test_timeout_status() {
        assert_eq!(status_of(ApiError::Timeout), StatusCode::REQUEST_TIMEOUT);
    }

    #[test]
    fn test_service_unavailable_status() {
        assert_eq!(
            status_of(ApiError::ServiceUnavailable("queue full".into())),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn test_existing_variants_unchanged() {
        assert_eq!(status_of(ApiError::Unauthorized), StatusCode::UNAUTHORIZED);
        assert_eq!(status_of(ApiError::NotFound), StatusCode::NOT_FOUND);
        assert_eq!(
            status_of(ApiError::BadRequest("x".into())),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_of(ApiError::Conflict("x".into())),
            StatusCode::CONFLICT
        );
        assert_eq!(
            status_of(ApiError::Forbidden("x".into())),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status_of(ApiError::Internal("x".into())),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
