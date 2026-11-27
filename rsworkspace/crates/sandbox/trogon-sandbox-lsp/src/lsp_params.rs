use serde::Serialize;
use serde_json::Value;

use crate::position::Position;

#[derive(Serialize)]
pub(crate) struct TextDocumentIdentifier<'a> {
    pub(crate) uri: &'a str,
}

#[derive(Serialize)]
pub(crate) struct TextDocumentParams<'a> {
    #[serde(rename = "textDocument")]
    pub(crate) text_document: TextDocumentIdentifier<'a>,
}

#[derive(Serialize)]
pub(crate) struct CompletionParams<'a> {
    #[serde(rename = "textDocument")]
    pub(crate) text_document: TextDocumentIdentifier<'a>,
    pub(crate) position: Position,
}

#[derive(Serialize)]
pub(crate) struct TextDocumentItem<'a> {
    pub(crate) uri: &'a str,
    #[serde(rename = "languageId")]
    pub(crate) language_id: &'a str,
    pub(crate) version: i32,
    pub(crate) text: &'a str,
}

#[derive(Serialize)]
pub(crate) struct DidOpenParams<'a> {
    #[serde(rename = "textDocument")]
    pub(crate) text_document: TextDocumentItem<'a>,
}

#[derive(Serialize)]
pub(crate) struct VersionedTextDocumentIdentifier<'a> {
    pub(crate) uri: &'a str,
    pub(crate) version: i32,
}

#[derive(Serialize)]
pub(crate) struct ContentChange<'a> {
    pub(crate) text: &'a str,
}

#[derive(Serialize)]
pub(crate) struct DidChangeParams<'a> {
    #[serde(rename = "textDocument")]
    pub(crate) text_document: VersionedTextDocumentIdentifier<'a>,
    #[serde(rename = "contentChanges")]
    pub(crate) content_changes: [ContentChange<'a>; 1],
}

#[derive(Serialize)]
pub(crate) struct WorkspaceFolder<'a> {
    pub(crate) uri: &'a str,
    pub(crate) name: &'a str,
}

#[derive(Serialize)]
pub(crate) struct InitializeParams<'a> {
    #[serde(rename = "processId")]
    pub(crate) process_id: Option<u32>,
    #[serde(rename = "rootUri")]
    pub(crate) root_uri: &'a str,
    #[serde(rename = "workspaceFolders")]
    pub(crate) workspace_folders: [WorkspaceFolder<'a>; 1],
    pub(crate) capabilities: Value,
    #[serde(rename = "initializationOptions")]
    pub(crate) initialization_options: Option<&'a Value>,
}

#[derive(Serialize)]
pub(crate) struct EmptyParams {}
