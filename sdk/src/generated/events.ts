// Generated from engine/src/events.rs by its tests. Don't edit: change the Rust and run
// `MESOVIBE_UPDATE_SDK=1 cargo test -p mesovibe-engine events`.

/** The event schema version these types describe. */
export const SCHEMA_VERSION = 6;

/**
 * Events the engine emits after a confident read (below the confidence
 * threshold a recognizer reports "unreadable" and no value event is sent).
 *
 * As JSON, an event is an object whose `type` names the variant, with its fields beside it.
 *
 * The events of one engine tick always come in this order: the EXP bar's (`ExpChanged`, then
 * `LevelUp`), the pickup notices' (`MesosGained`, `ItemGained`, `PartyBonus`, `NoticesMissed`),
 * `MapChanged`, then `HpMpChanged`. So a plugin comparing two HP and MP readings sees a level-up
 * or a map change the engine read between them first. The numbers can read while the EXP field
 * can't (a cursor over it): a level-up then shows as a higher max HP or MP before `LevelUp`.
 */
export type Event = { "type": "ExpChanged", level: number | null, exp: number, pct_hundredths: number, needed: number | null, } | { "type": "LevelUp", level: number, } | { "type": "MesosGained", amount: number, late: boolean, shaped: boolean, } | { "type": "ItemGained", item_key: string, count: number, tab: "Equip" | "Use" | "Setup" | "Etc" | "Cash" | null, item_id: number | null, name: string | null, sell: number | null, sell_min: number | null, sell_max: number | null, } | { "type": "PartyBonus", amount: number, } | { "type": "MapChanged", map_key: string | null, } | { "type": "NoticesMissed", reason: "unaligned" | "unreadable" | "not_learned" | "exp_short" | "exp_over" | "lost" | "dropped" | "shapes_wrong", } | { "type": "HpMpChanged", hp: number, max_hp: number, mp: number, max_mp: number, continuous: boolean, } | { "type": "Unreadable", recognizer: string, };


/**
 * The pickup notices' state. In status messages it's the state's snake_case name.
 */
export type NoticeState = "off" | "learning" | "reading" | "covered" | "blocked" | "not_found";


/**
 * The HP and MP numbers' state. In status messages it's the state's snake_case name.
 */
export type HpMpState = "off" | "learning" | "reading" | "covered" | "not_found" | "missing_digits";
