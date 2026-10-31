use super::*;

fn carries(builder: &str, settings: &[&str]) {
    for want in settings {
        assert!(
            builder.contains(want),
            "the builder lost `{want}`; read D51 in docs/decisions.md before changing an HTTP/2 limit:\n{builder}"
        );
    }
}

#[test]
fn the_decided_limits_keep_their_values() {
    assert_eq!(MAX_STREAMS, 100);
    assert_eq!(PENDING_ACCEPT_RESETS, 20);
    assert_eq!(LOCAL_ERROR_RESETS, 256);
    assert_eq!(CONCURRENT_RESETS, 50);
    assert_eq!(FRAME_SIZE, 16_384);
    assert_eq!(STREAM_WINDOW, 262_144);
    assert_eq!(CONNECTION_WINDOW, 1_048_576);
    assert_eq!(SEND_BUF, 32_768);
    assert_eq!(HEADER_TABLE, 4096);
    assert_eq!(PING_EVERY, Duration::from_secs(30));
    assert_eq!(PING_ANSWER, Duration::from_secs(20));
    assert_eq!(APP_READ_BUF, 32_768);
    assert_eq!(VISITOR_BUF, 65_536);
    assert_eq!(MAX_HEAD, 32_768);
    assert_eq!(MAX_HEADERS, 100);
}

#[test]
fn the_visitor_server_builder_carries_every_limit() {
    carries(
        &format!("{:?}", server()),
        &[
            "max_concurrent_streams: Some(100)",
            "max_pending_accept_reset_streams: Some(20)",
            "max_local_error_reset_streams: Some(256)",
            "max_header_list_size: 32768",
            "header_table_size: Some(4096)",
            "max_frame_size: 16384",
            "initial_stream_window_size: 262144",
            "initial_conn_window_size: 1048576",
            "adaptive_window: false",
            "max_send_buffer_size: 32768",
            "keep_alive_interval: Some(30s)",
            "keep_alive_timeout: 20s",
            "enable_connect_protocol: false",
        ],
    );
}

#[test]
fn the_visitor_http1_builder_bounds_its_buffers() {
    carries(
        &format!("{:?}", visitor_http1()),
        &[
            "h1_max_headers: Some(100)",
            "h1_max_header_size: Some(32768)",
            "max_buf_size: Some(65536)",
            "h1_keep_alive: true",
            "h1_half_close: false",
        ],
    );
}

#[test]
fn the_app_client_builders_carry_every_limit() {
    carries(
        &format!("{:?}", client()),
        &[
            "max_concurrent_streams: Some(100)",
            "max_pending_accept_reset_streams: Some(20)",
            "max_local_error_reset_streams: Some(256)",
            "max_concurrent_reset_streams: Some(50)",
            "max_header_list_size: 32768",
            "header_table_size: Some(4096)",
            "max_frame_size: Some(16384)",
            "initial_stream_window_size: 262144",
            "initial_conn_window_size: 1048576",
            "adaptive_window: false",
            "max_send_buffer_size: 32768",
            "keep_alive_interval: None",
            "keep_alive_while_idle: false",
        ],
    );
    carries(
        &format!("{:?}", app_http1()),
        &[
            "h1_max_headers: Some(100)",
            "h1_max_header_size: Some(32768)",
            "h1_max_buf_size: Some(32768)",
        ],
    );
}
