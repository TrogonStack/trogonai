//! What a tool call leaves behind after the model has seen its result.

use serde::{Deserialize, Serialize};

use crate::reads::ReadCoverage;
use crate::result::{
    CommandOutputReplay, CommandProcessOutcome, CommittedFilePresentation, TerminalActionPresentation,
    ToolResultArtifacts,
};
use crate::tool_image::ToolImage;
use crate::tool_output::{ToolOutputFragment, ToolResultObservations};

/// Where the full result lives.
///
/// Out-of-line storage is one decision, not three: a handle with no preview
/// leaves a caller with something it can fetch and nothing it can show, and a
/// preview with no handle leaves it with something to show and no way to get the
/// rest. They are recorded together so neither can exist alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredOutput {
    /// The whole result is in the transcript.
    Inline,
    /// The result is held elsewhere, under a handle, with a preview of it.
    OutOfLine {
        /// What the store was given to retrieve the result by.
        handle: String,
        /// The excerpt shown in place of the result.
        preview: String,
    },
}

impl StoredOutput {
    /// The handle, when the result is held elsewhere.
    #[must_use]
    pub fn handle(&self) -> Option<&str> {
        match self {
            Self::Inline => None,
            Self::OutOfLine { handle, .. } => Some(handle),
        }
    }

    /// The preview, when the result is held elsewhere.
    #[must_use]
    pub fn preview(&self) -> Option<&str> {
        match self {
            Self::Inline => None,
            Self::OutOfLine { preview, .. } => Some(preview),
        }
    }
}

/// What is remembered about one tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResultMemory {
    /// Host review feedback is retained for the agent, not security evidence.
    review_feedback: bool,
    output: StoredOutput,
    output_bytes: usize,
    stored_output_bytes: usize,
    truncated: bool,
    coverage: Option<ReadCoverage>,
    images: Vec<ToolImage>,
    #[serde(default, skip_serializing_if = "is_false")]
    images_unavailable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image_unavailable_cause: Option<ImageUnavailableCause>,
    #[serde(default)]
    output_fragments: Vec<ToolOutputFragment>,
    #[serde(default)]
    observations: Option<Box<ToolResultObservations>>,
    image_handle: Option<String>,
    artifacts: Option<Box<ToolResultArtifacts>>,
}

const fn is_false(value: &bool) -> bool {
    !*value
}

/// Why a saved image artifact could not be shown to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageUnavailableCause {
    /// Nothing is filed under the handle.
    Missing,
    /// Bytes are filed, and they are not the artifact that was written.
    Corrupt,
}

/// The sentence a provider request uses when saved images cannot be shown.
#[must_use]
pub const fn provider_image_notice(cause: Option<ImageUnavailableCause>) -> &'static str {
    match cause {
        Some(ImageUnavailableCause::Missing) => {
            "[Saved tool images are missing; their artifact is not in this session.]\n"
        }
        Some(ImageUnavailableCause::Corrupt) => {
            "[Saved tool images are corrupt; their artifact does not match what was stored.]\n"
        }
        None => "[Saved tool images are unavailable because their artifact could not be restored.]\n",
    }
}

impl ToolResultMemory {
    /// A result kept whole in the transcript.
    ///
    /// `truncated` is passed in rather than inferred from the two sizes, because
    /// they differ for a reason that is not truncation: masking a credential
    /// makes a result shorter without withholding anything from the model. See
    #[must_use]
    pub const fn inline(output_bytes: usize, stored_output_bytes: usize, truncated: bool) -> Self {
        Self {
            review_feedback: false,
            output: StoredOutput::Inline,
            output_bytes,
            stored_output_bytes,
            truncated,
            coverage: None,
            images: Vec::new(),
            images_unavailable: false,
            image_unavailable_cause: None,
            output_fragments: Vec::new(),
            observations: None,
            image_handle: None,
            artifacts: None,
        }
    }

    /// The same memory, marked as host review feedback.
    #[must_use]
    pub const fn with_review_feedback(mut self, review_feedback: bool) -> Self {
        self.review_feedback = review_feedback;
        self
    }

    /// Whether this result is host review feedback rather than security evidence.
    #[must_use]
    pub const fn review_feedback(&self) -> bool {
        self.review_feedback
    }

    /// The same memory, recording how much of a file the model was shown.
    #[must_use]
    pub fn covering(mut self, coverage: ReadCoverage) -> Self {
        self.coverage = Some(coverage);
        self
    }

