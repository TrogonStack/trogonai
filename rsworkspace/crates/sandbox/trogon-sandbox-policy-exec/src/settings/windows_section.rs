//! The `windows` section, read so a refused value is reported with the rest
//! of the document in the same wording.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, SeqAccess, Visitor};

use super::constants::{self, MAX_RANGE_WIDTH, MAX_SANDBOX_USER_LEN};
use super::issue_wording;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WindowsSection {
    #[serde(default, deserialize_with = "sandbox_user")]
    pub(super) sandbox_user: Option<String>,
    #[serde(default, deserialize_with = "uuid")]
    pub(super) sublayer_guid: Option<String>,
    #[serde(default, deserialize_with = "uuid")]
    pub(super) wfp_sublayer_guid: Option<String>,
    #[serde(default, deserialize_with = "port_range")]
    pub(super) proxy_port_range: Option<(u16, u16)>,
    #[serde(default, deserialize_with = "super::object_present")]
    pub(super) fence_helper: Option<FenceHelperSection>,
}

#[derive(Debug, Deserialize)]
pub(super) struct FenceHelperSection {
    #[serde(deserialize_with = "non_empty")]
    pub(super) path: String,
}

fn non_empty<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let text = String::deserialize(deserializer)?;
    if text.is_empty() {
        return Err(de::Error::custom(constants::NON_EMPTY));
    }
    Ok(text)
}

fn sandbox_user<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let name = non_empty(deserializer)?;
    if name.encode_utf16().count() > MAX_SANDBOX_USER_LEN {
        return Err(de::Error::custom(constants::SANDBOX_USER_TOO_LONG));
    }
    Ok(Some(name))
}

fn uuid<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    let text = String::deserialize(deserializer)?;
    if !issue_wording::uuid_shaped(&text) {
        return Err(de::Error::custom(constants::INVALID_UUID));
    }
    Ok(Some(text))
}

fn port_range<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<(u16, u16)>, D::Error> {
    struct Pair;

    impl<'de> Visitor<'de> for Pair {
        type Value = (u16, u16);

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a sequence")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(u16, u16), A::Error> {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element::<issue_wording::Loose>()? {
                items.push(item);
            }
            match items.len() {
                0 | 1 => {
                    return Err(de::Error::custom("Array must contain at least 2 element(s)"));
                }
                2 => {}
                _ => return Err(de::Error::custom("Array must contain at most 2 element(s)")),
            }
            let mut issues: Vec<(&str, String)> = Vec::new();
            let mut numbers = [None, None];
            for (index, item) in items.iter().enumerate() {
                let (min, max) = if index == 0 {
                    (Some(1), None)
                } else {
                    (None, Some(65535))
                };
                let at = ["[0]", "[1]"][index];
                match item {
                    issue_wording::Loose::Number(value) => {
                        numbers[index] = Some(*value);
                        for message in issue_wording::number_issues(*value, min, max) {
                            issues.push((at, message));
                        }
                    }
                    other => issues.push((
                        at,
                        issue_wording::aborting(format!("Expected number, received {}", other.kind())),
                    )),
                }
            }
            if let [Some(low), Some(high)] = numbers
                && !(low <= high && high - low <= MAX_RANGE_WIDTH)
            {
                issues.push(("", constants::PORT_RANGE.to_owned()));
            }
            if !issues.is_empty() {
                let issues: Vec<(&str, &str)> = issues.iter().map(|(at, message)| (*at, message.as_str())).collect();
                return Err(de::Error::custom(issue_wording::encode(&issues)));
            }
            let [Some(low), Some(high)] = numbers else {
                return Err(de::Error::custom(constants::PORT_RANGE));
            };
            Ok((low as u16, high as u16))
        }
    }

    deserializer.deserialize_seq(Pair).map(Some)
}
