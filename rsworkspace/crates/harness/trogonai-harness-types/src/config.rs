//! Values a person configures, and the text they are configured from.

use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Which backend runs the session's commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    /// The sandboxed macOS backend.
    #[serde(rename = "macos")]
    MacOs,
    /// The hosted backend.
    #[serde(rename = "vercel")]
    Vercel,
    /// No sandbox: commands run as the person's own shell would run them.
    #[serde(rename = "just-bash")]
    JustBash,
    /// No backend, so no commands.
    #[serde(rename = "none")]
    None,
    /// Whichever backend the machine turns out to support.
    #[serde(rename = "auto")]
    Auto,
}

impl BackendKind {
    /// The spelling written back to configuration, which [`Self::from_str`]
    /// accepts.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::Vercel => "vercel",
            Self::JustBash => "just-bash",
            Self::None => "none",
            Self::Auto => "auto",
        }
    }

    /// The name this backend is written under inside a permission identity.
    ///
    /// They are kept apart because an identity is matched against answers already stored, and a
    /// rule keyed under one spelling would silently stop matching if it were written under the
    /// other.
    #[must_use]
    pub const fn tag_name(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::Vercel => "vercel",
            Self::JustBash => "just_bash",
            Self::None => "none",
            Self::Auto => "auto",
        }
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A configured string that named no backend.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a backend trogonai knows")]
pub struct UnknownBackend(pub String);

impl FromStr for BackendKind {
    type Err = UnknownBackend;

    /// Accepts the labels in any case, with surrounding whitespace.
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let trimmed = raw.trim_matches([' ', '\t', '\r', '\n']);
        for candidate in [Self::MacOs, Self::Vercel, Self::JustBash, Self::None, Self::Auto] {
            if trimmed.eq_ignore_ascii_case(candidate.label()) {
                return Ok(candidate);
            }
        }
        Err(UnknownBackend(raw.to_owned()))
    }
}

/// Where the credential in use came from.
///
/// This is recorded so a session can say which of several credentials answered
/// for it, which matters when one of them stops working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    /// A workload identity token from the deployment environment.
    VercelOidcToken,
    /// An API key held in the environment.
    AiGatewayApiKey,
    /// The credential the runtime obtained by logging the person in.
    TrogonaiLogin,
    /// A key the person saved.
    StoredKey,
    /// A ChatGPT subscription the person is signed in to.
    ChatgptSubscription,
    /// A Grok subscription the person is signed in to.
    GrokSubscription,
    /// No local credential: the embedding host authorizes the request itself.
    ///
    /// This one exists only while a process is running. Nothing writes it down,
    /// because a stored session that claimed it would be claiming authority the
    /// next process has no way to hold.
    HostManaged,
    /// A credential resolved from a configured provider declaration.
    ///
    /// Like [`Self::HostManaged`], this names a runtime slot rather than a
    /// persisted credential source string.
    Configured,
}

impl CredentialSource {
    /// The name this source is written and read under.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::VercelOidcToken => "vercel_oidc_token",
            Self::AiGatewayApiKey => "ai_gateway_api_key",
            Self::TrogonaiLogin => "trogonai_login",
            Self::StoredKey => "stored_key",
            Self::ChatgptSubscription => "chatgpt_subscription",
            Self::GrokSubscription => "grok_subscription",
            Self::HostManaged => "host_managed",
            Self::Configured => "configured",
        }
    }

    /// Whether this source can outlive the process that observed it.
    #[must_use]
    pub fn is_persistable(self) -> bool {
        !matches!(self, Self::HostManaged | Self::Configured)
    }

    /// Reads a source named by something already running, host authority included.
    ///
    /// The counterpart for bytes read back from storage is [`FromStr`], which
    /// refuses [`Self::HostManaged`].
    #[must_use]
    pub fn from_runtime_label(raw: &str) -> Option<Self> {
        [
            Self::VercelOidcToken,
            Self::AiGatewayApiKey,
            Self::TrogonaiLogin,
            Self::StoredKey,
            Self::ChatgptSubscription,
            Self::GrokSubscription,
            Self::HostManaged,
            Self::Configured,
        ]
        .into_iter()
        .find(|candidate| raw == candidate.label())
    }
}

impl fmt::Display for CredentialSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// A persisted string that named no credential source.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a credential source trogonai knows")]
pub struct UnknownCredentialSource(pub String);

