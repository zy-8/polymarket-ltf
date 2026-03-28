use std::convert::Infallible;
use std::net::SocketAddr;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::CONTENT_TYPE;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use tokio::net::TcpListener;
use tracing::info;
use tracing::warn;

use crate::dashboard::Handle;

pub type Body = Full<Bytes>;
const INDEX_HTML: &str = include_str!("../../dashboard/index.html");
const APP_JS: &str = include_str!("../../dashboard/app.js");
const STYLES_CSS: &str = include_str!("../../dashboard/styles.css");
pub const DEFAULT_ADDR: &str = "0.0.0.0:3000";

pub async fn spawn(handle: Handle) -> anyhow::Result<()> {
    let addr: SocketAddr = DEFAULT_ADDR.parse()?;
    let listener = TcpListener::bind(addr).await?;

    info!(addr = %addr, "dashboard web server started");

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                warn!("dashboard web server accept failed");
                continue;
            };

            let handle = handle.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let service = service_fn(move |request| route(request, handle.clone()));

                if let Err(error) = http1::Builder::new().serve_connection(io, service).await {
                    warn!(error = %error, "dashboard http connection failed");
                }
            });
        }
    });

    Ok(())
}

pub async fn route(
    request: Request<Incoming>,
    handle: Handle,
) -> Result<Response<Body>, Infallible> {
    let response = match (request.method(), request.uri().path()) {
        (&Method::GET, "/") | (&Method::GET, "/index.html") => {
            asset_response(StatusCode::OK, "text/html; charset=utf-8", INDEX_HTML)
        }
        (&Method::GET, "/app.js") => asset_response(
            StatusCode::OK,
            "application/javascript; charset=utf-8",
            APP_JS,
        ),
        (&Method::GET, "/styles.css") => {
            asset_response(StatusCode::OK, "text/css; charset=utf-8", STYLES_CSS)
        }
        (&Method::GET, "/api/info") => info_response(handle).await,
        (&Method::GET, "/api/positions") => positions_response(&request, handle),
        (&Method::GET, "/api/open-orders") => open_orders_response(&request, handle),
        (&Method::GET, "/api/positions-page") => positions_page_response(&request, handle).await,
        _ => text_response(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            "not found",
        ),
    };

    Ok(response)
}

async fn positions_page_response(request: &Request<Incoming>, handle: Handle) -> Response<Body> {
    let Some(strategy) = query_param(request.uri().query(), "strategy") else {
        return text_response(
            StatusCode::BAD_REQUEST,
            "text/plain; charset=utf-8",
            "missing strategy query param",
        );
    };

    let page = parse_usize_param(request.uri().query(), "page").unwrap_or(1);
    let page_size = parse_usize_param(request.uri().query(), "page_size").unwrap_or(10);
    let range = query_param(request.uri().query(), "range");
    let payload = handle
        .positions_page(strategy, range, page, page_size)
        .await;

    json_response(StatusCode::OK, &payload)
}

async fn info_response(handle: Handle) -> Response<Body> {
    match handle.info().await {
        Ok(payload) => json_response(StatusCode::OK, &payload),
        Err(error) => text_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain; charset=utf-8",
            &format!("dashboard info unavailable: {error}"),
        ),
    }
}

fn positions_response(request: &Request<Incoming>, handle: Handle) -> Response<Body> {
    let Some(strategy) = query_param(request.uri().query(), "strategy") else {
        return text_response(
            StatusCode::BAD_REQUEST,
            "text/plain; charset=utf-8",
            "missing strategy query param",
        );
    };

    json_response(StatusCode::OK, &handle.positions(strategy))
}

fn open_orders_response(request: &Request<Incoming>, handle: Handle) -> Response<Body> {
    let Some(strategy) = query_param(request.uri().query(), "strategy") else {
        return text_response(
            StatusCode::BAD_REQUEST,
            "text/plain; charset=utf-8",
            "missing strategy query param",
        );
    };

    json_response(StatusCode::OK, &handle.open_orders(strategy))
}

fn parse_usize_param<'a>(query: Option<&'a str>, key: &str) -> Option<usize> {
    query_param(query, key)?.parse::<usize>().ok()
}

fn query_param<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    let query = query?;

    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let current = parts.next()?;
        if current == key {
            return Some(parts.next().unwrap_or_default());
        }
    }

    None
}

fn asset_response(status: StatusCode, content_type: &str, body: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, content_type)
        .body(Full::new(Bytes::from_static(body.as_bytes())))
        .expect("asset response should build")
}

fn json_response<T: Serialize>(status: StatusCode, payload: &T) -> Response<Body> {
    let body = match serde_json::to_vec(payload) {
        Ok(body) => body,
        Err(error) => {
            warn!(error = %error, "dashboard json response serialization failed");
            return text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "text/plain; charset=utf-8",
                "json serialization failed",
            );
        }
    };

    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Full::new(Bytes::from(body)))
        .expect("json response should build")
}

fn text_response(status: StatusCode, content_type: &str, body: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, content_type)
        .body(Full::new(Bytes::from(body.to_string())))
        .expect("text response should build")
}
