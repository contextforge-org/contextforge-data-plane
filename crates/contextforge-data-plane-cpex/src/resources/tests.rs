use super::*;

#[test]
fn resource_result_response_applies_text_changes() {
    let original =
        ReadResourceResult::new(vec![ResourceContents::text("AWS_ACCESS_KEY_ID=secret", "file:///password.env")]);
    let mut payload = resource_result_payload(&original, "resource-1").expect("resource result is supported");
    let ContentPart::Resource { content } = &mut payload.message.content[0] else {
        panic!("expected resource content");
    };
    content.content = Some("AWS_ACCESS_KEY_ID=[redacted]".to_owned());

    let result = resource_result_response(original, &payload).expect("text edit applies");

    let ResourceContents::TextResourceContents { text, uri, .. } = &result.contents[0] else {
        panic!("expected text resource");
    };
    assert_eq!("AWS_ACCESS_KEY_ID=[redacted]", text);
    assert_eq!("file:///password.env", uri);
}

#[test]
fn resource_result_response_decodes_and_applies_blob_changes() {
    let wire_blob = BASE64_STANDARD.encode(b"AWS_ACCESS_KEY_ID=secret");
    let original = ReadResourceResult::new(vec![
        ResourceContents::blob(wire_blob.clone(), "file:///password.bin").with_mime_type("application/octet-stream"),
    ]);
    let mut payload = resource_result_payload(&original, "resource-1").expect("valid blob is supported");
    let ContentPart::Resource { content } = &mut payload.message.content[0] else {
        panic!("expected resource content");
    };
    assert_eq!(Some(b"AWS_ACCESS_KEY_ID=secret".as_slice()), content.blob.as_deref());
    content.blob = Some(b"AWS_ACCESS_KEY_ID=[redacted]".to_vec());

    let result = resource_result_response(original, &payload).expect("blob edit applies");

    let ResourceContents::BlobResourceContents { blob, uri, .. } = &result.contents[0] else {
        panic!("expected blob resource");
    };
    assert_eq!(b"AWS_ACCESS_KEY_ID=[redacted]", BASE64_STANDARD.decode(blob).expect("valid base64").as_slice());
    assert_eq!("file:///password.bin", uri);
    assert_ne!(&wire_blob, blob);
}

#[test]
fn resource_result_response_preserves_unchanged_blob_wire_value() {
    let wire_blob = BASE64_STANDARD.encode(b"unchanged");
    let original = ReadResourceResult::new(vec![ResourceContents::blob(&wire_blob, "file:///image.bin")]);
    let payload = resource_result_payload(&original, "resource-1").expect("valid blob is supported");

    let result = resource_result_response(original, &payload).expect("unchanged blob applies");

    let ResourceContents::BlobResourceContents { blob, .. } = &result.contents[0] else {
        panic!("expected blob resource");
    };
    assert_eq!(&wire_blob, blob);
}

#[test]
fn resource_result_payload_rejects_invalid_base64_blob() {
    let original = ReadResourceResult::new(vec![ResourceContents::blob("not base64!", "file:///image.bin")]);

    assert!(resource_result_payload(&original, "resource-1").is_none());
}

#[test]
fn resource_result_allows_mime_uri_and_content_type_changes() {
    let original: ReadResourceResult = serde_json::from_value(serde_json::json!({
        "_meta": {"response": "preserved"},
        "contents": [
            {"uri": "file:///a", "mimeType": "text/plain", "text": "original", "_meta": {"item": 1}},
            {"uri": "file:///b", "mimeType": "application/octet-stream", "blob": "YmluYXJ5", "_meta": {"item": 2}}
        ]
    }))
    .expect("valid resource response");
    let mut payload = resource_result_payload(&original, "resource-1").expect("resource payload");
    for (index, part) in payload.message.content.iter_mut().enumerate() {
        let ContentPart::Resource { content } = part else { panic!("resource content") };
        content.uri = format!("file:///changed-{index}");
        if index == 0 {
            content.content = None;
            content.blob = Some(b"binary edit".to_vec());
            content.mime_type = Some("application/octet-stream".to_owned());
        } else {
            content.blob = None;
            content.content = Some("text edit".to_owned());
            content.mime_type = Some("text/plain".to_owned());
        }
    }
    let actual = serde_json::to_value(resource_result_response(original, &payload).expect("valid changes apply"))
        .expect("response serializes");
    assert_eq!(serde_json::json!({"response": "preserved"}), actual["_meta"]);
    assert_eq!("file:///changed-0", actual["contents"][0]["uri"]);
    assert_eq!("application/octet-stream", actual["contents"][0]["mimeType"]);
    assert_eq!(BASE64_STANDARD.encode(b"binary edit"), actual["contents"][0]["blob"]);
    assert_eq!(1, actual["contents"][0]["_meta"]["item"]);
    assert_eq!("file:///changed-1", actual["contents"][1]["uri"]);
    assert_eq!("text/plain", actual["contents"][1]["mimeType"]);
    assert_eq!("text edit", actual["contents"][1]["text"]);
    assert_eq!(2, actual["contents"][1]["_meta"]["item"]);
}

#[test]
fn resource_result_ignores_cmf_fields_that_are_not_mcp_content() {
    let original = ReadResourceResult::new(vec![ResourceContents::text("original", "file:///a")]);
    let mut payload = resource_result_payload(&original, "resource-1").expect("resource payload");
    payload.message.schema_version = "plugin value".to_owned();
    payload.message.role = Role::User;
    payload.message.channel = Some(cpex::cpex_core::cmf::Channel::Analysis);
    let ContentPart::Resource { content } = &mut payload.message.content[0] else { panic!("resource content") };
    content.resource_request_id = "plugin value".to_owned();
    content.name = Some("display name".to_owned());
    content.description = Some("description".to_owned());
    content.size_bytes = Some(8);
    content.version = Some("v2".to_owned());
    content.annotations.insert("note".to_owned(), serde_json::json!("annotation"));
    content.content = Some("redacted".to_owned());
    let result = resource_result_response(original, &payload).expect("MCP content remains usable");
    let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] else { panic!("text content") };
    assert_eq!("redacted", text);
}

#[test]
fn resource_result_prefers_text_when_both_content_fields_are_present() {
    let original = ReadResourceResult::new(vec![ResourceContents::blob("YmluYXJ5", "file:///a")]);
    let mut payload = resource_result_payload(&original, "resource-1").expect("resource payload");
    let ContentPart::Resource { content } = &mut payload.message.content[0] else { panic!("resource content") };
    content.content = Some("text replacement".to_owned());
    let result =
        resource_result_response(original, &payload).expect("text takes precedence, as in the built-in serializer");
    let ResourceContents::TextResourceContents { text, .. } = &result.contents[0] else { panic!("text resource") };
    assert_eq!("text replacement", text);
}

#[test]
fn resource_result_rejects_content_without_a_valid_mcp_representation() {
    let original = ReadResourceResult::new(vec![ResourceContents::text("original", "file:///a")]);
    let mut payload = resource_result_payload(&original, "resource-1").expect("resource payload");
    let ContentPart::Resource { content } = &mut payload.message.content[0] else { panic!("resource content") };
    content.content = None;
    assert!(resource_result_response(original, &payload).is_none());
}