impl FromStr for CredentialSource {
    type Err = UnknownCredentialSource;

    /// Accepts exactly the persisted names.
    ///
    /// Unlike a backend, this string is written by the runtime rather than by a person, so there is
    /// nothing to be lenient about. [`CredentialSource::HostManaged`] is refused here and
    /// accepted only by [`CredentialSource::from_runtime_label`].
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match Self::from_runtime_label(raw) {
            Some(source) if source.is_persistable() => Ok(source),
            _ => Err(UnknownCredentialSource(raw.to_owned())),
        }
    }
}

impl<'de> Deserialize<'de> for CredentialSource {
    /// Reads a stored source, which is the one place [`Self::HostManaged`] is
    /// not a source at all.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = <&str>::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// A rejected conversation language tag.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConversationLanguageError {
    /// The tag held nothing once its surrounding whitespace was removed.
    #[error("a conversation language must not be empty")]
    Empty,
    /// The tag was longer than any language tag the runtime stores.
    #[error("a conversation language must be at most {max} bytes, got {length}")]
    TooLong {
        /// How long the candidate was.
        length: usize,
        /// The most it could have been.
        max: usize,
    },
}

/// The language a conversation is being held in.
///
/// Mirrors `ConversationLanguage`, whose 24-byte inline buffer is a bound on
/// what a BCP 47 tag can be rather than a storage trick: a value longer than
/// that is not a mistyped tag, it is not a tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ConversationLanguage(String);

impl ConversationLanguage {
    /// The most a language tag can hold.
    pub const MAX_BYTES: usize = 24;

    /// The tag for a conversation whose language nobody has established.
    pub const UNDETERMINED: &'static str = "und";

    /// Accepts a tag, ignoring whitespace around it.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationLanguageError`] when the trimmed tag is empty or
    /// longer than [`Self::MAX_BYTES`].
    pub fn new(tag: &str) -> Result<Self, ConversationLanguageError> {
        let trimmed = tag.trim_matches([' ', '\t', '\r', '\n']);
        if trimmed.is_empty() {
            return Err(ConversationLanguageError::Empty);
        }
        if trimmed.len() > Self::MAX_BYTES {
            return Err(ConversationLanguageError::TooLong {
                length: trimmed.len(),
                max: Self::MAX_BYTES,
            });
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The tag itself.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the language is still the undetermined one.
    #[must_use]
    pub fn is_undetermined(&self) -> bool {
        self.0 == Self::UNDETERMINED
    }

    /// The label a session listing prints beside a turn count.
    ///
    /// Undetermined tags are omitted; script-only tags name the script; known language tags
    /// name the language; everything else is shown as stored.
    #[must_use]
    pub fn session_list_label(&self) -> Option<Cow<'_, str>> {
        let tag = self.as_str();
        if tag.eq_ignore_ascii_case(Self::UNDETERMINED) {
            return None;
        }

        if tag.len() > 4 && tag[..4].eq_ignore_ascii_case("und-") {
            let script = &tag[4..];
            return Some(Cow::Borrowed(undetermined_script_label(script).unwrap_or(tag)));
        }

        let primary = tag.split('-').next().unwrap_or(tag);
        Some(Cow::Borrowed(primary_language_label(primary).unwrap_or(tag)))
    }
}

fn undetermined_script_label(script: &str) -> Option<&'static str> {
    if script.eq_ignore_ascii_case("Latn") {
        return Some("Latin script");
    }
    if script.eq_ignore_ascii_case("Hani") {
        return Some("Han script");
    }
    if script.eq_ignore_ascii_case("Arab") {
        return Some("Arabic script");
    }
    if script.eq_ignore_ascii_case("Hebr") {
        return Some("Hebrew script");
    }
    if script.eq_ignore_ascii_case("Cyrl") {
        return Some("Cyrillic script");
    }
    if script.eq_ignore_ascii_case("Grek") {
        return Some("Greek script");
    }
    if script.eq_ignore_ascii_case("Deva") {
        return Some("Devanagari script");
    }
    if script.eq_ignore_ascii_case("Thai") {
        return Some("Thai script");
    }
    None
}

