use std::time::Instant;

use anyhow::Context;
use axum::{
    Extension, Json, Router,
    extract::Request,
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::get,
};
use serde::Serialize;
use tracing::{error, info, warn};

async fn hello_world() -> &'static str {
    info!("responding with hello world");
    "Hello, world!"
}

#[derive(Serialize)]
struct StatusServerResponse {
    hostname: String,
}

async fn status_server(headers: HeaderMap) -> Json<StatusServerResponse> {
    let hostname = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<unknown>")
        .to_string();

    info!(%hostname, "status endpoint resolved hostname");

    Json(StatusServerResponse { hostname })
}

#[derive(Debug, Clone, Serialize)]
struct User {
    id: String,
    email: String,
}

async fn auth_inject_user(mut req: Request, next: Next) -> Result<Response, StatusCode> {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();

    let auth = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if let Some(token) = auth.strip_prefix("Bearer ") {
        if token == "secret-token" {
            let user = User {
                id: "u_123".into(),
                email: "matheus@example.com".into(),
            };
            info!(%method, %path, user_id = %user.id, "authenticated request");
            req.extensions_mut().insert(user);
            let res = next.run(req).await;
            return Ok(res);
        }
        warn!(%method, %path, "invalid bearer token");
    } else {
        warn!(%method, %path, "authorization header missing or malformed");
    }

    Err(StatusCode::UNAUTHORIZED)
}

async fn me(Extension(user): Extension<User>) -> Json<User> {
    info!(user_id = %user.id, "serving authenticated user info");
    Json(user)
}

async fn log_requests(req: Request, next: Next) -> Result<Response, StatusCode> {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let user_agent = req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_else(|| "-".into());
    let start = Instant::now();

    info!(%method, %path, %user_agent, "received request");

    let response = next.run(req).await;
    let status = response.status();
    let elapsed = start.elapsed();

    info!(%method, %path, %status, elapsed_ms = %elapsed.as_millis(), "completed request");

    Ok(response)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "simple_http_server=info".into()),
        )
        .with_target(false)
        .compact()
        .init();

    let app = Router::new()
        .route("/", get(hello_world))
        .route("/status", get(status_server))
        .route("/me", get(me).layer(middleware::from_fn(auth_inject_user)))
        .layer(middleware::from_fn(log_requests));

    let addr = "0.0.0.0:3000";
    info!(%addr, "binding http server");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind to {addr}"))?;

    let listen_addr = format!("http://{addr}");
    info!(%listen_addr, "listening");

    match axum::serve(listener, app.into_make_service()).await {
        Ok(()) => info!("server shutdown gracefully"),
        Err(err) => {
            error!(error = %err, "server terminated with error");
            return Err(err.into());
        }
    }

    Ok(())
}
