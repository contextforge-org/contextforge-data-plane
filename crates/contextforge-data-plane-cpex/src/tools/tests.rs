use super::*;

#[test]
fn tool_result_response_uses_cmf_error_flag_for_nested_mcp_result() {
    let original = CallToolResult::success(vec![ContentBlock::text("original")]);
    let nested = CallToolResult::success(vec![ContentBlock::text("changed")]);
    let mut payload = tool_result_payload("sum", &nested, "call-1");
    let ContentPart::ToolResult { content } = &mut payload.message.content[0] else {
        panic!("expected tool result");
    };
    content.is_error = true;

    let result = tool_result_response(original, &payload);

    assert_eq!(Some(true), result.is_error);
}
