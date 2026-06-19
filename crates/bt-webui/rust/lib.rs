//! Binder Trace WebUI 内嵌资源服务器。
//!
//! # 职责
//! - 通过 `build.rs` 把 Vite `dist/` 输出嵌入 Rust 二进制。
//! - 为 CLI `webui` 子命令提供本地 HTTP 静态资源服务。
//!
//! # 不变量
//! - `/` 和未知的无扩展名路径返回 `index.html`，以支持 SPA 路由。
//! - 请求只在生成的资源表中解析，URL 路径不能逃逸到 `dist/` 之外。

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::Path;

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Query, State},
    http::{
        Method, StatusCode, Uri,
        header::{ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
        response::Builder as ResponseBuilder,
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
pub use events::WebuiEventsConfig;
use events::{EventsQuery, EventsResponse, WebuiEventHub};
use thiserror::Error;
use tokio::{net::TcpListener, runtime};

mod events;

#[derive(Clone, Copy, Debug)]
struct EmbeddedAsset {
    path: &'static str,
    content_type: &'static str,
    bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/webui_assets.rs"));

/// WebUI 静态资源服务器配置。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebuiServerConfig {
    /// 监听地址。默认绑定 `127.0.0.1`，避免调试页面意外暴露到局域网。
    pub listen: SocketAddr,
    /// 生产事件源配置。
    pub events: WebuiEventsConfig,
}

impl Default for WebuiServerConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 5173)),
            events: WebuiEventsConfig::default(),
        }
    }
}

/// WebUI 静态资源服务器错误。
#[derive(Debug, Error)]
pub enum WebuiError {
    /// 监听 socket 或处理连接时发生 IO 错误。
    #[error("webui io error: {0}")]
    Io(#[from] io::Error),
    /// 构建脚本没有嵌入 `index.html`。
    #[error("embedded WebUI index.html is missing")]
    MissingIndex,
}

/// 返回编译期 WebUI package 目录的绝对路径。
///
/// # Returns
/// 返回包含 `package.json`、`vite.config.ts` 和 React 源码树的路径。
pub fn webui_source_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// 返回 `build.rs` 嵌入的资源数量。
///
/// # Returns
/// 返回编译期在 Vite `dist/` 下发现的文件数量。
pub fn embedded_asset_count() -> usize {
    EMBEDDED_ASSETS.len()
}

/// 启动异步 HTTP server，用于提供内嵌 WebUI。
///
/// # Arguments
/// - `config`: 监听地址和后续可扩展的 server 配置。
///
/// # Returns
/// 函数会一直阻塞到 listener 失败；正常服务时不会返回。
///
/// # Errors
/// 绑定或接受连接失败时返回 `WebuiError::Io`；资源表缺少 `/index.html` 时返回
/// `WebuiError::MissingIndex`。
pub async fn serve(config: WebuiServerConfig) -> Result<(), WebuiError> {
    let listener = TcpListener::bind(config.listen).await?;
    serve_listener(listener, config).await
}

/// 启动一个阻塞式 HTTP server，用于提供内嵌 WebUI。
///
/// # Arguments
/// - `config`: 监听地址和后续可扩展的 server 配置。
///
/// # Returns
/// 函数会一直阻塞到 listener 失败；正常服务时不会返回。
///
/// # Errors
/// 绑定或接受连接失败时返回 `WebuiError::Io`；资源表缺少 `/index.html` 时返回
/// `WebuiError::MissingIndex`。
pub fn serve_blocking(config: WebuiServerConfig) -> Result<(), WebuiError> {
    let runtime = runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bt-webui")
        .build()?;
    runtime.block_on(serve(config))
}

async fn serve_listener(
    listener: TcpListener,
    config: WebuiServerConfig,
) -> Result<(), WebuiError> {
    ensure_index_asset()?;
    axum::serve(listener, webui_router(config.events)).await?;
    Ok(())
}

fn ensure_index_asset() -> Result<(), WebuiError> {
    if asset_for_path("/index.html").is_some() {
        Ok(())
    } else {
        Err(WebuiError::MissingIndex)
    }
}

fn webui_router(events_config: WebuiEventsConfig) -> Router {
    let events = WebuiEventHub::new(events_config);
    Router::new()
        .route("/api/events", get(events_handler))
        .route("/api/events/clear", post(clear_events_handler))
        .fallback(asset_handler)
        .with_state(events)
}

async fn events_handler(
    State(events): State<WebuiEventHub>,
    Query(query): Query<EventsQuery>,
) -> Json<EventsResponse> {
    Json(events.snapshot(&query))
}

async fn clear_events_handler(State(events): State<WebuiEventHub>) -> StatusCode {
    events.clear();
    StatusCode::NO_CONTENT
}

async fn asset_handler(method: Method, uri: Uri) -> Response {
    match method {
        Method::GET | Method::HEAD => serve_asset(method, uri).await,
        _ => method_not_allowed_response(),
    }
}

async fn serve_asset(method: Method, uri: Uri) -> Response {
    let path = uri.path();
    let asset = asset_for_request_path(path).or_else(|| {
        (!path.contains('.'))
            .then(|| asset_for_path("/index.html"))
            .flatten()
    });

    asset.map_or_else(not_found_response, |asset| {
        asset_response(asset, method == Method::HEAD)
    })
}

fn asset_for_request_path(path: &str) -> Option<&'static EmbeddedAsset> {
    let normalized = normalize_path(path);
    asset_for_path(&normalized)
}

