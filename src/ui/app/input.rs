//! Клавиатурный ввод и перетаскивание файлов.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShortcutId {
    Copy,
    Cut,
    Paste,
    PreviousTab,
    NewTab,
    CloseTab,
    NextTab,
    Delete,
    Properties,
    Rename,
    Preview,
    SelectAll,
    Parent,
    Open,
    Back,
    Forward,
    MoveDown,
    MoveUp,
    PageDown,
    PageUp,
    First,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShortcutModifiers {
    Any,
    Ctrl,
    CtrlShift,
    Alt,
    Command,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HotkeyBinding {
    pub(super) id: ShortcutId,
    pub(super) key: egui::Key,
    pub(super) modifiers: ShortcutModifiers,
    pub(super) key_label_key: &'static str,
    pub(super) action_key: &'static str,
    pub(super) condition_key: Option<&'static str>,
    pub(super) shift_action_key: Option<&'static str>,
}

pub(super) struct HotkeyGesture {
    pub(super) gesture_key: &'static str,
    pub(super) action_key: &'static str,
}

const HOTKEY_BINDINGS: &[HotkeyBinding] = &[
    HotkeyBinding {
        id: ShortcutId::Copy,
        key: egui::Key::C,
        modifiers: ShortcutModifiers::Ctrl,
        key_label_key: "help.hotkeys.key.c",
        action_key: "help.hotkeys.action.copy",
        condition_key: Some("help.hotkeys.condition.clipboard"),
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::Cut,
        key: egui::Key::X,
        modifiers: ShortcutModifiers::Ctrl,
        key_label_key: "help.hotkeys.key.x",
        action_key: "help.hotkeys.action.cut",
        condition_key: Some("help.hotkeys.condition.clipboard"),
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::Paste,
        key: egui::Key::V,
        modifiers: ShortcutModifiers::Ctrl,
        key_label_key: "help.hotkeys.key.v",
        action_key: "help.hotkeys.action.paste",
        condition_key: Some("help.hotkeys.condition.clipboard"),
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::PreviousTab,
        key: egui::Key::Tab,
        modifiers: ShortcutModifiers::CtrlShift,
        key_label_key: "help.hotkeys.key.tab",
        action_key: "help.hotkeys.action.previous_tab",
        condition_key: Some("help.hotkeys.condition.tabs"),
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::NewTab,
        key: egui::Key::T,
        modifiers: ShortcutModifiers::Ctrl,
        key_label_key: "help.hotkeys.key.t",
        action_key: "help.hotkeys.action.new_tab",
        condition_key: Some("help.hotkeys.condition.tabs"),
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::CloseTab,
        key: egui::Key::W,
        modifiers: ShortcutModifiers::Ctrl,
        key_label_key: "help.hotkeys.key.w",
        action_key: "help.hotkeys.action.close_tab",
        condition_key: Some("help.hotkeys.condition.tabs"),
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::NextTab,
        key: egui::Key::Tab,
        modifiers: ShortcutModifiers::Ctrl,
        key_label_key: "help.hotkeys.key.tab",
        action_key: "help.hotkeys.action.next_tab",
        condition_key: Some("help.hotkeys.condition.tabs"),
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::Delete,
        key: egui::Key::Delete,
        modifiers: ShortcutModifiers::Any,
        key_label_key: "help.hotkeys.key.delete",
        action_key: "help.hotkeys.action.delete",
        condition_key: None,
        shift_action_key: Some("help.hotkeys.action.delete_permanent"),
    },
    HotkeyBinding {
        id: ShortcutId::Properties,
        key: egui::Key::Enter,
        modifiers: ShortcutModifiers::Alt,
        key_label_key: "help.hotkeys.key.enter",
        action_key: "help.hotkeys.action.properties",
        condition_key: None,
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::Rename,
        key: egui::Key::F2,
        modifiers: ShortcutModifiers::Any,
        key_label_key: "help.hotkeys.key.f2",
        action_key: "help.hotkeys.action.rename",
        condition_key: None,
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::Preview,
        key: egui::Key::F3,
        modifiers: ShortcutModifiers::Any,
        key_label_key: "help.hotkeys.key.f3",
        action_key: "help.hotkeys.action.preview",
        condition_key: None,
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::SelectAll,
        key: egui::Key::A,
        modifiers: ShortcutModifiers::Command,
        key_label_key: "help.hotkeys.key.a",
        action_key: "help.hotkeys.action.select_all",
        condition_key: None,
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::Parent,
        key: egui::Key::Backspace,
        modifiers: ShortcutModifiers::Any,
        key_label_key: "help.hotkeys.key.backspace",
        action_key: "help.hotkeys.action.parent",
        condition_key: None,
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::Open,
        key: egui::Key::Enter,
        modifiers: ShortcutModifiers::Any,
        key_label_key: "help.hotkeys.key.enter",
        action_key: "help.hotkeys.action.open",
        condition_key: None,
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::Back,
        key: egui::Key::ArrowLeft,
        modifiers: ShortcutModifiers::Alt,
        key_label_key: "help.hotkeys.key.left",
        action_key: "help.hotkeys.action.back",
        condition_key: None,
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::Forward,
        key: egui::Key::ArrowRight,
        modifiers: ShortcutModifiers::Alt,
        key_label_key: "help.hotkeys.key.right",
        action_key: "help.hotkeys.action.forward",
        condition_key: None,
        shift_action_key: None,
    },
    HotkeyBinding {
        id: ShortcutId::MoveDown,
        key: egui::Key::ArrowDown,
        modifiers: ShortcutModifiers::Any,
        key_label_key: "help.hotkeys.key.down",
        action_key: "help.hotkeys.action.move_cursor",
        condition_key: None,
        shift_action_key: Some("help.hotkeys.action.extend_selection"),
    },
    HotkeyBinding {
        id: ShortcutId::MoveUp,
        key: egui::Key::ArrowUp,
        modifiers: ShortcutModifiers::Any,
        key_label_key: "help.hotkeys.key.up",
        action_key: "help.hotkeys.action.move_cursor",
        condition_key: None,
        shift_action_key: Some("help.hotkeys.action.extend_selection"),
    },
    HotkeyBinding {
        id: ShortcutId::PageDown,
        key: egui::Key::PageDown,
        modifiers: ShortcutModifiers::Any,
        key_label_key: "help.hotkeys.key.page_down",
        action_key: "help.hotkeys.action.move_page",
        condition_key: None,
        shift_action_key: Some("help.hotkeys.action.extend_selection"),
    },
    HotkeyBinding {
        id: ShortcutId::PageUp,
        key: egui::Key::PageUp,
        modifiers: ShortcutModifiers::Any,
        key_label_key: "help.hotkeys.key.page_up",
        action_key: "help.hotkeys.action.move_page",
        condition_key: None,
        shift_action_key: Some("help.hotkeys.action.extend_selection"),
    },
    HotkeyBinding {
        id: ShortcutId::First,
        key: egui::Key::Home,
        modifiers: ShortcutModifiers::Any,
        key_label_key: "help.hotkeys.key.home",
        action_key: "help.hotkeys.action.first",
        condition_key: None,
        shift_action_key: Some("help.hotkeys.action.extend_selection"),
    },
    HotkeyBinding {
        id: ShortcutId::Last,
        key: egui::Key::End,
        modifiers: ShortcutModifiers::Any,
        key_label_key: "help.hotkeys.key.end",
        action_key: "help.hotkeys.action.last",
        condition_key: None,
        shift_action_key: Some("help.hotkeys.action.extend_selection"),
    },
];

const HOTKEY_DND_GESTURES: &[HotkeyGesture] = &[
    HotkeyGesture {
        gesture_key: "help.hotkeys.gesture.drag",
        action_key: "help.hotkeys.action.dnd.drag",
    },
    HotkeyGesture {
        gesture_key: "help.hotkeys.gesture.ctrl_drag",
        action_key: "help.hotkeys.action.dnd.ctrl",
    },
    HotkeyGesture {
        gesture_key: "help.hotkeys.gesture.shift_drag",
        action_key: "help.hotkeys.action.dnd.shift",
    },
    HotkeyGesture {
        gesture_key: "help.hotkeys.gesture.escape_drag",
        action_key: "help.hotkeys.action.dnd.escape",
    },
    HotkeyGesture {
        gesture_key: "help.hotkeys.gesture.tab_drop",
        action_key: "help.hotkeys.action.dnd.tab",
    },
    HotkeyGesture {
        gesture_key: "help.hotkeys.gesture.bookmark_drop",
        action_key: "help.hotkeys.action.dnd.bookmark",
    },
];

pub(super) fn hotkey_bindings() -> &'static [HotkeyBinding] {
    HOTKEY_BINDINGS
}