    /// The same memory, recording the file change the call committed.
    #[must_use]
    pub fn committing(mut self, committed_file: CommittedFilePresentation) -> Self {
        self.artifacts_mut().committed_file = Some(committed_file);
        self
    }

    /// The same memory, recording whether the command output can be replayed.
    #[must_use]
    pub fn replayable(mut self, replay: CommandOutputReplay) -> Self {
        self.artifacts_mut().command_output_replay = Some(replay);
        self
    }

    /// The same memory, recording how the command's process ended.
    #[must_use]
    pub fn ending(mut self, outcome: CommandProcessOutcome) -> Self {
        self.artifacts_mut().command_process = Some(outcome);
        self
    }

    /// The same memory, recording how the terminal action ended.
    #[must_use]
    pub fn terminating(mut self, presentation: TerminalActionPresentation) -> Self {
        self.artifacts_mut().terminal_action = Some(presentation);
        self
    }

    fn artifacts_mut(&mut self) -> &mut ToolResultArtifacts {
        self.artifacts.get_or_insert_with(Box::default)
    }

    /// The same memory, holding the images the call produced.
    #[must_use]
    pub fn carrying(mut self, images: Vec<ToolImage>) -> Self {
        self.images = images;
        self.images_unavailable = false;
        self.image_unavailable_cause = None;
        self
    }

    /// The same memory, recording that its saved image artifact could not be restored.
    #[must_use]
    pub fn marking_images_unavailable(mut self) -> Self {
        self.images.clear();
        self.images_unavailable = true;
        self
    }

    /// The same memory, recording why the saved image artifact could not be restored.
    #[must_use]
    pub fn marking_images_unavailable_because(mut self, cause: Option<ImageUnavailableCause>) -> Self {
        self.images.clear();
        self.images_unavailable = true;
        self.image_unavailable_cause = cause;
        self
    }

    /// Whether the saved image artifact was missing, corrupt, or unreadable.
    #[must_use]
    pub const fn images_unavailable(&self) -> bool {
        self.images_unavailable
    }

    /// Why the saved image artifact could not be restored, when that is known.
    #[must_use]
    pub const fn image_unavailable_cause(&self) -> Option<ImageUnavailableCause> {
        self.image_unavailable_cause
    }

    /// The same memory, retaining typed non-text result boundaries.
    #[must_use]
    pub fn carrying_output_fragments(mut self, fragments: Vec<ToolOutputFragment>) -> Self {
        self.output_fragments = fragments;
        self
    }

    /// The same memory, retaining typed operational result facts.
    #[must_use]
    pub fn observing(mut self, observations: ToolResultObservations) -> Self {
        self.observations = Some(Box::new(observations));
        self
    }

    /// The same memory, naming the store the images were saved to.
    ///
    /// The handle and the images are not alternatives. A saved image is still in
    /// hand for the rest of the turn, and only the next session write drops the
    /// copy in favor of the handle, so a memory can hold both.
    #[must_use]
    pub fn saving_images(mut self, handle: impl Into<String>) -> Self {
        self.image_handle = Some(handle.into());
        self
    }

    /// The same memory, with the result held outside the transcript.
    #[must_use]
    pub fn stored(mut self, handle: impl Into<String>, preview: impl Into<String>) -> Self {
        self.output = StoredOutput::OutOfLine {
            handle: handle.into(),
            preview: preview.into(),
        };
        self
    }

    /// Where the full result lives.
    #[must_use]
    pub const fn output(&self) -> &StoredOutput {
        &self.output
    }

    /// Size of the result before anything was done to it.
    #[must_use]
    pub const fn output_bytes(&self) -> usize {
        self.output_bytes
    }

    /// Size of what was kept.
    #[must_use]
    pub const fn stored_output_bytes(&self) -> usize {
        self.stored_output_bytes
    }

    /// Whether the cap withheld part of the result.
    #[must_use]
    pub const fn was_truncated(&self) -> bool {
        self.truncated
    }

    /// The same memory, recording that some result content is unavailable.
    #[must_use]
    pub const fn marking_truncated(mut self) -> Self {
        self.truncated = true;
        self
    }

    /// How much of a file the model saw, for the calls that read one.
    #[must_use]
    pub const fn coverage(&self) -> Option<ReadCoverage> {
        self.coverage
    }

    /// The images the call produced, as far as this memory still holds them.
    #[must_use]
    pub fn images(&self) -> &[ToolImage] {
        &self.images
    }

