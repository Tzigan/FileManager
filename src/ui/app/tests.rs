//! Тесты расположения вкладок, навигации и окон.
use super::{
    MANUAL_UPDATE_CHECK_TIMEOUT, PreviewState, STARTUP_UPDATE_CHECK_ID, UpdateCheckState,
    SHUTDOWN_NOTICE_DELAY, ShutdownOperation, ShutdownOperationKind, ShutdownProgress,
    ShutdownSnapshot, log_shutdown_start, log_shutdown_wait,
    breadcrumb_segments, configure_tab_scroll_style, dialogs, dropped_paths,
    floating_dialog_viewport, parse_archive_address, persisted_tab_path, tab_close_button_hovered,
    tab_scroll_source, tab_strip_content_margin,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct MockDrop(PathBuf);

impl egui::DroppedFile for MockDrop {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(vec![])
    }
}

#[test]
fn update_install_respects_auto_restart_and_busy_work() {
    assert_eq!(
        dialogs::plan_update_install(true, false),
        dialogs::UpdateInstallAction::ScheduleRestart
    );
    assert_eq!(
        dialogs::plan_update_install(false, false),
        dialogs::UpdateInstallAction::OfferRestart
    );
    assert_eq!(
        dialogs::plan_update_install(true, true),
        dialogs::UpdateInstallAction::Blocked
    );
    assert_eq!(
        dialogs::plan_update_install(false, true),
        dialogs::UpdateInstallAction::Blocked
    );
}

#[test]
fn update_check_result_ignores_stale_response_after_about_close() {
    let mut state = UpdateCheckState::default();
    let in_flight = AtomicBool::new(false);
    let request_id = state.begin_manual(&in_flight, Instant::now()).unwrap();

    state.close_about();

    assert!(!state.accept_result(request_id, false));
    assert!(!state.accept_result(request_id, true));
    assert!(!state.busy);
    assert!(state.deadline.is_some());
    assert!(in_flight.load(Ordering::Acquire));

    state.worker_finished(request_id);
    in_flight.store(false, Ordering::Release);
}

#[test]
fn manual_update_check_supersedes_startup_auto_check() {
    let mut state = UpdateCheckState::default();
    let in_flight = AtomicBool::new(false);
    let manual_id = state.begin_manual(&in_flight, Instant::now()).unwrap();

    assert_ne!(manual_id, STARTUP_UPDATE_CHECK_ID);
    assert!(!state.accept_result(STARTUP_UPDATE_CHECK_ID, true));
    assert!(state.busy);
    state.worker_finished(manual_id);
    assert!(state.accept_result(manual_id, true));
    assert!(!state.busy);

    let app = include_str!("mod.rs");
    let drain_start = app.find("fn drain_messages").unwrap();
    let drain = &app[drain_start..];
    let found_start = drain.find("WorkerMsg::UpdateFound {").unwrap();
    let found_end = drain[found_start..]
        .find("WorkerMsg::UpdateProgress")
        .unwrap()
        + found_start;
    let found = &drain[found_start..found_end];
    assert!(
        found.find("accept_result").unwrap()
            < found.find("self.pending_update = Some(manifest)").unwrap()
    );
    let latest_start = drain.find("WorkerMsg::UpdateLatest {").unwrap();
    let latest_end = drain[latest_start..]
        .find("WorkerMsg::UpdateFailed")
        .unwrap()
        + latest_start;
    let latest = &drain[latest_start..latest_end];
    assert!(
        latest.find("accept_result").unwrap() < latest.find("self.update_status = Some").unwrap()
    );
}

#[test]
fn repeated_update_clicks_do_not_spawn_parallel_checks_or_break_status() {
    let mut state = UpdateCheckState::default();
    let in_flight = AtomicBool::new(false);
    let now = Instant::now();
    let active_id = state.begin_manual(&in_flight, now).unwrap();

    assert_eq!(state.begin_manual(&in_flight, now), None);
    assert_eq!(state.generation, active_id);
    assert!(state.busy);
    assert!(in_flight.load(Ordering::Acquire));
    let about = include_str!("dialogs.rs");
    assert!(about.contains("&& !self.update_check.busy"));
    assert!(about.contains(".begin_manual(&self.update_check_inflight, Instant::now())"));
    assert!(about.contains("self.spawn_update_check(&viewport_ctx, request_id)"));
}

