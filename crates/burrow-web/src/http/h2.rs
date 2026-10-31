use std::time::Duration;

use hyper_util::rt::{TokioExecutor, TokioTimer};

use super::{MAX_HEAD, MAX_HEADERS};

pub const MAX_STREAMS: u32 = 100;
pub const PENDING_ACCEPT_RESETS: usize = 20;
pub const LOCAL_ERROR_RESETS: usize = 256;
pub const CONCURRENT_RESETS: usize = 50;
pub const FRAME_SIZE: u32 = 16 * 1024;
pub const STREAM_WINDOW: u32 = 256 * 1024;
pub const CONNECTION_WINDOW: u32 = 1024 * 1024;
pub const SEND_BUF: usize = 32 * 1024;
pub const HEADER_TABLE: u32 = 4096;
pub const PING_EVERY: Duration = Duration::from_secs(30);
pub const PING_ANSWER: Duration = Duration::from_secs(20);
pub const APP_READ_BUF: usize = 32 * 1024;
pub const VISITOR_BUF: usize = 64 * 1024;

pub(crate) fn server() -> hyper::server::conn::http2::Builder<TokioExecutor> {
    let mut b = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
    b.timer(TokioTimer::new())
        .max_concurrent_streams(MAX_STREAMS)
        .max_pending_accept_reset_streams(PENDING_ACCEPT_RESETS)
        .max_local_error_reset_streams(LOCAL_ERROR_RESETS)
        .max_header_list_size(u32::try_from(MAX_HEAD).unwrap_or(u32::MAX))
        .header_table_size(HEADER_TABLE)
        .max_frame_size(FRAME_SIZE)
        .initial_stream_window_size(STREAM_WINDOW)
        .initial_connection_window_size(CONNECTION_WINDOW)
        .adaptive_window(false)
        .max_send_buf_size(SEND_BUF)
        .keep_alive_interval(PING_EVERY)
        .keep_alive_timeout(PING_ANSWER)
        .auto_date_header(true);
    b
}

pub(crate) fn client() -> hyper::client::conn::http2::Builder<TokioExecutor> {
    let mut b = hyper::client::conn::http2::Builder::new(TokioExecutor::new());
    b.timer(TokioTimer::new())
        .max_concurrent_streams(MAX_STREAMS)
        .max_pending_accept_reset_streams(PENDING_ACCEPT_RESETS)
        .max_local_error_reset_streams(LOCAL_ERROR_RESETS)
        .max_concurrent_reset_streams(CONCURRENT_RESETS)
        .max_header_list_size(u32::try_from(MAX_HEAD).unwrap_or(u32::MAX))
        .header_table_size(HEADER_TABLE)
        .max_frame_size(FRAME_SIZE)
        .initial_stream_window_size(STREAM_WINDOW)
        .initial_connection_window_size(CONNECTION_WINDOW)
        .adaptive_window(false)
        .max_send_buf_size(SEND_BUF)
        .keep_alive_interval(None)
        .keep_alive_while_idle(false);
    b
}

pub(crate) fn visitor_http1() -> hyper::server::conn::http1::Builder {
    let mut b = hyper::server::conn::http1::Builder::new();
    b.timer(TokioTimer::new())
        .max_buf_size(VISITOR_BUF)
        .max_header_size(MAX_HEAD)
        .max_headers(MAX_HEADERS)
        .keep_alive(true)
        .half_close(false);
    b
}

pub(crate) fn app_http1() -> hyper::client::conn::http1::Builder {
    let mut b = hyper::client::conn::http1::Builder::new();
    b.max_buf_size(APP_READ_BUF)
        .max_header_size(MAX_HEAD)
        .max_headers(MAX_HEADERS);
    b
}

#[cfg(test)]
mod tests;
