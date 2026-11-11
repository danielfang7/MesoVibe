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
export type Event = { "type": "ExpChanged", level: number | null, exp: number, pct_hundredths: number, needed: number | null, } | { "type": "LevelUp", level: number, } | { "type": "MesosGained", amount: number, late: boolean, } | { "type": "ItemGained", item: string, count: number, tab: string | null, } | { "type": "MapChanged", map: string | null, } | { "type": "NoticesMissed", reason: string, } | { "type": "Unreadable", recognizer: string, };
