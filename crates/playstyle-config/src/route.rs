//! The route's bosses (`route.toml`): where the farm list's "where:" line looks besides the
//! level-85 areas.

use serde::Deserialize;

use crate::worth;

/// The route file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Route {
    /// In the order the "where:" line names them.
    pub bosses: Vec<String>,
}

/// Parses `route.toml`; the warnings name the bosses without drop data.
///
/// # Errors
/// Invalid TOML or a field of the wrong shape.
pub fn parse_route(text: &str) -> Result<(Route, Vec<String>), toml::de::Error> {
    let route: Route = toml::from_str(text)?;
    let warnings = route
        .bosses
        .iter()
        .filter(|boss| worth::boss_drops(boss, 0).is_none())
        .map(|boss| format!("no drop data for boss {boss:?} (never shown)"))
        .collect();
    Ok((route, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[test]
    fn the_shipped_route_parses_without_warnings() -> Result<()> {
        let (route, warnings) = parse_route(include_str!("../../../playstyles/fury-druid/route.toml"))?;
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(route.bosses, ["The Countess", "Mephisto", "Andariel", "Baal"]);
        let (_, warnings) = parse_route("bosses = [\"Nobody\"]")?;
        assert_eq!(warnings, ["no drop data for boss \"Nobody\" (never shown)"]);
        Ok(())
    }
}