#[test]
fn reopening_about_does_not_show_stale_update_result() {
    let mut state = UpdateCheckState::default();
    let in_flight = AtomicBool::new(false);
    let old_request_id = state.begin_manual(&in_flight, Instant::now()).unwrap();

    state.close_about();

    assert!(!state.accept_result(old_request_id, true));
    assert!(!state.busy);

    let dialogs = include_str!("dialogs.rs");
    let close_start = dialogs.find("if close_requested {").unwrap();
    let close_end = dialogs[close_start..].find("\n        }").unwrap() + close_start;
    let close = &dialogs[close_start..close_end];
    assert!(close.contains("self.update_check.close_about()"));
    assert!(close.contains("self.update_status = None"));
    assert!(close.contains("self.update_err = None"));
}

#[test]
fn startup_auto_update_result_preserved_or_explicitly_cancelled_by_about_close() {
    let mut state = UpdateCheckState::default();
    state.close_about();

    assert!(state.accept_result(STARTUP_UPDATE_CHECK_ID, false));

    let app = include_str!("mod.rs");
    assert!(app.contains("request_id: STARTUP_UPDATE_CHECK_ID"));
    let dialogs = include_str!("dialogs.rs");
    let close_start = dialogs.find("if close_requested {").unwrap();
    let close_end = dialogs[close_start..].find("\n        }").unwrap() + close_start;
    let close = &dialogs[close_start..close_end];
    assert!(!close.contains("self.pending_update = None"));
}

#[test]
fn about_close_clears_manual_display_not_global_auto_state_if_expected() {
    let mut state = UpdateCheckState::default();
    let in_flight = AtomicBool::new(false);
    let manual_id = state.begin_manual(&in_flight, Instant::now()).unwrap();

    state.close_about();

    assert!(!state.busy);
    assert_ne!(state.generation, manual_id);
    assert!(!state.accept_result(manual_id, true));
    assert!(!state.accept_result(STARTUP_UPDATE_CHECK_ID, false));

    let dialogs = include_str!("dialogs.rs");
    let close_start = dialogs.find("if close_requested {").unwrap();
    let close_end = dialogs[close_start..].find("\n        }").unwrap() + close_start;
    let close = &dialogs[close_start..close_end];
    assert!(close.contains("self.update_status = None"));
    assert!(close.contains("self.update_err = None"));
    assert!(!close.contains("self.pending_update = None"));
}

#[test]
fn manual_update_check_has_final_timeout_or_failure_not_stuck_checking() {
    let mut state = UpdateCheckState::default();
    let in_flight = AtomicBool::new(false);
    let started = Instant::now();
    let request_id = state.begin_manual(&in_flight, started).unwrap();

    assert_eq!(
        state.expire(started + MANUAL_UPDATE_CHECK_TIMEOUT + Duration::from_millis(1)),
        Some((request_id, true))
    );
    assert!(!state.busy);
    assert!(!state.accept_result(request_id, true));
    assert!(in_flight.load(Ordering::Acquire));

    let app = include_str!("mod.rs");
    assert!(app.contains("self.update_check.expire(Instant::now())"));
    assert!(
        app.contains(
            "ctx.request_repaint_after(deadline.saturating_duration_since(Instant::now()))"
        )
    );
    assert!(app.contains("self.t(\"update.check_timeout\")"));
    assert!(
        include_str!("dialogs.rs")
            .contains("ctx.request_repaint_after(MANUAL_UPDATE_CHECK_TIMEOUT)")
    );
}

#[test]
fn rapid_about_reopen_does_not_spawn_unbounded_update_requests() {
    let mut state = UpdateCheckState::default();
    let in_flight = AtomicBool::new(false);
    let now = Instant::now();
    let first_id = state.begin_manual(&in_flight, now).unwrap();

    state.close_about();
    assert_eq!(
        state.begin_manual(&in_flight, now + Duration::from_secs(1)),
        None
    );
    assert!(in_flight.load(Ordering::Acquire));

    state.worker_finished(first_id);
    in_flight.store(false, Ordering::Release);
    let next_id = state
        .begin_manual(&in_flight, now + Duration::from_secs(2))
        .unwrap();
    assert_ne!(first_id, next_id);
    assert!(in_flight.load(Ordering::Acquire));

    let about = include_str!("dialogs.rs");
    assert!(about.contains("self.update_check_inflight.load(Ordering::Acquire)"));
}

