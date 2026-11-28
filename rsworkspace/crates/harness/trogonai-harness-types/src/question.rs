//! Asking the user a question the model cannot answer for itself.

use serde::{Deserialize, Serialize};

/// One choice offered for a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    /// What the choice is called.
    pub label: String,
    /// What choosing it means, when that is not obvious from the label.
    pub description: Option<String>,
}

impl QuestionOption {
    /// A choice that speaks for itself.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: None,
        }
    }

    /// The same choice, saying what it means.
    #[must_use]
    pub fn described(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// One question and what may be answered to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionBatchEntry {
    /// What is being asked.
    pub question: String,
    /// What may be answered.
    pub options: Vec<QuestionOption>,
}

impl QuestionBatchEntry {
    /// A question with the choices it offers.
    pub fn new(question: impl Into<String>, options: Vec<QuestionOption>) -> Self {
        Self {
            question: question.into(),
            options,
        }
    }

    /// The offered choice by that label, if it was offered.
    #[must_use]
    pub fn option(&self, label: &str) -> Option<&QuestionOption> {
        self.options.iter().find(|option| option.label == label)
    }

    /// Whether the answer is one of the choices this question offered.
    #[must_use]
    pub fn offers(&self, answer: &QuestionAnswer) -> bool {
        answer.question == self.question && self.option(&answer.answer).is_some()
    }
}

/// One answer given to a question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    /// The question it answers.
    pub question: String,
    /// The label of the choice taken.
    pub answer: String,
}

impl QuestionAnswer {
    /// An answer to a question.
    pub fn new(question: impl Into<String>, answer: impl Into<String>) -> Self {
        Self {
            question: question.into(),
            answer: answer.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> QuestionBatchEntry {
        QuestionBatchEntry::new(
            "Which backend?",
            vec![
                QuestionOption::new("vercel").described("the hosted gateway"),
                QuestionOption::new("macos"),
            ],
        )
    }

    #[test]
    fn an_answer_naming_a_choice_that_was_never_offered_is_not_one_of_the_options() {
        let asked = entry();

        assert!(asked.offers(&QuestionAnswer::new("Which backend?", "macos")));
        assert!(!asked.offers(&QuestionAnswer::new("Which backend?", "aws")));
    }

    #[test]
    fn an_answer_to_a_different_question_is_not_an_answer_to_this_one() {
        assert!(!entry().offers(&QuestionAnswer::new("Which model?", "macos")));
    }

    #[test]
    fn a_choice_that_speaks_for_itself_carries_no_description() {
        let asked = entry();

        assert_eq!(
            asked.option("vercel").and_then(|o| o.description.as_deref()),
            Some("the hosted gateway")
        );
        assert_eq!(asked.option("macos").and_then(|o| o.description.as_deref()), None);
        assert!(asked.option("aws").is_none());
    }
}
