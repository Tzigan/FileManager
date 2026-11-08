//! Клавиатурный ввод и перетаскивание файлов.

use super::*;

impl FileManagerApp {
    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
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

        // Tab management.
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::T)) {
            self.new_tab(ctx);
            return;
        }
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::W)) {
            self.close_tab(self.active, ctx);
            return;
        }
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            let next = (self.active + 1) % self.tabs.len();
            self.switch_tab(next);
            return;
        }
        if mods.ctrl && mods.shift && ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            let prev = (self.active + self.tabs.len() - 1) % self.tabs.len();
            self.switch_tab(prev);
            return;
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
        if mods.ctrl && ctx.input(|i| i.key_pressed(egui::Key::A)) {
            let len = self.tabs[self.active].entries.len();
            self.tabs[self.active].selection.extend(0..len);
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
                        .and_then(|&i| tab.entries.get(i))
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
        let cur = self.tabs[self.active].cursor.min(len - 1);
        let extend = mods.shift;
        let i = input(&ctx.input(|i| i.clone()));
        if i.key_pressed(egui::Key::ArrowDown) {
            self.move_cursor(cur.saturating_add(1), extend);
        } else if i.key_pressed(egui::Key::ArrowUp) {
            self.move_cursor(cur.saturating_sub(1), extend);
        } else if i.key_pressed(egui::Key::PageDown) {
            self.move_cursor(cur.saturating_add(20), extend);
        } else if i.key_pressed(egui::Key::PageUp) {
            self.move_cursor(cur.saturating_sub(20), extend);
        } else if i.key_pressed(egui::Key::Home) {
            self.move_cursor(0, extend);
        } else if i.key_pressed(egui::Key::End) {
            self.move_cursor(len - 1, extend);
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

    fn move_cursor(&mut self, target: usize, extend: bool) {
        let tab = &mut self.tabs[self.active];
        let len = tab.entries.len();
        let Some(target) = crate::ui::tabs::apply_cursor_move(
            &mut tab.selection,
            &mut tab.anchor,
            tab.cursor,
            len,
            target,
            extend,
        ) else {
            return;
        };
        tab.cursor = target;
        self.scroll_to_cursor = true;
    }
}
