use trogon_claude_agent_sdk::{Content, ContentBlock, JsonObject, ToolName, ToolResultStatus, ToolUseId};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn message_content_keeps_text_thinking_tool_calls_and_extensions() -> TestResult {
    let input: JsonObject = serde_json::from_str(r#"{"path":"file","future":null}"#)?;
    let fields: JsonObject = serde_json::from_str(r#"{"payload":[1,true,null]}"#)?;
    let blocks = vec![
        ContentBlock::Text { text: "visible".into() },
        ContentBlock::Thinking {
            thinking: "reason".into(),
            signature: "signature".into(),
        },
        ContentBlock::ToolUse {
            id: ToolUseId::from("tool-one"),
            name: ToolName::from("Read"),
            input: input.clone(),
        },
        ContentBlock::Unknown {
            kind: "future_block".into(),
            data: fields.clone(),
        },
    ];
    assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "visible"));
    assert!(
        matches!(&blocks[1], ContentBlock::Thinking { thinking, signature } if thinking == "reason" && signature == "signature")
    );
    assert!(
        matches!(&blocks[2], ContentBlock::ToolUse { id, name, input: actual } if id.as_str() == "tool-one" && name.as_str() == "Read" && actual == &input)
    );
    assert!(matches!(&blocks[3], ContentBlock::Unknown { kind, data } if kind == "future_block" && data == &fields));
    let content = Content::Blocks(blocks);
    assert!(matches!(content.clone(), Content::Blocks(blocks) if blocks.len() == 4));
    assert!(matches!(Content::Text("plain".into()), Content::Text(text) if text == "plain"));
    Ok(())
}

#[test]
fn tool_results_keep_unspecified_success_and_error_status_separate() -> TestResult {
    for status in [
        ToolResultStatus::NotReported,
        ToolResultStatus::Success,
        ToolResultStatus::Error,
    ] {
        let content = serde_json::from_str(r#"[{"type":"text","text":"result"}]"#)?;
        let block = ContentBlock::ToolResult {
            tool_use_id: ToolUseId::from("tool-one"),
            content,
            status,
        };
        assert!(
            matches!(block.clone(), ContentBlock::ToolResult { tool_use_id, content, status: actual }
            if tool_use_id.as_str() == "tool-one" && content.is_array() && actual == status)
        );
    }
    assert_ne!(ToolResultStatus::NotReported, ToolResultStatus::Success);
    assert_ne!(ToolResultStatus::Success, ToolResultStatus::Error);
    Ok(())
}
