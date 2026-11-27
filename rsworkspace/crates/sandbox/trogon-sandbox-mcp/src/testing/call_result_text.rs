use rmcp::model::CallToolResult;

/// The text blocks of a tool result, joined.
pub(crate) fn call_result_text(result: &CallToolResult) -> Result<String, serde_json::Error> {
    let blocks = serde_json::to_value(&result.content)?;
    let text = blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(|text| text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(text)
}
