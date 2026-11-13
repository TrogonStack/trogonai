/// Which upstream SDK's JSON number semantics a settings read follows.
///
/// `TypeScript` reads like `JSON.parse`: every number is a double, so an integer above 2^53 rounds to the nearest
/// double and prints back as the shortest digits that identify it. `Python` reads like `json.loads`: an integer
/// token stays an exact integer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SettingsDialect {
    #[default]
    TypeScript,
    Python,
}