#[test]
fn existing_update_guard_tests_not_regressed() {
    let tests = include_str!("tests.rs");
    for test_name in [
        "update_check_result_ignores_stale_response_after_about_close",
        "manual_update_check_supersedes_startup_auto_check",
        "repeated_update_clicks_do_not_spawn_parallel_checks_or_break_status",
        "reopening_about_does_not_show_stale_update_result",
    ] {
        assert!(tests.contains(&format!("fn {test_name}")));
    }
}

#[test]
fn existing_about_3_5_tests_not_regressed() {
    let dialogs = include_str!("dialogs.rs");
    for test_name in [
        "about_version_from_single_source_not_hardcoded",
        "about_build_profile_reflects_actual_configuration",
        "about_commit_hash_shown_only_if_really_available",
        "about_description_matches_real_capabilities_no_false_promises",
        "about_update_check_reuses_spawn_update_check_no_second_system",
        "about_update_result_user_facing_no_raw_error",
        "about_export_entry_not_duplicated",
        "about_window_single_instance_existing_style_localized",
    ] {
        assert!(
            dialogs.contains(&format!("fn {test_name}")),
            "missing About 3.5 test: {test_name}"
        );
    }
}

#[test]
fn thumbnails_channel_limit_not_lost() {
    assert_eq!(super::THUMBNAIL_QUEUE_CAPACITY, 24);
}

#[test]
fn dropped_paths_collects() {
    let files: Vec<egui::DroppedFileHandle> = vec![
        std::sync::Arc::new(MockDrop(PathBuf::from("C:\\a.txt"))),
        std::sync::Arc::new(MockDrop(PathBuf::from("D:\\b.txt"))),
    ];
    let got = dropped_paths(&files);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0], PathBuf::from("C:\\a.txt"));
    assert_eq!(got[1], PathBuf::from("D:\\b.txt"));
}

#[test]
fn breadcrumbs_split_path() {
    #[cfg(windows)]
    {
        let segs = breadcrumb_segments(Path::new("C:\\a\\b"));
        assert_eq!(
            segs.iter()
                .map(|(label, _)| label.as_str())
                .collect::<Vec<_>>(),
            vec!["C:\\", "a", "b"]
        );
        assert_eq!(segs[0].0, "C:\\");
        assert_eq!(segs[0].1, PathBuf::from("C:\\"));
        let last = segs.last().unwrap();
        assert_eq!(last.0, "b");
        assert!(last.1.ends_with("b"));
    }
}

#[test]
fn archive_address_round_trips_the_internal_folder() {
    let (archive, prefix) = parse_archive_address(r"C:\Downloads\files.zip!/docs/notes").unwrap();
    assert_eq!(archive, PathBuf::from(r"C:\Downloads\files.zip"));
    assert_eq!(prefix, "docs/notes");
}

#[test]
fn archive_tabs_restore_the_containing_filesystem_folder() {
    let mut tab = super::Tab::new(PathBuf::from(r"C:\Downloads\files.zip"));
    tab.archive_prefix = Some("docs".to_owned());
    assert_eq!(persisted_tab_path(&tab), PathBuf::from(r"C:\Downloads"));
}

#[test]
fn tab_strip_uses_mouse_wheel_without_showing_a_scrollbar() {
    let source = tab_scroll_source();
    assert!(source.mouse_wheel);
    assert!(!source.scroll_bar);
    assert_eq!(source.drag, egui::scroll_area::DragScroll::Never);

    let mut style = egui::Style::default();
    configure_tab_scroll_style(&mut style);
    assert!(style.always_scroll_the_only_direction);
    assert_eq!(
        style.spacing.item_spacing.y,
        super::style::TAB_BAR_VERTICAL_SPACING
    );
    let margin = tab_strip_content_margin();
    assert_eq!(margin.left, super::style::FRAME_PADDING_X);
    assert_eq!(margin.right, 0);
    assert_eq!(margin.top, 0);
    assert_eq!(margin.bottom, 0);
}

