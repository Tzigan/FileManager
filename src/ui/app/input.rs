//! Клавиатурный ввод и перетаскивание файлов.

use super::*;

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
        self.handle_side_mouse_navigation(ctx);
        let text_edit_focused = ctx.text_edit_focused();
        let mods = ctx.input(|i| i.modifiers);
        let input = |i: &egui::InputState| i.clone();

        // Prefer file operations when a file selection/internal file clipboard
        // exists. This also recovers if the address TextEdit retained focus after
        // the user clicked a file row; without a file selection normal text-edit
        // shortcuts (copy/paste in the address bar) remain untouched.
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::C)) {
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
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::X)) {
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
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::V)) {
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
            if mods.ctrl && mods.shift && ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
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
            if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::T)) {
                self.new_tab(ctx);
                return;
            }
            if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::W)) {
                self.close_tab(self.active, ctx);
                return;
            }
            if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
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

        if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
            self.act_delete(mods.shift);
            return;
        }
        if mods.alt && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.act_props();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
            self.act_rename();
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
            self.act_preview(ctx);
            return;
        }

        // Selection and navigation.
        if mods.command && ctx.input(|i| i.key_pressed(egui::Key::A)) {
            let tab = &mut self.tabs[self.active];
            crate::ui::tabs::select_all_entries(&mut tab.selection, &tab.entries);
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Backspace)) {
            self.go_up(ctx);
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
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
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) && mods.alt {
            self.go_back(ctx);
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) && mods.alt {
            self.go_forward(ctx);
            return;
        }

        let len = self.tabs[self.active].entries.len();
        if len == 0 {
            return;
        }
        let i = input(&ctx.input(|i| i.clone()));
        let relative_move = if i.key_pressed(egui::Key::ArrowDown) {
            Some(1isize)
        } else if i.key_pressed(egui::Key::ArrowUp) {
            Some(-1)
        } else if i.key_pressed(egui::Key::PageDown) {
            Some(20)
        } else if i.key_pressed(egui::Key::PageUp) {
            Some(-20)
        } else {
            None
        };
        let move_to_start = i.key_pressed(egui::Key::Home);
        let move_to_end = i.key_pressed(egui::Key::End);
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
