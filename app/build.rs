fn main() {
    // The panel, the mini-HUD and the chat overlay may call only these commands, and only as
    // capabilities/ allows each window.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "app_info",
            "get_settings",
            "accept_disclosure",
            "set_safe_mode",
            "retry_capture",
            "watch_capture",
            "open_privacy_settings",
            "plugin_storage_get",
            "plugin_storage_set",
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
        ]),
    ))
    .expect("tauri build script");
}
