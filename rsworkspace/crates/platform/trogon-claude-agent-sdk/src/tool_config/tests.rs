use super::QuestionPreviewFormat;

#[test]
fn preview_formats_use_the_upstream_spelling() {
    assert_eq!(QuestionPreviewFormat::Markdown.as_str(), "markdown");
    assert_eq!(QuestionPreviewFormat::Html.as_str(), "html");
}