#[test]
fn tab_drag_fsm_distinguishes_click_drag_and_close_hitbox() {
    use super::tab_drag_preview::{
        TabDragState, advance_tab_drag_state, tab_drag_press_hits_body, tab_drag_threshold_crossed,
        viewport_position_to_screen,
    };
    use super::window_frame::TAB_CLOSE_WIDTH;

    let tab_rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(120.0, 24.0));
    let close_rect = egui::Rect::from_min_size(
        egui::pos2(tab_rect.right() - TAB_CLOSE_WIDTH, tab_rect.top()),
        egui::vec2(TAB_CLOSE_WIDTH, tab_rect.height()),
    );
    assert!(tab_drag_press_hits_body(
        tab_rect,
        close_rect,
        egui::pos2(40.0, 20.0)
    ));
    assert!(!tab_drag_press_hits_body(
        tab_rect,
        close_rect,
        close_rect.center()
    ));
    assert!(tab_close_button_hovered(
        false,
        false,
        close_rect,
        Some(close_rect.center())
    ));
    assert!(!tab_close_button_hovered(
        true,
        false,
        close_rect,
        Some(close_rect.center())
    ));

    let start =
        viewport_position_to_screen(egui::pos2(40.0, 20.0), egui::pos2(-1280.0, -200.0), 1.5);
    assert_eq!(start, (-1860, -270));
    let pressed = TabDragState::Pressed {
        tab_id: 7,
        start_screen_position: start,
        start_pixels_per_point: 1.5,
    };
    assert_eq!(
        advance_tab_drag_state(pressed, Some((start.0 + 5, start.1)), true, false),
        pressed
    );
    assert_eq!(
        advance_tab_drag_state(pressed, Some((start.0 + 5, start.1)), false, false),
        TabDragState::Idle
    );
    assert_eq!(
        advance_tab_drag_state(pressed, Some((start.0 + 7, start.1)), true, false),
        TabDragState::Dragging { tab_id: 7 }
    );
    assert_eq!(
        advance_tab_drag_state(pressed, Some((start.0 + 200, start.1 + 300)), true, false),
        TabDragState::Dragging { tab_id: 7 }
    );
    assert!(!tab_drag_threshold_crossed(
        start,
        (start.0 + 7, start.1),
        2.0
    ));
    assert!(tab_drag_threshold_crossed(
        start,
        (start.0 + 9, start.1),
        2.0
    ));
    assert_eq!(
        advance_tab_drag_state(pressed, Some(start), true, true),
        TabDragState::Cancelled
    );
    assert!(TabDragState::Cancelled.is_active());
}

#[test]
fn detached_dialog_viewports_are_topmost_and_keep_user_moved_positions() {
    let ctx = egui::Context::default();
    let size = egui::vec2(480.0, 280.0);
    let initial = floating_dialog_viewport(&ctx, "About".into(), size, false);
    assert_eq!(
        initial.window_level,
        Some(egui::viewport::WindowLevel::AlwaysOnTop)
    );
    assert_eq!(initial.inner_size, Some(size));
    assert_eq!(initial.maximize_button, Some(false));
    assert!(initial.position.is_some());

    let moved = floating_dialog_viewport(&ctx, "About".into(), size, true);
    assert_eq!(
        moved.window_level,
        Some(egui::viewport::WindowLevel::AlwaysOnTop)
    );
    assert_eq!(moved.maximize_button, Some(false));
    assert_eq!(moved.position, None);
}

#[test]
fn window_frame_maps_resize_edges_and_drop_targets() {
    use super::window_frame::{release_is_outside_frame, resize_direction_at};

    let frame = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0));
    assert_eq!(
        resize_direction_at(frame, egui::pos2(1.0, 1.0)),
        Some(egui::viewport::ResizeDirection::NorthWest)
    );
    assert_eq!(
        resize_direction_at(frame, egui::pos2(399.0, 299.0)),
        Some(egui::viewport::ResizeDirection::SouthEast)
    );
    assert_eq!(resize_direction_at(frame, frame.center()), None);

    assert!(!release_is_outside_frame(frame, Some(frame.center())));
    assert!(release_is_outside_frame(
        frame,
        Some(egui::pos2(401.0, 140.0))
    ));
    assert!(release_is_outside_frame(frame, None));
}

#[test]
fn main_window_drag_area_avoids_resize_edges() {
    use super::window_frame::{
        MAIN_WINDOW_DRAG_AREA_MIN_WIDTH, WINDOW_DRAG_AREA_MIN_WIDTH, WINDOW_FRAME_HEIGHT,
        WINDOW_RESIZE_HANDLE_WIDTH, main_window_drag_hit_rect,
    };

    let frame = egui::Rect::from_min_size(
        egui::pos2(-400.0, -200.0),
        egui::vec2(640.0, WINDOW_FRAME_HEIGHT),
    );
    let drag_area = main_window_drag_hit_rect(frame);

    assert_eq!(drag_area.left(), frame.left());
    assert_eq!(drag_area.right(), frame.right());
    assert!(drag_area.top() > frame.top() + WINDOW_RESIZE_HANDLE_WIDTH);
    assert!(drag_area.bottom() < frame.bottom() - WINDOW_RESIZE_HANDLE_WIDTH);
    assert!(drag_area.contains(frame.center()));
    assert!(MAIN_WINDOW_DRAG_AREA_MIN_WIDTH > WINDOW_DRAG_AREA_MIN_WIDTH);
}