fn primary_language_label(primary: &str) -> Option<&'static str> {
    match () {
        _ if primary.eq_ignore_ascii_case("en") => Some("English"),
        _ if primary.eq_ignore_ascii_case("es") => Some("Spanish"),
        _ if primary.eq_ignore_ascii_case("fr") => Some("French"),
        _ if primary.eq_ignore_ascii_case("de") => Some("German"),
        _ if primary.eq_ignore_ascii_case("it") => Some("Italian"),
        _ if primary.eq_ignore_ascii_case("pt") => Some("Portuguese"),
        _ if primary.eq_ignore_ascii_case("ja") => Some("Japanese"),
        _ if primary.eq_ignore_ascii_case("ko") => Some("Korean"),
        _ if primary.eq_ignore_ascii_case("zh") => Some("Chinese"),
        _ if primary.eq_ignore_ascii_case("ar") => Some("Arabic"),
        _ if primary.eq_ignore_ascii_case("he") => Some("Hebrew"),
        _ if primary.eq_ignore_ascii_case("ru") => Some("Russian"),
        _ if primary.eq_ignore_ascii_case("el") => Some("Greek"),
        _ if primary.eq_ignore_ascii_case("hi") => Some("Hindi"),
        _ if primary.eq_ignore_ascii_case("th") => Some("Thai"),
        _ => None,
    }
}

impl Default for ConversationLanguage {
    /// The undetermined tag, which is what a conversation starts out in.
    fn default() -> Self {
        Self(Self::UNDETERMINED.to_owned())
    }
}

impl fmt::Display for ConversationLanguage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for ConversationLanguage {
    type Err = ConversationLanguageError;

    fn from_str(tag: &str) -> Result<Self, Self::Err> {
        Self::new(tag)
    }
}

impl TryFrom<String> for ConversationLanguage {
    type Error = ConversationLanguageError;

    fn try_from(tag: String) -> Result<Self, Self::Error> {
        Self::new(&tag)
    }
}

impl From<ConversationLanguage> for String {
    fn from(language: ConversationLanguage) -> Self {
        language.0
    }
}

/// A rejected reasoning effort name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EffortNameError {
    /// The name held nothing.
    #[error("a reasoning effort name must not be empty")]
    Empty,
    /// The name was longer than any provider's own.
    #[error("a reasoning effort name must be at most {max} bytes, got {length}")]
    TooLong {
        /// How long the candidate was.
        length: usize,
        /// The most it could have been.
        max: usize,
    },
    /// The name held something that cannot appear in a provider's vocabulary.
    #[error("a reasoning effort name may only hold letters, digits, `-`, `_` and `.`, got `{0}`")]
    Unnameable(String),
}

/// One provider's word for how hard to think.
///
/// Mirrors `ReasoningEffort.Name`. The vocabulary is the provider's rather than
/// The runtime's, so this is deliberately not an enum of `low`/`medium`/`high`: it is a
/// bounded string, restricted to what can appear in a slug so a configured
/// value cannot smuggle anything into a request body or a status line.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EffortName(String);

impl EffortName {
    /// The most a name can hold.
    pub const MAX_BYTES: usize = 64;

    /// Accepts a provider's name for an effort level.
    ///
    /// # Errors
    ///
    /// Returns [`EffortNameError`] when the name is empty, longer than
    /// [`Self::MAX_BYTES`], or holds anything but letters, digits, `-`, `_` and
    /// `.`.
    pub fn new(raw: &str) -> Result<Self, EffortNameError> {
        if raw.is_empty() {
            return Err(EffortNameError::Empty);
        }
        if raw.len() > Self::MAX_BYTES {
            return Err(EffortNameError::TooLong {
                length: raw.len(),
                max: Self::MAX_BYTES,
            });
        }
        if !raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(EffortNameError::Unnameable(raw.to_owned()));
        }
        Ok(Self(raw.to_owned()))
    }

    /// The name itself.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EffortName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for EffortName {
    type Err = EffortNameError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::new(raw)
    }
}

impl TryFrom<String> for EffortName {
    type Error = EffortNameError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        Self::new(&raw)
    }
}

impl From<EffortName> for String {
    fn from(name: EffortName) -> Self {
        name.0
    }
}

/// How hard a model should think, when it offers the choice.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ReasoningEffort {
    /// Whatever the model does when nobody says.
    #[default]
    Auto,
    /// A level the provider named.
    Named(EffortName),
}

impl ReasoningEffort {
    /// The most effort levels the runtime will list for one model.
    pub const MAX_OPTIONS: usize = 16;

