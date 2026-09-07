use super::*;

fn text_prompt() -> GetPromptResult {
    GetPromptResult::new(vec![PromptMessage::new_text(McpRole::User, "review of weather")])
}

fn prompt_result_mut(payload: &mut MessagePayload) -> &mut PromptResult {
    payload
        .message
        .content
        .iter_mut()
        .find_map(|part| match part {
            ContentPart::PromptResult { content } => Some(content),
            _ => None,
        })
        .expect("payload carries a prompt result")
}

fn edited_messages(payload: &mut MessagePayload) -> &mut Vec<Message> {
    &mut prompt_result_mut(payload).messages
}

#[test]
fn prompt_result_response_rejects_added_message() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let extra = edited_messages(&mut payload).first().cloned().expect("one message");
    edited_messages(&mut payload).push(extra);

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_extra_prompt_result() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let duplicate = payload.message.content[0].clone();
    payload.message.content.push(duplicate);

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_rejection_reports_the_plugin_error_message() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let result = prompt_result_mut(&mut payload);
    result.is_error = true;
    result.error_message = Some("blocked by policy".to_owned());

    assert_eq!(Some("blocked by policy".to_owned()), prompt_result_rejection(&payload));
}

#[test]
fn prompt_result_rejection_falls_back_when_the_plugin_gives_no_message() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    prompt_result_mut(&mut payload).is_error = true;

    assert_eq!(Some("Plugin rejected the rendered prompt".to_owned()), prompt_result_rejection(&payload));
}

#[test]
fn prompt_result_rejection_is_absent_for_a_normal_result() {
    let original = text_prompt();
    let payload = prompt_result_payload(&original, "review", "prompt-1");

    assert_eq!(None, prompt_result_rejection(&payload));
}

fn review_payload() -> MessagePayload {
    let request = GetPromptRequestParams::new("review")
        .with_arguments(Map::from_iter([("topic".to_owned(), Value::from("weather"))]));
    prompt_request_payload(&request, "review", "backend-a", "prompt-1")
}

fn prompt_request_mut(payload: &mut MessagePayload) -> &mut PromptRequest {
    payload
        .message
        .content
        .iter_mut()
        .find_map(|part| match part {
            ContentPart::PromptRequest { content } => Some(content),
            _ => None,
        })
        .expect("payload carries a prompt request")
}

#[test]
fn prompt_request_arguments_accepts_an_argument_edit() {
    let mut payload = review_payload();
    prompt_request_mut(&mut payload).arguments.insert("topic".to_owned(), Value::from("rain"));

    let arguments = prompt_request_arguments(&payload, "review", "backend-a", "prompt-1");

    assert_eq!(Some(&Value::from("rain")), arguments.as_ref().and_then(|args| args.get("topic")));
}

#[test]
fn prompt_request_arguments_rejects_a_renamed_prompt() {
    let mut payload = review_payload();
    "other".clone_into(&mut prompt_request_mut(&mut payload).name);

    assert!(prompt_request_arguments(&payload, "review", "backend-a", "prompt-1").is_none());
}

#[test]
fn prompt_request_arguments_rejects_a_rerouted_backend() {
    let mut payload = review_payload();
    prompt_request_mut(&mut payload).server_id = Some("backend-b".to_owned());

    assert!(prompt_request_arguments(&payload, "review", "backend-a", "prompt-1").is_none());
}

#[test]
fn prompt_request_arguments_rejects_a_recorrelated_request() {
    let mut payload = review_payload();
    "prompt-2".clone_into(&mut prompt_request_mut(&mut payload).prompt_request_id);

    assert!(prompt_request_arguments(&payload, "review", "backend-a", "prompt-1").is_none());
}

