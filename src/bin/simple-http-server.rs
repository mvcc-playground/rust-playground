use axum::{
    Json,
    Router,
    // extract::Request,
    http::{HeaderMap, header},
    routing::get,
};
use serde::Serialize;

async fn hello_world() -> &'static str {
    // let res = req
    //     .headers()
    //     .get(header::HOST)
    //     .and_then(|val| val.to_str().ok())
    //     .unwrap_or("unknown")
    //     .to_string();
    "Hello, world!"
}

#[derive(Serialize)]
struct StatusServerResponse {
    hostname: String,
}

async fn status_server(headers: HeaderMap) -> Json<StatusServerResponse> {
    // Extrai o valor do header HOST
    let hostname = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("<unknown>")
        .to_string();

    // Opcional: log
    println!("hostname: {:?}", hostname);

    // Retorna JSON
    Json(StatusServerResponse { hostname })
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(hello_world))
        .route("/status", get(status_server));

    let addr = "0.0.0.0:3000";
    println!("listening on http://{addr}");
    axum::serve(
        tokio::net::TcpListener::bind(addr).await.unwrap(),
        app.into_make_service(),
    )
    .await
    .unwrap();
}