    /// The spelling sent to the provider or written to configuration.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Named(name) => name.as_str(),
        }
    }

    /// The spelling shown to the person.
    ///
    /// Automatic effort reads as "default" here and as "auto" in
    /// [`Self::label`]: one is what a person recognizes in a menu, the other is
    /// what round-trips through configuration.
    #[must_use]
    pub fn display_label(&self) -> &str {
        match self {
            Self::Auto => "default",
            Self::Named(name) => name.as_str(),
        }
    }

    /// What to send the gateway, which is nothing when the model decides.
    #[must_use]
    pub fn gateway_value(&self) -> Option<&str> {
        match self {
            Self::Auto => None,
            Self::Named(name) => Some(name.as_str()),
        }
    }

    /// Whether this is the level the runtime picks when nobody has.
    #[must_use]
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Auto)
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

impl FromStr for ReasoningEffort {
    type Err = EffortNameError;

    /// Accepts the three spellings of automatic in any case, then any name.
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        if ["auto", "adaptive", "default"]
            .iter()
            .any(|spelling| raw.eq_ignore_ascii_case(spelling))
        {
            return Ok(Self::Auto);
        }
        Ok(Self::Named(EffortName::new(raw)?))
    }
}

impl TryFrom<String> for ReasoningEffort {
    type Error = EffortNameError;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        raw.parse()
    }
}