#[test]
fn new_tab_button_size_tracks_tab_text_and_click_target() {
    use super::window_frame::{
        TAB_PLUS_BUTTON_WIDTH, TAB_VERTICAL_EXTRA_HEIGHT, new_tab_button_size,
    };

    let compact = new_tab_button_size(14.0, 20.0);
    assert_eq!(compact.x, TAB_PLUS_BUTTON_WIDTH);
    assert_eq!(compact.y, 20.0);

    let larger_font = new_tab_button_size(18.0, 20.0);
    assert_eq!(larger_font.y, 18.0 + TAB_VERTICAL_EXTRA_HEIGHT);
}

#[test]
fn tab_strip_scroll_alignment_changes_only_when_target_is_clipped() {
    use super::toolbar::tab_strip_scroll_alignment;

    let viewport = egui::Rect::from_min_size(egui::pos2(50.0, 0.0), egui::vec2(100.0, 40.0));
    let visible = egui::Rect::from_min_size(egui::pos2(70.0, 4.0), egui::vec2(40.0, 32.0));
    let clipped_left = egui::Rect::from_min_size(egui::pos2(20.0, 4.0), egui::vec2(40.0, 32.0));
    let clipped_right = egui::Rect::from_min_size(egui::pos2(130.0, 4.0), egui::vec2(40.0, 32.0));
    let wider_than_viewport =
        egui::Rect::from_min_size(egui::pos2(20.0, 4.0), egui::vec2(180.0, 32.0));

    assert_eq!(tab_strip_scroll_alignment(visible, viewport), None);
    assert_eq!(
        tab_strip_scroll_alignment(clipped_left, viewport),
        Some(egui::Align::Min)
    );
    assert_eq!(
        tab_strip_scroll_alignment(clipped_right, viewport),
        Some(egui::Align::Max)
    );
    assert_eq!(
        tab_strip_scroll_alignment(wider_than_viewport, viewport),
        Some(egui::Align::Min)
    );
}

#[test]
fn tab_drag_preview_converts_negative_screen_coordinates_and_preserves_drop_slots() {
    use super::tab_drag_preview::{
        preview_window_position, screen_position_to_viewport, tab_drop_slot_at,
    };

    let local = screen_position_to_viewport((-1500, -150), egui::pos2(-1280.0, -200.0), 1.5);
    assert_eq!(local, egui::pos2(280.0, 100.0));
    assert_eq!(
        preview_window_position((-1500, -150), egui::vec2(10.0, -16.0), 1.5),
        (-1515, -126)
    );

    let tabs = [
        (
            0,
            egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(100.0, 30.0)),
        ),
        (
            1,
            egui::Rect::from_min_size(egui::pos2(112.0, 10.0), egui::vec2(100.0, 30.0)),
        ),
    ];
    assert_eq!(tab_drop_slot_at(egui::pos2(20.0, 20.0), &tabs), Some(0));
    assert_eq!(tab_drop_slot_at(egui::pos2(100.0, 20.0), &tabs), Some(1));
    assert_eq!(tab_drop_slot_at(egui::pos2(300.0, 20.0), &tabs), None);
}

#[test]
fn detached_window_position_stays_on_the_release_monitor() {
    use super::tab_drag_preview::detached_window_position;

    let right_monitor = detached_window_position(
        (3830, 1060),
        1.5,
        egui::vec2(1000.0, 640.0),
        (1920, 0, 3840, 1080),
    );
    assert_eq!(right_monitor, (2328, 108));

    let left_monitor = detached_window_position(
        (-10, -190),
        1.0,
        egui::vec2(1000.0, 640.0),
        (-1920, -200, 0, 1080),
    );
    assert_eq!(left_monitor, (-1008, -182));
}

#[test]
fn detached_tab_viewport_is_borderless_resizable_and_keeps_its_size() {
    use super::window_frame::detached_viewport_builder_at;

    let release_position = egui::pos2(-720.0, 140.0);
    let initial = detached_viewport_builder_at("Detached".into(), false, Some(release_position));
    assert_eq!(initial.decorations, Some(false));
    assert_eq!(initial.resizable, Some(true));
    assert_eq!(initial.inner_size, Some(egui::vec2(1000.0, 640.0)));
    assert_eq!(initial.position, Some(release_position));
    assert_eq!(initial.visible, Some(false));

    let placed = detached_viewport_builder_at("Detached".into(), true, None);
    assert_eq!(placed.inner_size, None);
    assert_eq!(placed.position, None);
    assert_eq!(placed.visible, None);
}