fn asset_for_path(path: &str) -> Option<&'static EmbeddedAsset> {
    let path = if path == "/" { "/index.html" } else { path };
    EMBEDDED_ASSETS.iter().find(|asset| asset.path == path)
}

fn normalize_path(path: &str) -> String {
    let without_query = path.split_once('?').map_or(path, |(path, _)| path);
    let without_fragment = without_query
        .split_once('#')
        .map_or(without_query, |(path, _)| path);
    if without_fragment.starts_with('/') {
        without_fragment.to_owned()
    } else {
        format!("/{without_fragment}")
    }
}

fn asset_response(asset: &EmbeddedAsset, head_only: bool) -> Response {
    let cache_control = if asset.path == "/index.html" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };

    let body = if head_only {
        Body::empty()
    } else {
        Body::from(Bytes::from_static(asset.bytes))
    };

    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, asset.content_type)
        .header(CONTENT_LENGTH, asset.bytes.len().to_string())
        .header(CACHE_CONTROL, cache_control);
    build_response(builder, body)
}

fn method_not_allowed_response() -> Response {
    let body = "method not allowed";
    let builder = Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header(ALLOW, "GET, HEAD")
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CONTENT_LENGTH, body.len().to_string());
    build_response(builder, Body::from(body))
}

fn not_found_response() -> Response {
    let body = "not found";
    let builder = Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CONTENT_LENGTH, body.len().to_string());
    build_response(builder, Body::from(body))
}

fn build_response(builder: ResponseBuilder, body: Body) -> Response {
    builder.body(body).unwrap_or_else(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to build HTTP response",
        )
            .into_response()
    })
}

impl fmt::Display for WebuiServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "http://{}", self.listen)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::http::{
        Method, StatusCode, Uri,
        header::{CACHE_CONTROL, CONTENT_TYPE},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        time::timeout,
    };

    use super::{
        WebuiServerConfig, asset_for_request_path, asset_handler, embedded_asset_count,
        serve_listener,
    };

    #[test]
    fn embeds_dist_assets() {
        assert!(embedded_asset_count() >= 3);
        assert!(asset_for_request_path("/").is_some());
        assert!(asset_for_request_path("/index.html").is_some());
    }

    #[tokio::test]
    async fn extensionless_routes_fall_back_to_index() {
        let response = asset_handler(
            Method::GET,
            "/trace/session-1"
                .parse::<Uri>()
                .expect("test route should parse"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(CACHE_CONTROL)
                .expect("index response should set cache policy")
                .to_str()
                .expect("cache policy should be ASCII"),
            "no-cache"
        );
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .expect("index response should set content type")
                .to_str()
                .expect("content type should be ASCII"),
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_connection_does_not_block_other_requests() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let address = listener
            .local_addr()
            .expect("test server should have address");
        let server = tokio::spawn(serve_listener(listener, WebuiServerConfig::default()));
        let slow_connection = TcpStream::connect(address)
            .await
            .expect("slow client should connect");

        let mut fast_connection = TcpStream::connect(address)
            .await
            .expect("fast client should connect");
        fast_connection
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("fast client should send request");

        let mut response = Vec::new();
        timeout(
            Duration::from_secs(2),
            fast_connection.read_to_end(&mut response),
        )
        .await
        .expect("fast request should not wait for slow client")
        .expect("fast response should be readable");

        assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 200 OK"));

        drop(slow_connection);
        server.abort();
        let join_error = server
            .await
            .expect_err("aborted test server should return join error");
        assert!(join_error.is_cancelled());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn head_request_returns_headers_without_body() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let address = listener
            .local_addr()
            .expect("test server should have address");
        let server = tokio::spawn(serve_listener(listener, WebuiServerConfig::default()));

        let mut connection = TcpStream::connect(address)
            .await
            .expect("client should connect");
        connection
            .write_all(b"HEAD / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("client should send request");

        let mut response = Vec::new();
        timeout(
            Duration::from_secs(2),
            connection.read_to_end(&mut response),
        )
        .await
        .expect("HEAD request should finish")
        .expect("HEAD response should be readable");

        let response = String::from_utf8_lossy(&response);
        let (headers, body) = response
            .split_once("\r\n\r\n")
            .expect("HTTP response should contain header terminator");
        assert!(headers.starts_with("HTTP/1.1 200 OK"));
        assert!(headers.contains("content-length: "));
        assert!(body.is_empty());

        server.abort();
        let join_error = server
            .await
            .expect_err("aborted test server should return join error");
        assert!(join_error.is_cancelled());
    }
}