impl From<ReasoningEffort> for String {
    fn from(effort: ReasoningEffort) -> Self {
        effort.label().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_backend_round_trips_through_its_label() {
        for backend in [
            BackendKind::MacOs,
            BackendKind::Vercel,
            BackendKind::JustBash,
            BackendKind::None,
            BackendKind::Auto,
        ] {
            assert_eq!(backend.label().parse(), Ok(backend));
        }
    }

    #[test]
    fn a_backend_typed_with_stray_spacing_or_capitals_still_parses() {
        assert_eq!("  MacOS \n".parse(), Ok(BackendKind::MacOs));
        assert_eq!("JUST-BASH".parse(), Ok(BackendKind::JustBash));
        assert_eq!(
            "just_bash".parse::<BackendKind>(),
            Err(UnknownBackend("just_bash".to_owned())),
        );
        assert_eq!("".parse::<BackendKind>(), Err(UnknownBackend(String::new())),);
    }

    #[test]
    fn a_credential_source_round_trips_through_its_persisted_name() {
        for source in [
            CredentialSource::VercelOidcToken,
            CredentialSource::AiGatewayApiKey,
            CredentialSource::TrogonaiLogin,
            CredentialSource::StoredKey,
        ] {
            assert_eq!(source.label().parse(), Ok(source));
        }
    }

    #[test]
    fn a_credential_source_trogonai_did_not_write_is_not_guessed_at() {
        assert_eq!(
            "keychain".parse::<CredentialSource>(),
            Err(UnknownCredentialSource("keychain".to_owned())),
        );
        assert_eq!(
            "TROGONAI_LOGIN".parse::<CredentialSource>(),
            Err(UnknownCredentialSource("TROGONAI_LOGIN".to_owned())),
        );
        assert_eq!(
            " trogonai_login".parse::<CredentialSource>(),
            Err(UnknownCredentialSource(" trogonai_login".to_owned())),
        );
    }

    #[test]
    fn a_conversation_starts_out_in_no_established_language() {
        let language = ConversationLanguage::default();
        assert_eq!(language.as_str(), "und");
        assert!(language.is_undetermined());
        assert!(!ConversationLanguage::new("en-US").expect("a tag").is_undetermined());
    }

    #[test]
    fn a_language_tag_is_taken_without_the_whitespace_around_it() {
        assert_eq!(ConversationLanguage::new("  pt-BR\n").expect("a tag").as_str(), "pt-BR",);
    }

    #[test]
    fn a_session_list_omits_an_undetermined_language_and_names_the_rest() {
        assert_eq!(ConversationLanguage::default().session_list_label(), None,);
        assert_eq!(
            ConversationLanguage::new("und-Latn")
                .expect("a script tag")
                .session_list_label()
                .map(Cow::into_owned),
            Some("Latin script".to_owned()),
        );
        assert_eq!(
            ConversationLanguage::new("es")
                .expect("a language tag")
                .session_list_label()
                .map(Cow::into_owned),
            Some("Spanish".to_owned()),
        );
        assert_eq!(
            ConversationLanguage::new("en-US")
                .expect("a language tag")
                .session_list_label()
                .map(Cow::into_owned),
            Some("English".to_owned()),
        );
        assert_eq!(
            ConversationLanguage::new("und-zzzz")
                .expect("a script tag")
                .session_list_label()
                .map(Cow::into_owned),
            Some("und-zzzz".to_owned()),
        );
        assert_eq!(
            ConversationLanguage::new("xx-YY")
                .expect("a language tag")
                .session_list_label()
                .map(Cow::into_owned),
            Some("xx-YY".to_owned()),
        );
    }

    #[test]
    fn a_language_tag_that_is_empty_or_oversized_is_refused() {
        assert_eq!(ConversationLanguage::new("   "), Err(ConversationLanguageError::Empty),);
        let long = "x".repeat(ConversationLanguage::MAX_BYTES + 1);
        assert_eq!(
            ConversationLanguage::new(&long),
            Err(ConversationLanguageError::TooLong {
                length: ConversationLanguage::MAX_BYTES + 1,
                max: ConversationLanguage::MAX_BYTES,
            }),
        );
        assert!(ConversationLanguage::new(&"x".repeat(ConversationLanguage::MAX_BYTES)).is_ok());
    }

    #[test]
    fn the_three_spellings_of_automatic_effort_all_mean_the_same_thing() {
        for spelling in ["auto", "adaptive", "default", "AUTO", "Default"] {
            assert_eq!(spelling.parse(), Ok(ReasoningEffort::Auto));
        }
        assert!(ReasoningEffort::default().is_default());
        assert_eq!(ReasoningEffort::Auto.gateway_value(), None);
        assert_eq!(ReasoningEffort::Auto.label(), "auto");
        assert_eq!(ReasoningEffort::Auto.display_label(), "default");
    }

    #[test]
    fn a_named_effort_keeps_the_case_the_provider_uses() {
        let effort: ReasoningEffort = "xHigh".parse().expect("a name");
        assert_eq!(effort.label(), "xHigh");
        assert_eq!(effort.display_label(), "xHigh");
        assert_eq!(effort.gateway_value(), Some("xHigh"));
        assert!(!effort.is_default());
        assert_ne!(effort, "xhigh".parse().expect("a name"));
    }

    #[test]
    fn an_effort_name_holds_nothing_that_could_change_a_request() {
        assert_eq!(EffortName::new(""), Err(EffortNameError::Empty));
        assert_eq!(
            EffortName::new("high effort"),
            Err(EffortNameError::Unnameable("high effort".to_owned())),
        );
        assert_eq!(
            EffortName::new("high\"}"),
            Err(EffortNameError::Unnameable("high\"}".to_owned())),
        );
        assert!(EffortName::new("gpt-5.1_high").is_ok());

        let long = "x".repeat(EffortName::MAX_BYTES + 1);
        assert_eq!(
            EffortName::new(&long),
            Err(EffortNameError::TooLong {
                length: EffortName::MAX_BYTES + 1,
                max: EffortName::MAX_BYTES,
            }),
        );
    }

    #[test]
    fn an_effort_round_trips_through_the_label_it_is_configured_by() {
        for raw in ["auto", "high", "gpt-5.1_high"] {
            let effort: ReasoningEffort = raw.parse().expect("an effort");
            assert_eq!(effort.label().parse(), Ok(effort));
        }
    }

    #[test]
    fn a_configured_value_survives_a_round_trip_through_json() {
        let effort: ReasoningEffort = "high".parse().expect("an effort");
        let encoded = serde_json::to_string(&effort).expect("an effort encodes");
        assert_eq!(encoded, "\"high\"");
        assert_eq!(
            serde_json::from_str::<ReasoningEffort>(&encoded).expect("an effort decodes"),
            effort,
        );

        assert_eq!(
            serde_json::to_string(&BackendKind::JustBash).expect("a backend encodes"),
            "\"just-bash\"",
        );
        assert_eq!(
            serde_json::to_string(&CredentialSource::VercelOidcToken).expect("a source encodes"),
            "\"vercel_oidc_token\"",
        );
        assert_eq!(
            serde_json::to_string(&ConversationLanguage::default()).expect("a tag encodes"),
            "\"und\"",
        );
        assert!(serde_json::from_str::<ConversationLanguage>("\"\"").is_err());
    }
}
