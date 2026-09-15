use contextforge_data_plane_lib::Result;
use rmcp::model::{ArgumentInfo, CompleteRequestParams, Reference};

use crate::harness::{TEST_USER_ID, connect_modern_client, create_client, modern_client_info, start_counter_gateway};

const EXAMPLE_PROMPT: &str = "00000000-0000-0000-0000-000000000001-example_prompt";
const MEMO_RESOURCE: &str = "00000000-0000-0000-0000-000000000001-memo://insights";

#[tokio::test]
async fn plaintext_complete_for_unrouted_reference_errors() -> Result<()> {
    let fixture = start_counter_gateway(TEST_USER_ID).await?;
    let service = connect_modern_client(&fixture.gateway_url, create_client(TEST_USER_ID), modern_client_info()).await;

    service
        .complete_prompt_simple("unrouted_prompt", "message", "h")
        .await
        .expect_err("an unrouted completion reference must fail");
    Ok(())
}

#[tokio::test]
async fn test_complete_for_promtp() -> Result<()> {
    let fixture = start_counter_gateway(TEST_USER_ID).await?;
    let service = connect_modern_client(&fixture.gateway_url, create_client(TEST_USER_ID), modern_client_info()).await;

    let result = service
        .complete(CompleteRequestParams::new(Reference::for_prompt(EXAMPLE_PROMPT), ArgumentInfo::new("message", "h")))
        .await?;
    assert_eq!(result.completion.values, ["hello", "hola"]);
    Ok(())
}

#[tokio::test]
async fn test_complete_for_resource() -> Result<()> {
    let fixture = start_counter_gateway(TEST_USER_ID).await?;
    let service = connect_modern_client(&fixture.gateway_url, create_client(TEST_USER_ID), modern_client_info()).await;

    let result = service
        .complete(CompleteRequestParams::new(Reference::for_resource(MEMO_RESOURCE), ArgumentInfo::new("message", "h")))
        .await?;
    assert_eq!(result.completion.values, ["memo://insights"]);
    Ok(())
}