pub(super) fn hotkey_dnd_gestures() -> &'static [HotkeyGesture] {
    HOTKEY_DND_GESTURES
}

fn hotkey_binding(id: ShortcutId) -> Option<&'static HotkeyBinding> {
    HOTKEY_BINDINGS.iter().find(|binding| binding.id == id)
}

fn shortcut_pressed(ctx: &egui::Context, id: ShortcutId) -> bool {
    let Some(binding) = hotkey_binding(id) else {
        return false;
    };
    let (pressed, modifiers) = ctx.input(|input| (input.key_pressed(binding.key), input.modifiers));
    pressed
        && match binding.modifiers {
            ShortcutModifiers::Any => true,
            ShortcutModifiers::Ctrl => modifiers.ctrl,
            ShortcutModifiers::CtrlShift => modifiers.ctrl && modifiers.shift,
            ShortcutModifiers::Alt => modifiers.alt,
            ShortcutModifiers::Command => modifiers.command,
        }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MouseNavigationAction {
    Back,
    Forward,
}

pub(super) fn map_mouse_button_to_navigation(
    button: egui::PointerButton,
) -> Option<MouseNavigationAction> {
    // В egui боковые кнопки обычно приходят как Extra1/Extra2; точное соответствие зависит от платформы и библиотеки.
    match button {
        egui::PointerButton::Extra1 => Some(MouseNavigationAction::Back),
        egui::PointerButton::Extra2 => Some(MouseNavigationAction::Forward),
        _ => None,
    }
}

impl FileManagerApp {
    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
        self.handle_keys_impl(ctx, true);
    }

    pub(super) fn handle_detached_keys(&mut self, ctx: &egui::Context) {
        self.handle_keys_impl(ctx, false);
    }

    fn handle_keys_impl(&mut self, ctx: &egui::Context, allow_tab_management: bool) {
        if self.shutdown_active() {
            return;
        }
        self.handle_side_mouse_navigation(ctx);
        let text_edit_focused = ctx.text_edit_focused();
        let mods = ctx.input(|i| i.modifiers);

        // Prefer file operations when a file selection/internal file clipboard
        // exists. This also recovers if the address TextEdit retained focus after
        // the user clicked a file row; without a file selection normal text-edit
        // shortcuts (copy/paste in the address bar) remain untouched.
        if shortcut_pressed(ctx, ShortcutId::Copy) {
            match crate::ui::opsui::clipboard_target(
                crate::ui::opsui::ClipboardCommand::Copy,
                text_edit_focused,
                !self.selected_paths().is_empty(),
                false,
            ) {
                crate::ui::opsui::ClipboardTarget::Files => {
                    tracing::info!("Ctrl+C shortcut received");
                    self.act_copy();
                }
                crate::ui::opsui::ClipboardTarget::Text => {
                    ctx.input_mut(|i| i.events.push(egui::Event::Copy));
                }
            }
            return;
        }
        if shortcut_pressed(ctx, ShortcutId::Cut) {
            match crate::ui::opsui::clipboard_target(
                crate::ui::opsui::ClipboardCommand::Cut,
                text_edit_focused,
                !self.selected_paths().is_empty(),
                false,
            ) {
                crate::ui::opsui::ClipboardTarget::Files => self.act_cut(),
                crate::ui::opsui::ClipboardTarget::Text => {
                    ctx.input_mut(|i| i.events.push(egui::Event::Cut));
                }
            }
            return;
        }
        if shortcut_pressed(ctx, ShortcutId::Paste) {
            let has_file_clipboard = self.ops_ui.clipboard.is_some()
                || !crate::platform::get_clipboard_files().is_empty();
            match crate::ui::opsui::clipboard_target(
                crate::ui::opsui::ClipboardCommand::Paste,
                text_edit_focused,
                false,
                has_file_clipboard,
            ) {
                crate::ui::opsui::ClipboardTarget::Files => {
                    tracing::info!("Ctrl+V shortcut received");
                    self.act_paste();
                }
                crate::ui::opsui::ClipboardTarget::Text => {
                    let text = crate::platform::get_clipboard_text();
                    if !text.is_empty() {
                        ctx.input_mut(|i| i.events.push(egui::Event::Paste(text)));
                    }
                }
            }
            return;
        }
        if text_edit_focused {
            return;
        }

        // Пока вкладка нажата или перетаскивается, горячие клавиши не меняют список.
        if allow_tab_management && !self.tab_drag_state.is_active() {
            if shortcut_pressed(ctx, ShortcutId::PreviousTab) {
                let visible = self.visible_tab_indices();
                if !visible.is_empty() {
                    let current = visible
                        .iter()
                        .position(|&index| index == self.active)
                        .unwrap_or_default();
                    let prev = (current + visible.len() - 1) % visible.len();
                    self.switch_tab(visible[prev]);
                }
                return;
            }
            if shortcut_pressed(ctx, ShortcutId::NewTab) {
                self.new_tab(ctx);
                return;
            }
            if shortcut_pressed(ctx, ShortcutId::CloseTab) {
                self.close_tab(self.active, ctx);
                return;
            }
            if shortcut_pressed(ctx, ShortcutId::NextTab) {
                let visible = self.visible_tab_indices();
                if !visible.is_empty() {
                    let current = visible
                        .iter()
                        .position(|&index| index == self.active)
                        .unwrap_or_default();
                    self.switch_tab(visible[(current + 1) % visible.len()]);
                }
                return;
            }
        }

        if shortcut_pressed(ctx, ShortcutId::Delete) {
            self.act_delete(mods.shift);
            return;
        }
        if shortcut_pressed(ctx, ShortcutId::Properties) {
            self.act_props();
            return;
        }
        if shortcut_pressed(ctx, ShortcutId::Rename) {
            self.act_rename();
            return;
        }
        if shortcut_pressed(ctx, ShortcutId::Preview) {
            self.act_preview(ctx);
            return;
        }

        // Selection and navigation.
        if shortcut_pressed(ctx, ShortcutId::SelectAll) {
            let tab = &mut self.tabs[self.active];
            crate::ui::tabs::select_all_entries(&mut tab.selection, &tab.entries);
            return;
        }
        if shortcut_pressed(ctx, ShortcutId::Parent) {
            self.go_up(ctx);
            return;
        }
        if shortcut_pressed(ctx, ShortcutId::Open) {
            let target = {
                let tab = &self.tabs[self.active];
                if tab.selection.len() == 1 {
                    tab.selection
                        .iter()
                        .next()
                        .and_then(|selected_key| {
                            tab.entries.iter().find(|entry| selected_key.matches(entry))
                        })
                        .filter(|entry| {
                            entry.is_dir
                                || (entry.archive_member.is_none()
                                    && entry.extension.as_deref() == Some("zip"))
                        })
                        .map(|entry| {
                            (
                                entry.path.clone(),
                                entry.archive_member.clone(),
                                entry.is_dir,
                                entry.extension.as_deref() == Some("zip"),
                            )
                        })
                } else {
                    None
                }
            };
            if let Some((_path, Some(member), true, _)) = target {
                self.navigate_zip(self.active, member, ctx);
            } else if let Some((path, None, is_dir, is_zip)) = target {
                if is_dir || is_zip {
                    self.navigate(self.active, path, ctx);
                }
            }
            return;
        }
        if shortcut_pressed(ctx, ShortcutId::Back) {
            self.go_back(ctx);
            return;
        }
        if shortcut_pressed(ctx, ShortcutId::Forward) {
            self.go_forward(ctx);
            return;
        }

        let len = self.tabs[self.active].entries.len();
        if len == 0 {
            return;
        }
        let relative_move = if shortcut_pressed(ctx, ShortcutId::MoveDown) {
            Some(1isize)
        } else if shortcut_pressed(ctx, ShortcutId::MoveUp) {
            Some(-1)
        } else if shortcut_pressed(ctx, ShortcutId::PageDown) {
            Some(20)
        } else if shortcut_pressed(ctx, ShortcutId::PageUp) {
            Some(-20)
        } else {
            None
        };
        let move_to_start = shortcut_pressed(ctx, ShortcutId::First);
        let move_to_end = shortcut_pressed(ctx, ShortcutId::Last);
        if relative_move.is_none() && !move_to_start && !move_to_end {
            return;
        }
        let visual_order = self.selection_order();
        let order_len = visual_order.as_ref().map_or(len, Vec::len);
        if order_len == 0 {
            return;
        }
        let cursor = self.tabs[self.active].cursor;
        let current_position = visual_order.as_ref().map_or_else(
            || cursor.min(len - 1),
            |order| {
                order
                    .iter()
                    .position(|&index| index == cursor)
                    .unwrap_or(0)
            },
        );
        let extend = mods.shift;
        let target_position = if move_to_start {
            0
        } else if move_to_end {
            order_len - 1
        } else {
            (current_position as isize + relative_move.unwrap())
                .clamp(0, order_len as isize - 1) as usize
        };
        self.move_cursor(visual_order.as_deref(), target_position, extend);
    }

    fn handle_side_mouse_navigation(&mut self, ctx: &egui::Context) {
        if !self.store.settings.enable_mouse_side_navigation {
            return;
        }
        let action = ctx.input(|input| {
            for button in [egui::PointerButton::Extra1, egui::PointerButton::Extra2] {
                if input.pointer.button_clicked(button) {
                    return map_mouse_button_to_navigation(button);
                }
            }
            None
        });
        match action {
            Some(MouseNavigationAction::Back) => self.go_back(ctx),
            Some(MouseNavigationAction::Forward) => self.go_forward(ctx),
            None => {}
        }
    }

    /// Files dropped onto the window from Windows — copy into current folder.
    pub(super) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| dropped_paths(&i.raw.dropped_files));
        if dropped.is_empty() {
            return;
        }
        let dest = self.tabs[self.active].path.clone();
        if dropped.iter().any(|p| p == &dest || dest.starts_with(p)) {
            return; // dropped onto its own parent — no-op like Explorer
        }
        self.enqueue(
            "ops.copying",
            crate::core::ops::OpKind::Copy {
                sources: dropped,
                dest,
            },
        );
    }

    fn move_cursor(
        &mut self,
        visual_order: Option<&[usize]>,
        target_position: usize,
        extend: bool,
    ) {
        let tab = &mut self.tabs[self.active];
        let target = visual_order
            .and_then(|order| order.get(target_position).copied())
            .unwrap_or(target_position);
        if target >= tab.entries.len() {
            return;
        }
        let Some(target) = crate::ui::tabs::apply_cursor_move(
            &mut tab.selection,
            &mut tab.anchor,
            &tab.entries,
            visual_order,
            tab.cursor,
            target,
            extend,
        ) else {
            return;
        };
        tab.cursor = target;
        self.scroll_to_cursor = true;
    }
}

