//! Classic World's items and maps as reference data: names, inventory tabs and NPC sell prices,
//! for naming what the player picks up and valuing it. [`name_item`] finds the item a pickup
//! notice names, even while the reader knows only some of the name's letters.
//!
//! The data ships inside the engine (engine/data, from OSMS Data Explorer under CC BY 4.0, credited
//! in that folder's README), so reading it never touches the network or the game's files.
//! `node scripts/game-data.mjs` refreshes it after a game patch.

use std::sync::OnceLock;

use crate::recognizers::notices::{NameLetter, Tab};

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

/// The items a picked-up item's name fits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemMatch {
    /// The items that fit, sorted by ID. Several when the game cut the name short, when the game
    /// has more than one item of that name, or when the letters read so far fit more than one.
    pub items: Vec<&'static Item>,
    /// The name to show: the items' name when they share it, or else the start every one of their
    /// names shares, then "…". `None` when their names start differently.
    pub name: Option<String>,
}

impl ItemMatch {
    /// The item's ID, when only one item fits.
    pub fn id(&self) -> Option<u32> {
        match self.items.as_slice() {
            [item] => Some(item.id),
            _ => None,
        }
    }

    /// Mesos an NPC shop pays for one, when every item that fits sells for the same.
    pub fn sell(&self) -> Option<u32> {
        let first = self.items.first()?.sell;
        self.items
            .iter()
            .all(|item| item.sell == first)
            .then_some(first)
            .flatten()
    }
}

/// Finds the items of `tab` (every tab while it isn't known) whose names fit `letters`: an item's
/// name from a pickup notice, as far as the reader could read it.
///
/// A name fits when each letter the reader knows is the name's letter in that place, and letters
/// drawn alike are the same letter. Spaces aren't compared, as the gap after a narrow letter can
/// look like one. Letters drawn differently may still be one letter, and a shape the reader
/// doesn't know may be a letter it does: a speck of the scene over a letter changes its shape.
/// When the name ends in "...", the game cut it short, so every longer name that starts with the
/// rest fits.
///
/// Two touching letters read as one piece, and a letter a speck splits reads as two, so the name
/// may be a letter longer or shorter than read. Once some item fits, every item those misreads
/// fit joins the match too: a single item is named only when no misread could be another one.
///
/// `None` when no item fits.
pub fn name_item(letters: &[NameLetter], tab: Option<Tab>) -> Option<ItemMatch> {
    let cut = letters.len() > 3
        && letters[letters.len() - 3..]
            .iter()
            .all(|l| l.known == Some('.'));
    let read = if cut {
        &letters[..letters.len() - 3]
    } else {
        letters
    };
    if read.is_empty() {
        return None;
    }
    let in_tab = || {
        items()
            .iter()
            .filter(move |item| tab.is_none_or(|tab| item.tab == tab))
    };
    let mut found: Vec<&'static Item> =
        in_tab().filter(|item| fits(read, cut, item.name)).collect();
    if found.is_empty() {
        return None;
    }
    for misread in misreads(read) {
        for item in in_tab().filter(|item| fits(&misread, cut, item.name)) {
            if !found.iter().any(|f| f.id == item.id) {
                found.push(item);
            }
        }
    }
    found.sort_by_key(|item| item.id);
    Some(ItemMatch {
        name: shared_name(&found),
        items: found,
    })
}

/// A letter of a misread that stands for any letter: see [`misreads`].
const ANY_SHAPE: u16 = u16::MAX;

/// The other ways the reader could have cut the same name: one unknown piece that was two
/// touching letters, or two unknown pieces in a row that were one letter split in two. A known
/// letter's piece was cut right, as its shape matched.
fn misreads(read: &[NameLetter]) -> impl Iterator<Item = Vec<NameLetter>> + '_ {
    let any = NameLetter {
        known: None,
        shape: ANY_SHAPE,
    };
    let unknown = |i: usize| read[i].known.is_none();
    let touching = (0..read.len()).filter(move |&i| unknown(i)).map(move |i| {
        let mut misread = read.to_vec();
        misread.splice(i..=i, [any, any]);
        misread
    });
    let split = (1..read.len())
        .filter(move |&i| unknown(i - 1) && unknown(i))
        .map(move |i| {
            let mut misread = read.to_vec();
            misread.splice(i - 1..=i, [any]);
            misread
        });
    touching.chain(split)
}

