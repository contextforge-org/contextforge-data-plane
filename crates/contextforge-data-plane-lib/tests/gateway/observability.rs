use std::sync::Arc;

use contextforge_data_plane_cpex::CpexRuntimeRegistry;
use contextforge_data_plane_lib::{CORRELATION_ID_HEADER, Result};

use crate::harness::{TEST_USER_ID, connect_modern_client, modern_client_info, start_gateway, sum_request, token};

const CORRELATION_ID: &str = "gateway-request-123";

fn client_with_correlation_id() -> reqwest::Client {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_str(&format!("Bearer {}", token(TEST_USER_ID))).expect("valid authorization header"),
    );
    headers.insert(CORRELATION_ID_HEADER, http::HeaderValue::from_static(CORRELATION_ID));
    reqwest::Client::builder().default_headers(headers).build().expect("client should build")
}

#[tokio::test]
async fn correlation_id_is_propagated_to_the_backend() -> Result<()> {
    let gateway = start_gateway(TEST_USER_ID, false, Arc::new(CpexRuntimeRegistry::default())).await;
    let service =
        connect_modern_client(gateway.gateway_url(), client_with_correlation_id(), modern_client_info()).await;

    service.call_tool(sum_request("sum", 1, 2)).await?;

    let backend_headers = gateway
        .backend_state
        .request_headers
        .lock()
        .expect("backend request headers lock poisoned")
        .last()
        .cloned()
        .expect("backend should receive the tool call");
    assert_eq!(backend_headers[CORRELATION_ID_HEADER], CORRELATION_ID);

    drop(service);
    gateway.shutdown().await
}