#[cfg(test)]
mod tests {
    use super::{ShortcutId, ShortcutModifiers, hotkey_bindings};

    #[test]
    fn hotkey_registry_covers_real_bindings_from_input_handler() {
        let ids: Vec<_> = hotkey_bindings().iter().map(|binding| binding.id).collect();
        assert_eq!(
            ids,
            [
                ShortcutId::Copy,
                ShortcutId::Cut,
                ShortcutId::Paste,
                ShortcutId::PreviousTab,
                ShortcutId::NewTab,
                ShortcutId::CloseTab,
                ShortcutId::NextTab,
                ShortcutId::Delete,
                ShortcutId::Properties,
                ShortcutId::Rename,
                ShortcutId::Preview,
                ShortcutId::SelectAll,
                ShortcutId::Parent,
                ShortcutId::Open,
                ShortcutId::Back,
                ShortcutId::Forward,
                ShortcutId::MoveDown,
                ShortcutId::MoveUp,
                ShortcutId::PageDown,
                ShortcutId::PageUp,
                ShortcutId::First,
                ShortcutId::Last,
            ]
        );
        let source = include_str!("input.rs");
        let handler_start = source.find("fn handle_keys_impl").unwrap();
        let handler_end = source[handler_start..]
            .find("\n    fn handle_side_mouse_navigation")
            .unwrap()
            + handler_start;
        let handler = &source[handler_start..handler_end];
        assert!(!handler.contains("key_pressed(egui::Key::"));
        for id in ids {
            assert!(
                handler.contains(&format!("ShortcutId::{id:?}")),
                "handler does not use registry entry {id:?}"
            );
        }
    }

