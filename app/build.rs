fn main() {
    // The panel, the mini-HUD, the chat overlay and the price label may call only these
    // commands, and only as capabilities/ allows each window.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "app_info",
            "get_settings",
            "set_safe_mode",
            "retry_capture",
            "watch_capture",
            "open_privacy_settings",
            "plugin_storage_get",
            "plugin_storage_set",
            "game_text",
            "set_display",
            "share_summary",
            "hud_summary",
            "chat_history",
            "set_chat_overlay",
            "get_startup",
            "set_startup",
            "update_status",
            "check_for_updates",
            "install_update",
            "price_status",
            "price_search",
            "price_item",
            "price_rows",
            "price_movers",
            "price_recent",
            "set_price_recent",
            "set_price_world",
            "set_price_label",
            "teach_tooltip_name",
            "open_price_page",
        ]),
    ))
    .expect("tauri build script");
}
