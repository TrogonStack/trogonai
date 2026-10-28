use axum::http::HeaderMap;

use super::constants::{HEADER_RETRY_NUM, HEADER_RETRY_REASON, NATS_HEADER_RETRY_NUM, NATS_HEADER_RETRY_REASON};
use super::header_text::{DeliveryHeaders, HeaderText, InvalidHeaderTextError};
use super::retry_attempt::RetryAttempt;
use super::retry_reason::RetryReason;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeliveryContext {
    retry_num: Option<RetryAttempt>,
    retry_reason: Option<RetryReason>,
}

impl DeliveryContext {
    pub fn http(headers: &HeaderMap) -> Self {
        let text = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
        Self {
            retry_num: text(HEADER_RETRY_NUM)
                .and_then(parse_leading_integer)
                .map(RetryAttempt::new),
            retry_reason: text(HEADER_RETRY_REASON).and_then(|reason| RetryReason::new(reason).ok()),
        }
    }

    pub fn socket(retry_attempt: Option<i64>, retry_reason: Option<&str>) -> Result<Self, InvalidHeaderTextError> {
        Ok(Self {
            retry_num: retry_attempt.and_then(RetryAttempt::socket),
            retry_reason: match retry_reason {
                Some(reason) => RetryReason::socket(reason)?,
                None => None,
            },
        })
    }

    pub fn apply(&self, headers: DeliveryHeaders) -> DeliveryHeaders {
        let retry_num = self.retry_num.map(|num| HeaderText::integer(num.get()));
        headers
            .with_optional_text(NATS_HEADER_RETRY_NUM, retry_num.as_ref())
            .with_optional_text(
                NATS_HEADER_RETRY_REASON,
                self.retry_reason.as_ref().map(RetryReason::as_header_text),
            )
    }
}

fn parse_leading_integer(text: &str) -> Option<i64> {
    let text = text.trim_start();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    digits[..end].parse::<i64>().ok().map(|num| sign * num)
}

#[cfg(test)]
mod tests;