    #[test]
    fn f3_is_preview_not_search_in_hotkey_registry() {
        let binding = hotkey_bindings()
            .iter()
            .find(|binding| binding.id == ShortcutId::Preview)
            .unwrap();
        assert_eq!(binding.key, egui::Key::F3);
        assert_eq!(binding.action_key, "help.hotkeys.action.preview");
    }

    #[test]
    fn properties_shortcut_does_not_fall_through_to_open() {
        let source = include_str!("input.rs");
        let start = source
            .find("if shortcut_pressed(ctx, ShortcutId::Properties)")
            .unwrap();
        let end = source[start..]
            .find("if shortcut_pressed(ctx, ShortcutId::Rename)")
            .unwrap()
            + start;
        let handler = &source[start..end];
        assert!(handler.contains("self.act_props();"));
        assert!(handler.contains("return;"));
    }

    #[test]
    fn conditional_bindings_are_represented_in_registry() {
        let bindings = hotkey_bindings();
        for id in [ShortcutId::Copy, ShortcutId::Cut, ShortcutId::Paste] {
            let binding = bindings.iter().find(|binding| binding.id == id).unwrap();
            assert_eq!(binding.modifiers, ShortcutModifiers::Ctrl);
            assert_eq!(
                binding.condition_key,
                Some("help.hotkeys.condition.clipboard")
            );
        }
        for id in [
            ShortcutId::PreviousTab,
            ShortcutId::NewTab,
            ShortcutId::CloseTab,
            ShortcutId::NextTab,
        ] {
            let binding = bindings.iter().find(|binding| binding.id == id).unwrap();
            assert_eq!(binding.condition_key, Some("help.hotkeys.condition.tabs"));
        }
        for id in [
            ShortcutId::MoveDown,
            ShortcutId::MoveUp,
            ShortcutId::PageDown,
            ShortcutId::PageUp,
            ShortcutId::First,
            ShortcutId::Last,
        ] {
            let binding = bindings.iter().find(|binding| binding.id == id).unwrap();
            assert_eq!(
                binding.shift_action_key,
                Some("help.hotkeys.action.extend_selection")
            );
        }
        let delete = bindings
            .iter()
            .find(|binding| binding.id == ShortcutId::Delete)
            .unwrap();
        assert_eq!(
            delete.shift_action_key,
            Some("help.hotkeys.action.delete_permanent")
        );
        let source = include_str!("input.rs");
        assert!(source.contains("if allow_tab_management && !self.tab_drag_state.is_active()"));
        assert!(source.contains("self.act_delete(mods.shift)"));
        assert!(source.contains("let extend = mods.shift"));
        assert!(source.contains("crate::ui::opsui::clipboard_target"));
    }
}
