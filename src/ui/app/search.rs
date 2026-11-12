//! Поиск по именам и настройка фильтров.

use super::*;
use crate::core::search as core_search;

pub(super) fn search_result_tab_index(tabs: &[Tab], tab_id: u64, generation: u64) -> Option<usize> {
    tabs.iter()
        .position(|tab| tab.id == tab_id && tab.search.generation == generation)
}

impl FileManagerApp {
    fn schedule_search(&mut self) {
        let tab_id = self.tabs[self.active].id;
        self.cancel_search_bg_ops(tab_id);
        self.tabs[self.active].search.schedule();
    }

    pub(super) fn start_search(&mut self, tab: usize, ctx: &egui::Context) {
        let tab_id = self.tabs[tab].id;
        self.cancel_search_bg_ops(tab_id);
        if !self.tabs[tab].search.has_criteria() {
            self.tabs[tab].search.running = false;
            self.tabs[tab].search.results_active = false;
            self.navigate_fresh(tab, ctx);
            return;
        }

        let options = {
            let search = &self.tabs[tab].search;
            core_search::parse_options(
                &search.query,
                search.recursive,
                search.fuzzy,
                &search.extensions,
                &search.min_size,
                &search.max_size,
                &search.modified_from,
                &search.modified_to,
            )
        };
        let mut options = match options {
            Ok(options) => options,
            Err(error) => {
                let key = match error {
                    SearchValidationError::InvalidSize => "search.error_size",
                    SearchValidationError::SizeRange => "search.error_size_range",
                    SearchValidationError::InvalidDate => "search.error_date",
                    SearchValidationError::DateRange => "search.error_date_range",
                };
                let message = self.t(key).to_owned();
                let state = &mut self.tabs[tab];
                state.search.running = false;
                state.search.results_active = false;
                state.search.validation_error = Some(message);
                state.status = ListStatus::Ready;
                return;
            }
        };
        options.show_hidden = self.store.settings.show_hidden_files;

        let (generation, root, archive_prefix, cancel) = {
            let state = &mut self.tabs[tab];
            state.generation = state.generation.wrapping_add(1);
            state.cancel.store(true, Ordering::Relaxed);
            state.cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            state.status = ListStatus::Loading;
            state.error = None;
            state.search.running = false;
            state.search.results_active = true;
            state.search.validation_error = None;
            state.clear_selection();
            state.cursor = 0;
            (
                state.search.generation,
                state.path.clone(),
                state.archive_prefix.clone(),
                Arc::clone(&state.search.cancel),
            )
        };
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        match self.bg_ops_pool.submit_with_cancel(
            crate::core::bg_ops::BgOpKind::Search,
            Arc::clone(&cancel),
            move |cancel| {
                let result = match archive_prefix {
                    Some(prefix) => crate::platform::search_zip(&root, &prefix, &options, &cancel),
                    None => core_search::search(&root, &options, &cancel),
                };
                let result = if cancel.load(Ordering::Relaxed) {
                    Err(crate::core::fs::ReadDirError::Cancelled)
                } else {
                    result
                };
                let _ = tx.send(WorkerMsg::SearchResults {
                    tab_id,
                    generation,
                    result,
                });
                ctx.request_repaint();
            },
        ) {
            Ok(id) => {
                self.bg_op_ui.insert(
                    id,
                    super::BgOpUiTask {
                        kind: super::BgOpUiKind::Search { tab_id, generation },
                        queued: true,
                    },
                );
            }
            Err(_) => {
                let error = self.t("background_ops.failed").to_owned();
                let state = &mut self.tabs[tab];
                state.status = ListStatus::Ready;
                state.search.results_active = false;
                state.search.validation_error = Some(error);
            }
        }
    }

    pub(super) fn dispatch_due_searches(&mut self, ctx: &egui::Context) {
        let Some(due) = self.tabs[self.active].search.debounce_at else {
            return;
        };
        let now = std::time::Instant::now();
        if due > now {
            ctx.request_repaint_after(due.duration_since(now));
            return;
        }
        self.tabs[self.active].search.debounce_at = None;
        if self.tabs[self.active].search.has_criteria() {
            self.start_search(self.active, ctx);
        } else if self.tabs[self.active].search.results_active {
            self.tabs[self.active].search.results_active = false;
            self.navigate_fresh(self.active, ctx);
        }
    }

