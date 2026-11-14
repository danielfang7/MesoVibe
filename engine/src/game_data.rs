//! Classic World's items and maps as reference data: names, inventory tabs and NPC sell prices,
//! for naming what the player picks up and valuing it.
//!
//! The data ships inside the engine (engine/data, from OSMS Data Explorer under CC BY 4.0, credited
//! in that folder's README), so reading it never touches the network or the game's files.
//! `node scripts/game-data.mjs` refreshes it after a game patch.

use std::sync::OnceLock;

use crate::recognizers::notices::Tab;

const ITEMS: &str = include_str!("../data/items.tsv");
const MAPS: &str = include_str!("../data/maps.tsv");

/// An item the game has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: u32,
    /// The name as the game writes it.
    pub name: &'static str,
    /// The inventory tab it goes to when picked up.
    pub tab: Tab,
    /// Mesos an NPC shop pays for one, or `None` when shops won't buy it.
    pub sell: Option<u32>,
    pub quest: bool,
    /// It can't be traded to other players, so it has no Free Market price.
    pub untradeable: bool,
}

/// A map the game has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Map {
    pub id: u32,
    /// The minimap's first line, such as "Victoria Road". Empty for a few maps.
    pub region: &'static str,
    pub name: &'static str,
}

/// Every item, sorted by ID.
pub fn items() -> &'static [Item] {
    static PARSED: OnceLock<Vec<Item>> = OnceLock::new();
    // The bundled file is checked by the tests below, so a parse error here can't ship; if it
    // somehow did, the engine carries on without reference data instead of failing.
    PARSED.get_or_init(|| parse_items(ITEMS).unwrap_or_default())
}

/// Every map, sorted by ID.
pub fn maps() -> &'static [Map] {
    static PARSED: OnceLock<Vec<Map>> = OnceLock::new();
    PARSED.get_or_init(|| parse_maps(MAPS).unwrap_or_default())
}

/// The data lines of a TSV file, each split into exactly `columns` fields. The first line is the
/// header.
fn rows(text: &str, columns: usize) -> Result<Vec<(usize, Vec<&str>)>, String> {
    text.lines()
        .enumerate()
        .skip(1)
        .map(|(i, line)| {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() == columns {
                Ok((i + 1, fields))
            } else {
                Err(format!(
                    "line {}: {} fields, not {columns}",
                    i + 1,
                    fields.len()
                ))
            }
        })
        .collect()
}

/// A line's ID, which must be higher than the line before's.
fn id(line: usize, text: &str, last: &mut Option<u32>) -> Result<u32, String> {
    let id: u32 = text
        .parse()
        .map_err(|_| format!("line {line}: ID {text:?}"))?;
    if last.is_some_and(|last| id <= last) {
        return Err(format!("line {line}: ID {id} out of order"));
    }
    *last = Some(id);
    Ok(id)
}

fn tab(name: &str) -> Option<Tab> {
    match name {
        "Equip" => Some(Tab::Equip),
        "Use" => Some(Tab::Use),
        "Setup" => Some(Tab::Setup),
        "Etc" => Some(Tab::Etc),
        "Cash" => Some(Tab::Cash),
        _ => None,
    }
}

fn parse_items(text: &'static str) -> Result<Vec<Item>, String> {
    let mut last = None;
    rows(text, 5)?
        .into_iter()
        .map(|(line, f)| {
            let sell = match f[2] {
                "" => None,
                n => Some(n.parse().map_err(|_| format!("line {line}: sell {n:?}"))?),
            };
            let flags: Vec<&str> = f[3].split(',').filter(|s| !s.is_empty()).collect();
            if let Some(flag) = flags.iter().find(|f| !["quest", "untradeable"].contains(f)) {
                return Err(format!("line {line}: flag {flag:?}"));
            }
            if f[4].is_empty() {
                return Err(format!("line {line}: no name"));
            }
            Ok(Item {
                id: id(line, f[0], &mut last)?,
                tab: tab(f[1]).ok_or(format!("line {line}: tab {:?}", f[1]))?,
                sell,
                quest: flags.contains(&"quest"),
                untradeable: flags.contains(&"untradeable"),
                name: f[4],
            })
        })
        .collect()
}

fn parse_maps(text: &'static str) -> Result<Vec<Map>, String> {
    let mut last = None;
    rows(text, 3)?
        .into_iter()
        .map(|(line, f)| {
            if f[2].is_empty() {
                return Err(format!("line {line}: no name"));
            }
            Ok(Map {
                id: id(line, f[0], &mut last)?,
                region: f[1],
                name: f[2],
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The bundled files must parse whole: items() and maps() would quietly come back empty.
    #[test]
    fn bundled_data_parses() {
        let parsed_items = parse_items(ITEMS).unwrap();
        let parsed_maps = parse_maps(MAPS).unwrap();
        assert!(parsed_items.len() > 2000, "{} items", parsed_items.len());
        assert!(parsed_maps.len() > 300, "{} maps", parsed_maps.len());
        assert_eq!(parsed_items.len(), items().len());
        assert_eq!(parsed_maps.len(), maps().len());
    }

    // Known values from the game: catches columns shifted or swapped by a script change.
    #[test]
    fn known_items_and_maps_read_right() {
        let find = |name| items().iter().find(|i| i.name == name).unwrap();
        let ribbon = find("Pig's Ribbon");
        assert_eq!(
            (ribbon.id, ribbon.tab, ribbon.sell),
            (4000010, Tab::Etc, Some(10))
        );
        assert_eq!(find("Orange Potion").tab, Tab::Use);
        let town = maps().iter().find(|m| m.id == 10).unwrap();
        assert_eq!((town.region, town.name), ("Maple Road", "Mushroom Town"));
    }

    // An item's tab and its ID's first digit must agree, the way the game files them.
    #[test]
    fn tabs_match_item_ids() {
        for item in items() {
            let digit = match item.tab {
                Tab::Equip => 1,
                Tab::Use => 2,
                Tab::Setup => 3,
                Tab::Etc => 4,
                Tab::Cash => 5,
            };
            assert_eq!(item.id / 1_000_000, digit, "{} ({})", item.name, item.id);
        }
    }

    // A damaged file is an error, not a quietly shorter list.
    #[test]
    fn rejects_damaged_lines() {
        let header = "id\ttab\tsell\tflags\tname\n";
        for bad in [
            "4000010\tEtc\t10\tPig's Ribbon\n",
            "4000010\tBag\t10\t\tPig's Ribbon\n",
            "4000010\tEtc\tten\t\tPig's Ribbon\n",
            "4000010\tEtc\t10\tshiny\tPig's Ribbon\n",
            "4000010\tEtc\t10\t\t\n",
            "4000010\tEtc\t10\t\tB\n4000009\tEtc\t8\t\tA\n",
        ] {
            let text: &'static str = format!("{header}{bad}").leak();
            assert!(parse_items(text).is_err(), "{bad:?}");
        }
        assert!(parse_maps("id\tregion\tname\n10\tMaple Road\t\n").is_err());
    }
}