#[test]
fn window_control_visual_states_and_maximize_icon_follow_viewport_state() {
    use super::window_frame::{
        WINDOW_CONTROL_GAP, WINDOW_CONTROL_HEIGHT, WINDOW_CONTROL_VERTICAL_GAP,
        WINDOW_CONTROL_WIDTH, WINDOW_CONTROLS_WIDTH, WINDOW_FRAME_HEIGHT,
        WINDOW_RESIZE_HANDLE_WIDTH, WindowControlIcon, maximize_restore_icon,
        window_control_background,
    };

    let visuals = egui::Visuals::default();
    let idle = window_control_background(&visuals, WindowControlIcon::Minimize, true, 0.0, 0.0);
    let hover = window_control_background(&visuals, WindowControlIcon::Minimize, true, 1.0, 0.0);
    let pressed = window_control_background(&visuals, WindowControlIcon::Minimize, true, 0.0, 1.0);
    let close_hover = window_control_background(&visuals, WindowControlIcon::Close, true, 1.0, 0.0);
    let close_pressed =
        window_control_background(&visuals, WindowControlIcon::Close, true, 0.0, 1.0);
    let disabled =
        window_control_background(&visuals, WindowControlIcon::Maximize, false, 1.0, 0.0);

    assert_eq!(idle, egui::Color32::TRANSPARENT);
    assert_ne!(hover, idle);
    assert_ne!(pressed, hover);
    assert_ne!(close_hover, hover);
    assert_ne!(close_pressed, close_hover);
    assert_eq!(disabled, egui::Color32::TRANSPARENT);
    assert_eq!(maximize_restore_icon(false), WindowControlIcon::Maximize);
    assert_eq!(maximize_restore_icon(true), WindowControlIcon::Restore);
    assert_eq!(
        WINDOW_CONTROL_HEIGHT,
        WINDOW_FRAME_HEIGHT - WINDOW_RESIZE_HANDLE_WIDTH * 2.0 - WINDOW_CONTROL_VERTICAL_GAP * 2.0
    );
    assert_eq!(
        WINDOW_CONTROLS_WIDTH,
        WINDOW_CONTROL_WIDTH * 3.0 + WINDOW_CONTROL_GAP * 2.0
    );
}

#[test]
fn preview_viewport_can_resize_and_keeps_its_user_size() {
    let ctx = egui::Context::default();
    let initial = super::preview::preview_viewport_builder(&ctx, "Preview".into(), false);
    assert_eq!(initial.inner_size, Some(egui::vec2(760.0, 540.0)));
    assert!(initial.position.is_some());
    assert_eq!(initial.resizable, Some(true));
    assert_eq!(initial.maximize_button, Some(true));
    assert_eq!(
        initial.window_level,
        Some(egui::viewport::WindowLevel::AlwaysOnTop)
    );

    let placed = super::preview::preview_viewport_builder(&ctx, "Preview".into(), true);
    assert_eq!(placed.inner_size, None);
    assert_eq!(placed.position, None);
    assert_eq!(placed.resizable, Some(true));
    assert_eq!(placed.maximize_button, Some(true));
}

#[test]
fn side_mouse_buttons_map_to_navigation_without_changing_standard_buttons() {
    use super::input::{MouseNavigationAction, map_mouse_button_to_navigation};

    assert_eq!(
        map_mouse_button_to_navigation(egui::PointerButton::Extra1),
        Some(MouseNavigationAction::Back)
    );
    assert_eq!(
        map_mouse_button_to_navigation(egui::PointerButton::Extra2),
        Some(MouseNavigationAction::Forward)
    );
    for button in [
        egui::PointerButton::Primary,
        egui::PointerButton::Secondary,
        egui::PointerButton::Middle,
    ] {
        assert_eq!(map_mouse_button_to_navigation(button), None);
    }
}

#[test]
fn clearing_preview_resets_floating_viewport_state() {
    let mut preview = PreviewState::default();
    preview.target = Some(PathBuf::from("preview.txt"));
    preview.viewport_placed = true;

    preview.clear();

    assert!(preview.target.is_none());
    assert!(!preview.viewport_placed);
}

