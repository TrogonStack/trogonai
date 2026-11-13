/// Which upstream SDK's summary layout and prompt rules a fold follows.
///
/// `TypeScript` stores camelCase `data` keys (`isSidechain`, `firstPromptLocked`), honors `relocated` entries, shows
/// bash input as `! command` and measures prompts in UTF-16 units. `Python` stores snake_case keys.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SummaryDialect {
    #[default]
    TypeScript,
    Python,
}