#[test]
fn prompt_request_arguments_rejects_extra_prompt_requests() {
    let mut payload = review_payload();
    let duplicate = payload.message.content[0].clone();
    payload.message.content.push(duplicate);

    assert!(prompt_request_arguments(&payload, "review", "backend-a", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_envelope_content_edit() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    prompt_result_mut(&mut payload).content = Some("[REDACTED]".to_owned());

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_renamed_prompt() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    prompt_result_mut(&mut payload).prompt_name = "other".to_owned();

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_recorrelated_result() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    prompt_result_mut(&mut payload).prompt_request_id = "prompt-2".to_owned();

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_error_message_without_error_flag() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    prompt_result_mut(&mut payload).error_message = Some("blocked".to_owned());

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

fn resource_prompt() -> GetPromptResult {
    GetPromptResult::new(vec![PromptMessage::new(
        McpRole::User,
        ContentBlock::resource(ResourceContents::text("token=secret", "file:///app.env")),
    )])
}

#[test]
fn prompt_result_response_rejects_resource_type_edit() {
    let original = resource_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::Resource { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("expected a resource part");
    };
    content.resource_type = ResourceType::Database;

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_dropped_resource_metadata() {
    let original = resource_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::Resource { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("expected a resource part");
    };
    content.description = Some("annotated by policy".to_owned());

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

fn media_prompt(content: ContentBlock) -> GetPromptResult {
    GetPromptResult::new(vec![PromptMessage::new(McpRole::User, content)])
}

#[test]
fn prompt_result_response_round_trips_an_image_edit() {
    let original = media_prompt(ContentBlock::image("aW1hZ2U=", "image/png"));
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::Image { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("expected an image part");
    };
    content.data = "cmVkYWN0ZWQ=".to_owned();

    let result = prompt_result_response(original, &payload, "review", "prompt-1").expect("image edit applies");

    let ContentBlock::Image(image) = &result.messages[0].content else { panic!("expected an image") };
    assert_eq!("cmVkYWN0ZWQ=", image.data);
    assert_eq!("image/png", image.mime_type);
}

#[test]
fn prompt_result_response_round_trips_an_audio_edit() {
    let original = media_prompt(ContentBlock::audio("YXVkaW8=", "audio/mp3"));
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::Audio { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("audio reaches the plugin as a CMF audio part");
    };
    content.data = "cmVkYWN0ZWQ=".to_owned();

    let result = prompt_result_response(original, &payload, "review", "prompt-1").expect("audio edit applies");

    let ContentBlock::Audio(audio) = &result.messages[0].content else { panic!("expected audio") };
    assert_eq!("cmVkYWN0ZWQ=", audio.data);
    assert_eq!("audio/mp3", audio.mime_type);
}

#[test]
fn prompt_result_response_rejects_url_sourced_audio() {
    let original = media_prompt(ContentBlock::audio("YXVkaW8=", "audio/mp3"));
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::Audio { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("expected an audio part");
    };
    "url".clone_into(&mut content.source_type);
    content.data = "https://example.invalid/clip.mp3".to_owned();

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_audio_without_media_type() {
    let original = media_prompt(ContentBlock::audio("YXVkaW8=", "audio/mp3"));
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::Audio { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("expected an audio part");
    };
    content.media_type = None;

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_round_trips_a_resource_link_edit() {
    let original = media_prompt(ContentBlock::ResourceLink(McpResource::new("file:///app.env", "app-env")));
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::ResourceRef { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("expected a resource reference part");
    };
    content.name = Some("redacted-env".to_owned());

    let result = prompt_result_response(original, &payload, "review", "prompt-1").expect("link edit applies");

    let ContentBlock::ResourceLink(link) = &result.messages[0].content else { panic!("expected a link") };
    assert_eq!("redacted-env", link.name);
    assert_eq!("file:///app.env", link.uri);
}

#[test]
fn prompt_result_response_rejects_resource_with_removed_text() {
    let original = resource_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::Resource { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("expected a resource part");
    };
    content.content = None;

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_multiple_content_parts() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    edited_messages(&mut payload)[0].content.push(ContentPart::Text { text: "extra".to_owned() });

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_a_cmf_only_content_part() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    edited_messages(&mut payload)[0].content = vec![ContentPart::Thinking { text: "reasoning".to_owned() }];

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_a_payload_without_a_prompt_result() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    payload.message.content.clear();

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_request_arguments_rejects_a_payload_without_a_prompt_request() {
    let mut payload = review_payload();
    payload.message.content.clear();

    assert!(prompt_request_arguments(&payload, "review", "backend-a", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_url_sourced_image() {
    let original =
        GetPromptResult::new(vec![PromptMessage::new(McpRole::User, ContentBlock::image("aW1hZ2U=", "image/png"))]);
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::Image { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("expected an image part");
    };
    "url".clone_into(&mut content.source_type);
    content.data = "https://example.invalid/image.png".to_owned();

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_image_without_media_type() {
    let original =
        GetPromptResult::new(vec![PromptMessage::new(McpRole::User, ContentBlock::image("aW1hZ2U=", "image/png"))]);
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::Image { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("expected an image part");
    };
    content.media_type = None;

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_resource_link_without_name() {
    let original = GetPromptResult::new(vec![PromptMessage::new(
        McpRole::User,
        ContentBlock::ResourceLink(McpResource::new("file:///app.env", "app-env")),
    )]);
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::ResourceRef { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("expected a resource reference part");
    };
    content.name = None;

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_resource_link_range_edit() {
    let original = GetPromptResult::new(vec![PromptMessage::new(
        McpRole::User,
        ContentBlock::ResourceLink(McpResource::new("file:///app.env", "app-env")),
    )]);
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    let ContentPart::ResourceRef { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("expected a resource reference part");
    };
    content.range_start = Some(10);

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_removed_message() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    edited_messages(&mut payload).clear();

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_rejects_unmappable_role() {
    let original = text_prompt();
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");
    edited_messages(&mut payload)[0].role = Role::System;

    assert!(prompt_result_response(original, &payload, "review", "prompt-1").is_none());
}

#[test]
fn prompt_result_response_preserves_unmodified_messages() {
    let original = text_prompt();
    let payload = prompt_result_payload(&original, "review", "prompt-1");

    let result =
        prompt_result_response(original.clone(), &payload, "review", "prompt-1").expect("unmodified payload applies");

    assert_eq!(
        serde_json::to_value(&original).expect("original serializes"),
        serde_json::to_value(&result).expect("result serializes")
    );
}

#[test]
fn prompt_result_response_round_trips_embedded_resource() {
    let original = GetPromptResult::new(vec![PromptMessage::new(
        McpRole::User,
        ContentBlock::resource(ResourceContents::text("token=secret", "file:///app.env")),
    )]);
    let mut payload = prompt_result_payload(&original, "review", "prompt-1");

    let ContentPart::Resource { content } = &mut edited_messages(&mut payload)[0].content[0] else {
        panic!("embedded resource reaches the plugin as a CMF resource part");
    };
    assert_eq!(Some("token=secret"), content.content.as_deref());
    content.content = Some("token=[REDACTED]".to_owned());

    let result = prompt_result_response(original, &payload, "review", "prompt-1").expect("resource edit applies");

    let ContentBlock::Resource(resource) = &result.messages[0].content else {
        panic!("expected an embedded resource");
    };
    let ResourceContents::TextResourceContents { text, uri, .. } = &resource.resource else {
        panic!("expected text resource contents");
    };
    assert_eq!("token=[REDACTED]", text);
    assert_eq!("file:///app.env", uri);
}