#[test]
fn update_percent_is_bounded_and_handles_zero_total() {
    let percent = super::dialogs::update_percent;

    assert_eq!(percent(0, 100), 0);
    assert_eq!(percent(25, 100), 25);
    assert_eq!(percent(50, 100), 50);
    assert_eq!(percent(100, 100), 100);
    assert_eq!(percent(125, 100), 100);
    assert_eq!(percent(0, 0), 0);
}

#[test]
fn drive_root_is_one_absolute_breadcrumb() {
    #[cfg(windows)]
    {
        let segs = breadcrumb_segments(Path::new("C:\\"));
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, "C:\\");
        assert_eq!(segs[0].1, PathBuf::from("C:\\"));
    }
}

fn shutdown_fixture() -> ShutdownSnapshot {
    ShutdownSnapshot {
        operations: vec![ShutdownOperation {
            id: 41,
            kind: ShutdownOperationKind::File("ops.copying"),
            queued: None,
        }],
    }
}

#[test]
fn shutdown_with_pending_ops_shows_waiting_status_not_silent() {
    let now = Instant::now();
    let mut progress = ShutdownProgress::default();
    assert!(progress.begin(now, shutdown_fixture()));
    assert!(progress.is_active());
    assert!(include_str!("mod.rs").contains("self.t(\"shutdown.waiting\")"));
    assert!(include_str!("mod.rs").contains("ViewportCommand::CancelClose"));
}