    pub(super) fn search_bar(&mut self, ui: &mut egui::Ui) {
        let hint = self.t("search.hint").to_owned();
        let recursive_label = self.t("search.subfolders").to_owned();
        let fuzzy_label = self.t("search.fuzzy").to_owned();
        let filters_label = self.t("search.filters").to_owned();
        let clear_label = self.t("search.clear").to_owned();
        let extension_label = self.t("search.extension").to_owned();
        let min_size_label = self.t("search.min_size").to_owned();
        let max_size_label = self.t("search.max_size").to_owned();
        let from_label = self.t("search.modified_from").to_owned();
        let to_label = self.t("search.modified_to").to_owned();
        let date_hint = self.t("search.date_hint").to_owned();
        let size_hint = self.t("search.size_hint").to_owned();
        let mut changed = false;
        let mut submit_now = false;

        ui.horizontal_wrapped(|ui| {
            let search = &mut self.tabs[self.active].search;
            let response = ui.add(
                egui::TextEdit::singleline(&mut search.query)
                    .hint_text(hint)
                    .desired_width(250.0),
            );
            changed |= response.changed();
            submit_now |=
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            changed |= ui
                .checkbox(&mut search.recursive, recursive_label)
                .changed();
            changed |= ui.checkbox(&mut search.fuzzy, fuzzy_label).changed();
            if ui.button(filters_label).clicked() {
                search.filters_expanded = !search.filters_expanded;
            }
            if search.has_criteria() && ui.button(clear_label).clicked() {
                search.query.clear();
                search.recursive = false;
                search.fuzzy = false;
                search.extensions.clear();
                search.min_size.clear();
                search.max_size.clear();
                search.modified_from.clear();
                search.modified_to.clear();
                changed = true;
            }
        });

        if self.tabs[self.active].search.filters_expanded {
            ui.horizontal_wrapped(|ui| {
                let search = &mut self.tabs[self.active].search;
                ui.label(extension_label);
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut search.extensions).desired_width(130.0))
                    .changed();
                ui.label(min_size_label);
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut search.min_size)
                            .hint_text(size_hint.clone())
                            .desired_width(90.0),
                    )
                    .changed();
                ui.label(max_size_label);
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut search.max_size)
                            .hint_text(size_hint)
                            .desired_width(90.0),
                    )
                    .changed();
                ui.label(from_label);
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut search.modified_from)
                            .hint_text(date_hint.clone())
                            .desired_width(105.0),
                    )
                    .changed();
                ui.label(to_label);
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut search.modified_to)
                            .hint_text(date_hint)
                            .desired_width(105.0),
                    )
                    .changed();
            });
        }

        if let Some(error) = &self.tabs[self.active].search.validation_error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if changed || submit_now {
            self.schedule_search();
            if submit_now {
                self.tabs[self.active].search.debounce_at = Some(std::time::Instant::now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn search_cancel_generation_still_safe_in_pool() {
        let (events_tx, _events_rx) = mpsc::channel();
        let pool = crate::core::bg_ops::BgOpPool::new(1, events_tx, || {});
        let mut tabs = vec![Tab::new(PathBuf::from(r"C:\search"))];
        tabs[0].search.generation = 7;
        let tab_id = tabs[0].id;
        let cancel = Arc::clone(&tabs[0].search.cancel);
        let (started_tx, started_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let task_id = pool
            .submit_with_cancel(
                crate::core::bg_ops::BgOpKind::Search,
                cancel,
                move |cancel| {
                    let _ = started_tx.send(());
                    while !cancel.load(Ordering::Relaxed) {
                        thread::yield_now();
                    }
                    let _ = result_tx.send((tab_id, 7));
                },
            )
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        tabs[0].search.generation = 8;
        assert_eq!(
            pool.cancel(task_id),
            crate::core::bg_ops::BgOpCancelOutcome::Running
        );
        let (result_tab, result_generation) =
            result_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            search_result_tab_index(&tabs, result_tab, result_generation),
            None
        );
        assert_eq!(search_result_tab_index(&tabs, tab_id, 8), Some(0));
    }
}
