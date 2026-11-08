//! Предпросмотр текста и изображений, а также кэш эскизов.

use super::*;

impl FileManagerApp {
    pub(crate) fn open_preview(&mut self, path: PathBuf, ctx: &egui::Context) {
        if !crate::core::preview::supports_preview(&path) {
            return;
        }
        self.preview.cancel.store(true, Ordering::Relaxed);
        self.preview.generation = self.preview.generation.wrapping_add(1);
        self.preview.cancel = Arc::new(AtomicBool::new(false));
        self.preview.target = Some(path.clone());
        self.preview.text = None;
        self.preview.image = None;
        self.preview.image_dimensions = None;
        self.preview.error = None;
        self.preview.loading = true;

        let generation = self.preview.generation;
        let cancel = self.preview.cancel.clone();
        let tx = self.msg_tx.clone();
        let ctx = ctx.clone();
        if crate::core::preview::supports_text_preview(&path) {
            thread::spawn(move || {
                let result = if cancel.load(Ordering::Relaxed) {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "preview cancelled",
                    ))
                } else {
                    crate::core::preview::read_text_preview(&path)
                };
                let _ = tx.send(WorkerMsg::TextPreview {
                    generation,
                    path,
                    result,
                });
                ctx.request_repaint();
            });
        } else {
            thread::spawn(move || {
                let result = crate::platform::decode_image(&path, 1600);
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let _ = tx.send(WorkerMsg::ImagePreview {
                    generation,
                    path,
                    result,
                });
                ctx.request_repaint();
            });
        }
    }

    pub(super) fn act_preview(&mut self, ctx: &egui::Context) {
        let tab = &self.tabs[self.active];
        let index = tab
            .selection
            .iter()
            .next()
            .copied()
            .or(if tab.entries.is_empty() {
                None
            } else {
                Some(tab.cursor)
            });
        let path = index
            .and_then(|index| tab.entries.get(index))
            .filter(|entry| !entry.is_dir)
            .map(|entry| entry.path.clone());
        if let Some(path) = path {
            self.open_preview(path, ctx);
        }
    }

    pub(super) fn preview_window(&mut self, ctx: &egui::Context) {
        let Some(target) = self.preview.target.clone() else {
            return;
        };
        let name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.display().to_string());
        let title = format!("{} — {name}", self.t("preview.title"));
        let loading_label = self.t("preview.loading").to_owned();
        let truncated_label = self.t("preview.truncated").to_owned();
        let mut open = true;
        egui::Window::new(title)
            .resizable(true)
            .default_size([760.0, 540.0])
            .open(&mut open)
            .show(ctx, |ui| {
                if self.preview.loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(&loading_label);
                    });
                }
                if let Some(error) = &self.preview.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                if let Some(preview) = &mut self.preview.text {
                    if preview.truncated {
                        ui.label(&truncated_label);
                    }
                    ui.add(
                        egui::TextEdit::multiline(&mut preview.text)
                            .font(egui::TextStyle::Monospace)
                            .code_editor()
                            .interactive(false)
                            .desired_width(f32::INFINITY)
                            .desired_rows(28),
                    );
                }
                if let Some(texture) = &self.preview.image {
                    if let Some((width, height)) = self.preview.image_dimensions {
                        ui.label(format!("{width} × {height}"));
                    }
                    let image_size = texture.size_vec2();
                    let available = ui.available_size();
                    let scale = (available.x.max(1.0) / image_size.x)
                        .min(available.y.max(1.0) / image_size.y)
                        .min(1.0);
                    let display_size = image_size * scale;
                    ui.centered_and_justified(|ui| {
                        ui.add(egui::Image::new((texture.id(), display_size)));
                    });
                }
            });
        if !open {
            self.preview.clear();
        }
    }

    pub(crate) fn paint_thumbnail(
        &mut self,
        ui: &mut egui::Ui,
        entry: &FileEntry,
        rect: egui::Rect,
    ) -> bool {
        if !crate::core::preview::supports_image_preview(&entry.path) || !ui.is_rect_visible(rect) {
            return false;
        }
        let key = Self::thumbnail_key(entry);
        self.thumbnail_visible.insert(key.clone());

        if !self.thumbnail_cache.contains_key(&key)
            && !self.thumbnail_jobs.contains_key(&key)
            && !self.thumbnail_failed.contains(&key)
        {
            let cancel = Arc::new(AtomicBool::new(false));
            match self.thumbnail_tx.try_send(ThumbnailRequest {
                key: key.clone(),
                cancel: cancel.clone(),
            }) {
                Ok(()) => {
                    self.thumbnail_jobs.insert(key.clone(), cancel);
                }
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
            }
        }

        let Some(texture) = self.thumbnail_cache.get(&key) else {
            return false;
        };
        let source_size = texture.size_vec2();
        let scale = (rect.width() / source_size.x).min(rect.height() / source_size.y);
        let target = egui::Rect::from_center_size(rect.center(), source_size * scale);
        ui.painter().image(
            texture.id(),
            target,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        true
    }

    fn thumbnail_key(entry: &FileEntry) -> ThumbnailKey {
        let modified_nanos = entry
            .modified
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        ThumbnailKey {
            path: entry.path.clone(),
            size: entry.size.unwrap_or_default(),
            modified_nanos,
        }
    }

    pub(super) fn cache_thumbnail(
        &mut self,
        ctx: &egui::Context,
        key: ThumbnailKey,
        decoded: crate::platform::DecodedImage,
    ) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [decoded.width as usize, decoded.height as usize],
            &decoded.rgba,
        );
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        let texture = ctx.load_texture(
            format!("thumbnail-{:x}", hasher.finish()),
            image,
            egui::TextureOptions::LINEAR,
        );
        self.thumbnail_cache.insert(key.clone(), texture);
        self.thumbnail_order.push_back(key);
        while self.thumbnail_cache.len() > 256 {
            if let Some(oldest) = self.thumbnail_order.pop_front() {
                self.thumbnail_cache.remove(&oldest);
            }
        }
    }

    fn cancel_invisible_thumbnails(&mut self) {
        let invisible: Vec<_> = self
            .thumbnail_jobs
            .keys()
            .filter(|key| !self.thumbnail_visible.contains(*key))
            .cloned()
            .collect();
        for key in invisible {
            if let Some(cancel) = self.thumbnail_jobs.remove(&key) {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        self.thumbnail_failed
            .retain(|key| self.thumbnail_visible.contains(key));
    }

    pub(crate) fn begin_thumbnail_frame(&mut self) {
        self.thumbnail_visible.clear();
    }

    pub(crate) fn finish_thumbnail_frame(&mut self) {
        self.cancel_invisible_thumbnails();
    }

    pub(crate) fn cancel_all_thumbnails(&mut self) {
        for cancel in self.thumbnail_jobs.drain().map(|(_, cancel)| cancel) {
            cancel.store(true, Ordering::Relaxed);
        }
        self.thumbnail_visible.clear();
    }
}