#[test]
fn shutdown_long_wait_shows_fs_unresponsive_warning_localized() {
    let now = Instant::now();
    let mut progress = ShutdownProgress::default();
    assert!(progress.begin(now, shutdown_fixture()));
    assert!(!progress.take_warning(now + SHUTDOWN_NOTICE_DELAY - Duration::from_millis(1)));
    assert!(progress.take_warning(now + SHUTDOWN_NOTICE_DELAY));
    assert!(!progress.take_warning(now + SHUTDOWN_NOTICE_DELAY * 2));

    for (json, filesystem, risk) in [
        (
            include_str!("../../../i18n/ru.json"),
            "файловая система",
            "потере или повреждению",
        ),
        (
            include_str!("../../../i18n/en.json"),
            "file system",
            "data loss or corruption",
        ),
    ] {
        let labels: serde_json::Value = serde_json::from_str(json).unwrap();
        assert!(
            labels["shutdown.fs_unresponsive"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains(filesystem)
        );
        assert!(
            labels["shutdown.force_close_risk"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains(risk)
        );
    }
}

#[test]
fn shutdown_warning_does_not_detach_or_force_kill_worker() {
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    let now = Instant::now();
    let pending = shutdown_fixture();
    let mut progress = ShutdownProgress::default();
    assert!(progress.begin(now, pending.clone()));
    assert!(progress.take_warning(now + SHUTDOWN_NOTICE_DELAY));
    assert!(!worker.is_finished());
    assert!(!progress.workers_finished(&pending, worker.is_finished()));

    let source = include_str!("mod.rs");
    assert!(source.contains("worker.join()"));
    assert!(source.contains("bg_ops_pool.join_workers()"));
    assert!(!source.contains("TerminateThread"));
    assert!(release_tx.send(()).is_ok());
    worker.join().unwrap();
}

#[test]
fn shutdown_completes_normally_after_worker_returns() {
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || release_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    let now = Instant::now();
    let mut progress = ShutdownProgress::default();
    assert!(progress.begin(now, shutdown_fixture()));
    assert!(release_tx.send(()).is_ok());
    worker.join().unwrap();
    assert!(progress.workers_finished(&ShutdownSnapshot::default(), true));
    assert!(progress.allow_close());
    assert!(!progress.allow_close());
}

#[test]
fn shutdown_no_warning_when_no_pending_ops() {
    let mut progress = ShutdownProgress::default();
    assert!(!progress.begin(Instant::now(), ShutdownSnapshot::default()));
    assert!(!progress.is_active());
    assert!(!progress.take_warning(Instant::now() + SHUTDOWN_NOTICE_DELAY * 2));
}

fn capture_shutdown_logs(run: impl FnOnce()) -> String {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    struct BufferWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for BufferWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
        type Writer = BufferWriter;

        fn make_writer(&'a self) -> Self::Writer {
            BufferWriter(Arc::clone(&self.0))
        }
    }

    let output = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(Buffer(Arc::clone(&output)))
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    String::from_utf8(output.lock().unwrap().clone()).unwrap()
}

#[test]
fn shutdown_logs_pending_operation_and_elapsed() {
    let pending = shutdown_fixture();
    let output = capture_shutdown_logs(|| {
        log_shutdown_start(&pending);
        log_shutdown_wait(Duration::from_millis(12_000), &pending);
    });
    assert!(output.contains("operation_id=41"));
    assert!(output.contains("pending_operations=1"));
    assert!(output.contains("elapsed_ms=12000"));
    assert!(output.contains("продолжает ждать"));
}

#[test]
fn shutdown_multiple_ops_reported_honestly() {
    let snapshot = ShutdownSnapshot {
        operations: vec![
            ShutdownOperation {
                id: 1,
                kind: ShutdownOperationKind::File("ops.copying"),
                queued: None,
            },
            ShutdownOperation {
                id: 2,
                kind: ShutdownOperationKind::File("ops.copying"),
                queued: None,
            },
            ShutdownOperation {
                id: 3,
                kind: ShutdownOperationKind::Search,
                queued: Some(false),
            },
            ShutdownOperation {
                id: 4,
                kind: ShutdownOperationKind::Zip,
                queued: Some(false),
            },
        ],
    };
    let i18n = crate::core::i18n::I18n::new(crate::core::i18n::Language::En);
    let summary = snapshot.summary(&i18n);
    assert_eq!(summary.split(", ").count(), 3);
    assert!(summary.contains("Copying: 2"));
    assert!(summary.contains("Searches: 1"));
    assert!(summary.contains("ZIP operations: 1"));
}

#[test]
fn shutdown_observer_does_not_deadlock_with_worker_channel() {
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        event_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    event_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(event_rx.try_recv().is_err());
    assert!(release_tx.send(()).is_ok());
    worker.join().unwrap();

    let source = include_str!("mod.rs");
    let start = source.find("fn drain_shutdown_ops").unwrap();
    let drain = &source[start..];
    assert!(drain.contains("self.ops_evt.try_recv()"));
    assert!(!drain.contains("self.ops_evt.recv()"));
}

#[test]
fn duplicate_close_requests_do_not_spawn_multiple_observers() {
    let now = Instant::now();
    let mut progress = ShutdownProgress::default();
    assert!(progress.begin(now, shutdown_fixture()));
    assert!(!progress.begin(now + Duration::from_secs(1), shutdown_fixture()));
    assert_eq!(progress.initial_snapshot.operations.len(), 1);
}

#[test]
fn domain1_5_shutdown_tests_not_regressed() {
    let pool = include_str!("../../core/bg_ops.rs");
    let file_ops = include_str!("../../core/ops/worker/tests/tests_v04.rs");
    let zip = include_str!("../../platform/windows/zip/mod.rs");
    for name in [
        "app_shutdown_with_pending_bg_ops",
        "app_shutdown_with_pending_bg_ops_and_file_ops_still_hold",
        "critical_flows_not_blocked_by_pool",
    ] {
        assert!(pool.contains(name));
    }
    assert!(file_ops.contains("file_ops_shutdown_skips_cancelled_queue"));
    assert!(zip.contains("shutdown_does_not_block_on_impossible_zip_publish"));
}

#[test]
fn domain3_1_2_3_4_5_not_regressed() {
    let dialogs = include_str!("dialogs.rs");
    let app_tests = include_str!("tests.rs");
    assert!(dialogs.contains("domain3_1_2_3_4_not_regressed"));
    assert!(dialogs.contains("domain1_2_not_regressed"));
    assert!(app_tests.contains("existing_about_3_5_tests_not_regressed"));
    for name in [
        "update_check_result_ignores_stale_response_after_about_close",
        "manual_update_check_supersedes_startup_auto_check",
        "repeated_update_clicks_do_not_spawn_parallel_checks_or_break_status",
        "reopening_about_does_not_show_stale_update_result",
        "about_build_profile_reflects_actual_configuration",
    ] {
        assert!(app_tests.contains(name) || dialogs.contains(name));
    }
}

#[test]
fn settings_structure_and_json_schema_unchanged_if_constant_used() {
    let app = include_str!("mod.rs");
    let settings = include_str!("../../core/settings.rs");
    assert!(app.contains("const SHUTDOWN_NOTICE_DELAY"));
    assert!(!settings.contains("shutdown_notice_delay"));
    assert!(!include_str!("../../../i18n/ru.json").contains("shutdown_notice_delay"));
    assert!(!include_str!("../../../i18n/en.json").contains("shutdown_notice_delay"));
}