    /// Structured values and resource links retained from the tool result.
    #[must_use]
    pub fn output_fragments(&self) -> &[ToolOutputFragment] {
        &self.output_fragments
    }

    /// Operational facts retained from the tool result.
    #[must_use]
    pub fn observations(&self) -> Option<&ToolResultObservations> {
        self.observations.as_deref()
    }

    /// The store the images were saved to, when they were saved.
    #[must_use]
    pub fn image_handle(&self) -> Option<&str> {
        self.image_handle.as_deref()
    }

    /// Whether the call produced images at all.
    ///
    /// A memory whose images were saved and then dropped still answers yes: the
    /// handle is what says they existed once the copy is gone.
    #[must_use]
    pub fn has_images(&self) -> bool {
        !self.images_unavailable && (!self.images.is_empty() || self.image_handle.is_some())
    }

    /// What the call left outside the transcript, when it left anything.
    #[must_use]
    pub fn artifacts(&self) -> Option<&ToolResultArtifacts> {
        self.artifacts.as_deref()
    }

    /// The file change the call committed, for the calls that wrote one.
    #[must_use]
    pub fn committed_file(&self) -> Option<&CommittedFilePresentation> {
        self.artifacts()?.committed_file.as_ref()
    }

    /// Whether a command's output survives to be replayed.
    #[must_use]
    pub fn command_output_replay(&self) -> Option<&CommandOutputReplay> {
        self.artifacts()?.command_output_replay.as_ref()
    }

    /// How a command's process ended, for the calls that ran one.
    #[must_use]
    pub fn command_process(&self) -> Option<CommandProcessOutcome> {
        self.artifacts()?.command_process
    }

    /// How a terminal action ended, for the calls that made one.
    #[must_use]
    pub fn terminal_action(&self) -> Option<TerminalActionPresentation> {
        self.artifacts()?.terminal_action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_that_named_no_file_does_not_claim_a_partial_view_of_one() {
        let memory = ToolResultMemory::inline(120, 120, false);
        assert_eq!(memory.coverage(), None);
        assert!(!memory.was_truncated());
    }

    #[test]
    fn a_masked_result_is_shorter_without_having_been_truncated() {
        let memory = ToolResultMemory::inline(40, 22, false);
        assert!(
            !memory.was_truncated(),
            "the cap is what truncates; masking only rewrites"
        );
        assert_eq!(memory.output_bytes(), 40);
        assert_eq!(memory.stored_output_bytes(), 22);
    }

    #[test]
    fn an_out_of_line_result_has_both_a_handle_and_a_preview_or_neither() {
        let inline = ToolResultMemory::inline(9, 9, false);
        assert_eq!(inline.output().handle(), None);
        assert_eq!(inline.output().preview(), None);

        let stored = inline.stored("blob-1", "first line");
        assert_eq!(stored.output().handle(), Some("blob-1"));
        assert_eq!(stored.output().preview(), Some("first line"));
    }

    #[test]
    fn images_saved_to_a_store_are_still_remembered_once_the_copy_is_dropped() {
        let carried = ToolResultMemory::inline(12, 12, false).carrying(vec![ToolImage {
            data: "aGk=".to_owned(),
            mime_type: "image/png".to_owned(),
        }]);
        assert!(carried.has_images());
        assert_eq!(carried.image_handle(), None);

        let saved = carried.saving_images("img-1").carrying(Vec::new());
        assert!(saved.images().is_empty());
        assert!(
            saved.has_images(),
            "a gateway asking whether to fetch them would otherwise skip the message"
        );
        assert_eq!(saved.image_handle(), Some("img-1"));
    }

    #[test]
    fn an_unreadable_saved_image_is_not_claimed_as_provider_visible() {
        let memory = ToolResultMemory::inline(8, 8, false)
            .saving_images("img-1")
            .marking_images_unavailable();

        assert!(memory.images_unavailable());
        assert!(!memory.has_images());
        assert_eq!(memory.image_handle(), Some("img-1"));
    }

    #[test]
    fn a_windowed_read_is_remembered_as_a_view_narrower_than_the_file() {
        let memory = ToolResultMemory::inline(2000, 400, true).covering(ReadCoverage::FileHashed);
        assert_eq!(memory.coverage(), Some(ReadCoverage::FileHashed));
        assert!(
            !memory.coverage().expect("a coverage").covers_model_view(),
            "an overwrite authorized against this would delete lines nobody saw"
        );
    }
}
