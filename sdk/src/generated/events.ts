// Generated from engine/src/events.rs by its tests. Don't edit: change the Rust and run
// `MESOVIBE_UPDATE_SDK=1 cargo test -p mesovibe-engine events`.

/** The event schema version these types describe. */
export const SCHEMA_VERSION = 3;

/**
 * Events the engine emits after a confident read (below the confidence
 * threshold a recognizer reports "unreadable" and no value event is sent).
 *
 * As JSON, an event is an object whose `type` names the variant, with its fields beside it.
 */
export type Event = { "type": "ExpChanged", level: number | null, exp: number, pct_hundredths: number, needed: number | null, } | { "type": "LevelUp", level: number, } | { "type": "MesosGained", amount: number, late: boolean, shaped: boolean, } | { "type": "ItemGained", item_key: string, count: number, tab: "Equip" | "Use" | "Setup" | "Etc" | "Cash" | null, } | { "type": "PartyBonus", amount: number, } | { "type": "MapChanged", map_key: string | null, } | { "type": "NoticesMissed", reason: "unaligned" | "unreadable" | "not_learned" | "exp_short" | "exp_over" | "lost" | "dropped" | "shapes_wrong", } | { "type": "Unreadable", recognizer: string, };


/**
 * The pickup notices' state. In status messages it's the state's snake_case name.
 */
export type NoticeState = "off" | "learning" | "reading" | "covered" | "blocked" | "not_found";