/// Whether `name` fits the letters read, by [`name_item`]'s rules.
fn fits(read: &[NameLetter], cut: bool, name: &str) -> bool {
    let mut chars = name.chars().filter(|c| *c != ' ');
    // The letter each of the read name's shapes stands for, once a place has shown it.
    let shape_count = read
        .iter()
        .filter(|l| l.shape != ANY_SHAPE)
        .map(|l| usize::from(l.shape) + 1)
        .max()
        .unwrap_or(0);
    let mut shapes: Vec<Option<char>> = vec![None; shape_count];
    for letter in read {
        let Some(c) = chars.next() else {
            return false;
        };
        if letter.known.is_some_and(|known| known != c) {
            return false;
        }
        if letter.shape == ANY_SHAPE {
            continue;
        }
        if *shapes[usize::from(letter.shape)].get_or_insert(c) != c {
            return false;
        }
    }
    // A cut name goes on past what was shown; a whole one ends here.
    chars.next().is_some() == cut
}

/// The name to show for `items`: theirs when they share it, or else the start all their names
/// share, then "…". `None` when they share nothing.
fn shared_name(items: &[&Item]) -> Option<String> {
    let (first, rest) = items.split_first()?;
    if rest.iter().all(|item| item.name == first.name) {
        return Some(first.name.to_string());
    }
    let shared = rest.iter().fold(first.name, |shared, item| {
        let len = shared
            .chars()
            .zip(item.name.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.len_utf8())
            .sum();
        &shared[..len]
    });
    let shared = shared.trim_end();
    (!shared.is_empty()).then(|| format!("{shared}…"))
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

    /// The letters the reader learns from the game's own lines before it sees any item: the EXP
    /// and mesos lines, "earned.", the amounts and the tabs' names.
    const PHRASES: &str = "YoureceivedEXPYouhavegainedmesosearned.(+)x0123456789,EquipUseEtcSetup";

    /// `name` as the reader reads it knowing only `known`'s letters, each letter drawn the same
    /// way every time.
    fn read(name: &str, known: &str) -> Vec<NameLetter> {
        let mut shapes: Vec<char> = Vec::new();
        name.chars()
            .filter(|c| *c != ' ')
            .map(|c| {
                let shape = shapes.iter().position(|s| *s == c).unwrap_or_else(|| {
                    shapes.push(c);
                    shapes.len() - 1
                });
                NameLetter {
                    known: known.contains(c).then_some(c),
                    shape: shape as u16,
                }
            })
            .collect()
    }

    fn found(name: &str, tab: Tab) -> Option<ItemMatch> {
        name_item(&read(name, PHRASES), Some(tab))
    }

    // Names from real pickups, read knowing only the phrases' letters, find their one item.
    // Fails if a rule stops real names from fitting.
    #[test]
    fn names_find_their_item_with_only_the_phrases_letters() {
        for (name, tab, id, sell) in [
            ("Tablecloth", Tab::Etc, 4000028, 34),
            ("Jr. Necki Skin", Tab::Etc, 4000018, 21),
            ("Snail Shell", Tab::Etc, 4000001, 1),
            ("Orange Potion", Tab::Use, 2000001, 15),
        ] {
            let found = found(name, tab).unwrap();
            assert_eq!(
                (found.id(), found.name.as_deref(), found.sell()),
                (Some(id), Some(name), Some(sell))
            );
        }
    }

    // The game shortens long names with "...": the rest fits the longer names it starts. Fails
    // if a cut name finds nothing, or shows more of a name than every fitting item shares.
    #[test]
    fn a_name_the_game_cut_short_fits_the_names_it_starts() {
        let medicine = found("Medicine With...", Tab::Etc).unwrap();
        assert_eq!(medicine.id(), Some(4000039));
        assert_eq!(medicine.name.as_deref(), Some("Medicine With Weird Vibes"));

        let scroll = found("Hat Accuracy Sc...", Tab::Use).unwrap();
        assert_eq!(
            scroll.items.len(),
            4,
            "Lesser, Intermediate, Greater, Chaos"
        );
        assert_eq!(scroll.id(), None);
        assert_eq!(scroll.name.as_deref(), Some("Hat Accuracy Scroll:…"));
        assert_eq!(scroll.sell(), Some(1), "all four sell for 1");
    }

    // While the tab isn't read yet, every tab is searched. Fails if an unknown tab finds nothing.
    #[test]
    fn a_name_with_no_tab_is_found_in_any_tab() {
        let found = name_item(&read("Orange Potion", PHRASES), None).unwrap();
        assert_eq!(found.id(), Some(2000001));
    }

    // A price shows only when every fitting item has the same one, and a shown name only when the
    // items share a start. Fails if a guessed price or an empty name ("…") gets through.
    #[test]
    fn no_price_or_name_is_made_up_for_items_that_differ() {
        let item = |id, sell, name| Item {
            id,
            name,
            tab: Tab::Etc,
            sell,
            quest: false,
            untradeable: false,
        };
        let a = item(4000001, Some(1), "Snail Shell");
        let b = item(4000002, Some(2), "Snail Skin");
        let c = item(4000003, None, "Snail Shell");
        let d = item(4000004, Some(1), "Tablecloth");
        let pair = |x: &Item, y: &Item| ItemMatch {
            items: vec![
                Box::leak(Box::new(x.clone())),
                Box::leak(Box::new(y.clone())),
            ],
            name: None,
        };
        assert_eq!(pair(&a, &b).sell(), None, "prices differ");
        assert_eq!(pair(&a, &c).sell(), None, "one has no price");
        assert_eq!(shared_name(&[&a, &b]).as_deref(), Some("Snail S…"));
        assert_eq!(shared_name(&[&a, &d]), None);
        assert_eq!(shared_name(&[]), None);
    }

    // Two items the game gives one name are one name with no single ID.
    #[test]
    fn two_items_with_one_name_share_it() {
        let potion = found("Red Potion", Tab::Use).unwrap();
        assert_eq!(potion.items.len(), 2);
        assert_eq!(
            (potion.id(), potion.name.as_deref(), potion.sell()),
            (None, Some("Red Potion"), Some(5))
        );
    }

    // A speck over one letter makes it a shape of its own that the reader doesn't know, and the
    // name must still fit: the recordings had one such read of Jr. Necki Skin.
    #[test]
    fn a_speck_over_a_letter_doesnt_stop_the_match() {
        let mut letters = read("Jr. Necki Skin", PHRASES);
        let e = letters.iter().position(|l| l.known == Some('e')).unwrap();
        // A shape number no other letter of the name has.
        letters[e] = NameLetter {
            known: None,
            shape: 11,
        };
        let found = name_item(&letters, Some(Tab::Etc)).unwrap();
        assert_eq!(found.id(), Some(4000018));
    }

    // Each rule that rules a name out. Fails if a name that can't be the item fits it.
    #[test]
    fn names_that_cant_be_the_item_dont_fit() {
        // In another tab.
        assert_eq!(found("Orange Potion", Tab::Etc), None);
        // A known letter that isn't the item's.
        let mut letters = read("Snail Shell", PHRASES);
        letters[1].known = Some('x');
        assert_eq!(name_item(&letters, Some(Tab::Etc)), None);
        // Letters drawn alike that would be two letters: S and n as one shape.
        let mut letters = read("Snail Shell", PHRASES);
        letters[5].shape = letters[1].shape;
        assert_eq!(name_item(&letters, Some(Tab::Etc)), None);
        // A name the game data doesn't have, and nothing but the "...".
        assert_eq!(found("Mystery Box", Tab::Etc), None);
        assert_eq!(found("...", Tab::Etc), None);
        // A whole name too short for the item, and a cut one with nothing past what was shown.
        assert_eq!(found("Snail Shel", Tab::Etc), None);
        assert_eq!(found("Snail Shell...", Tab::Etc), None);
    }

    // Two touching letters read as one piece can make a name fit another item exactly: Red
    // Bandana with n and d touching fits Red Katana. Fails if that wrong item is named as certain,
    // with its price 36 times the real one, or if a letter split in two could do the same.
    #[test]
    fn a_misread_letter_doesnt_name_the_wrong_item() {
        let mut letters = read("Red Bandana", PHRASES);
        let n = letters.iter().position(|l| l.known == Some('n')).unwrap();
        letters.splice(
            n..=n + 1,
            [NameLetter {
                known: None,
                shape: 20,
            }],
        );
        let bandana = name_item(&letters, Some(Tab::Equip)).unwrap();
        let ids: Vec<u32> = bandana.items.iter().map(|i| i.id).collect();
        assert!(ids.contains(&1302018) && ids.contains(&1002012), "{ids:?}");
        assert_eq!((bandana.id(), bandana.sell()), (None, None));

        // Read right, Red Willow fits only itself, but its two l's as one split letter would be
        // Red Diros.
        let willow = found("Red Willow", Tab::Equip).unwrap();
        let names: Vec<&str> = willow.items.iter().map(|i| i.name).collect();
        assert!(
            names.contains(&"Red Willow") && names.contains(&"Red Diros"),
            "{names:?}"
        );
        assert_eq!(willow.id(), None);
    }

    // Items whose names start differently still make a match, so a plugin can tell them from an
    // item the data doesn't have. Fails if they come back as no match.
    #[test]
    fn items_with_different_names_still_match() {
        let stars = found("Ilbi Throwing Stars", Tab::Use).unwrap();
        assert!(stars.items.len() > 1, "{:?}", stars.items);
        assert!(stars.items.iter().any(|i| i.id == 2070006));
        assert_eq!((stars.id(), stars.name.as_deref()), (None, None));
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
