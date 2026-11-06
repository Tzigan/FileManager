use egui::{Color32, Painter, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

use crate::ui::style;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileIconKind {
    Image,
    Video,
    Audio,
    Archive,
    Pdf,
    Document,
    Spreadsheet,
    Presentation,
    Code,
    Executable,
    Shortcut,
    Generic,
}

pub(crate) fn classify_extension(extension: Option<&str>) -> FileIconKind {
    let extension = extension
        .unwrap_or_default()
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "svg" | "heic"
        | "heif" | "raw" | "cr2" | "nef" | "ico" | "avif" | "psd" | "ai" => FileIconKind::Image,
        "mp4" | "m4v" | "mov" | "avi" | "mkv" | "webm" | "wmv" | "flv" | "mpg" | "mpeg"
        | "m2ts" | "3gp" => FileIconKind::Video,
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "oga" | "wma" | "opus" | "mid"
        | "midi" => FileIconKind::Audio,
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "tgz" | "cab" | "iso"
        | "dmg" => FileIconKind::Archive,
        "pdf" => FileIconKind::Pdf,
        "xls" | "xlsx" | "xlsm" | "ods" | "csv" | "tsv" => FileIconKind::Spreadsheet,
        "ppt" | "pptx" | "pptm" | "odp" | "key" => FileIconKind::Presentation,
        "rs" | "c" | "cc" | "cpp" | "h" | "hpp" | "py" | "js" | "jsx" | "ts" | "tsx" | "html"
        | "css" | "scss" | "json" | "xml" | "yaml" | "yml" | "toml" | "sql" | "mts" | "sh"
        | "bat" | "cmd" | "ps1" | "java" | "go" | "rb" | "php" | "swift" | "kt" | "cs" => {
            FileIconKind::Code
        }
        "doc" | "docx" | "docm" | "odt" | "rtf" | "txt" | "md" | "markdown" | "log" | "tex"
        | "pages" | "ini" | "conf" | "cfg" | "env" | "rst" => FileIconKind::Document,
        "exe" | "dll" | "msi" | "msp" | "sys" | "com" | "appx" | "msix" | "apk" => {
            FileIconKind::Executable
        }
        "lnk" | "url" => FileIconKind::Shortcut,
        _ => FileIconKind::Generic,
    }
}

pub(crate) fn paint_entry_icon(
    painter: &Painter,
    rect: Rect,
    is_dir: bool,
    extension: Option<&str>,
    dark_mode: bool,
) {
    if is_dir {
        paint_folder_icon(painter, rect);
    } else {
        paint_file_icon(painter, rect, classify_extension(extension), dark_mode);
    }
}

pub(crate) fn paint_folder_icon(painter: &Painter, bounds: Rect) {
    let rect = fitted_folder_rect(bounds);
    let width = rect.width();
    let height = rect.height();
    let tab = Rect::from_min_max(
        pos2(rect.left() + width * 0.08, rect.top() + height * 0.14),
        pos2(rect.left() + width * 0.48, rect.top() + height * 0.43),
    );
    let body = Rect::from_min_max(
        pos2(rect.left() + width * 0.05, rect.top() + height * 0.30),
        pos2(rect.right() - width * 0.05, rect.bottom() - height * 0.12),
    );
    let back = Color32::from_rgb(202, 128, 36);
    let front = Color32::from_rgb(245, 188, 82);
    painter.rect_filled(tab, style::RADIUS_ICON_FINE, back);
    painter.rect_filled(body, style::RADIUS_ICON, front);
    painter.line_segment(
        [body.left_bottom(), body.right_bottom()],
        Stroke::new((width.min(height) * 0.035).max(0.7), back),
    );
}

fn fitted_folder_rect(bounds: Rect) -> Rect {
    const ASPECT: f32 = 1.2;
    let available_width = bounds.width();
    let available_height = bounds.height();
    let (width, height) = if available_width / available_height > ASPECT {
        (available_height * ASPECT, available_height)
    } else {
        (available_width, available_width / ASPECT)
    };
    Rect::from_center_size(bounds.center(), vec2(width, height))
}

fn paint_file_icon(painter: &Painter, rect: Rect, kind: FileIconKind, dark_mode: bool) {
    let scale = rect.width().min(rect.height());
    let fold = scale * 0.22;
    let page = rect.shrink(scale * 0.06);
    let (accent, pale) = palette(kind);
    let paper = if dark_mode {
        Color32::from_rgb(45, 53, 64)
    } else {
        Color32::from_rgb(249, 251, 253)
    };
    let outline = if dark_mode {
        Color32::from_rgb(113, 126, 142)
    } else {
        Color32::from_rgb(151, 164, 178)
    };

    painter.rect_filled(page, style::RADIUS_ICON_FINE, paper);
    painter.rect_stroke(
        page,
        style::RADIUS_ICON_FINE,
        Stroke::new((scale * 0.035).max(0.7), outline),
        StrokeKind::Inside,
    );
    painter.add(Shape::convex_polygon(
        vec![
            pos2(page.right() - fold, page.top()),
            pos2(page.right(), page.top() + fold),
            pos2(page.right() - fold, page.top() + fold),
        ],
        pale,
        Stroke::NONE,
    ));
    painter.line_segment(
        [
            pos2(page.right() - fold, page.top()),
            pos2(page.right(), page.top() + fold),
        ],
        Stroke::new((scale * 0.035).max(0.7), accent),
    );

    if should_paint_kind_mark(kind, scale) {
        paint_kind_mark(painter, page, kind, accent);
    } else {
        painter.line_segment(
            [
                pos2(page.left() + scale * 0.20, page.bottom() - scale * 0.18),
                pos2(page.right() - scale * 0.18, page.bottom() - scale * 0.18),
            ],
            Stroke::new((scale * 0.07).max(1.0), accent),
        );
    }
}

fn should_paint_kind_mark(kind: FileIconKind, scale: f32) -> bool {
    scale >= 26.0 || kind == FileIconKind::Executable
}

fn paint_kind_mark(painter: &Painter, page: Rect, kind: FileIconKind, color: Color32) {
    let scale = page.width().min(page.height());
    let mark = Rect::from_min_max(
        pos2(
            page.left() + page.width() * 0.22,
            page.top() + page.height() * 0.34,
        ),
        pos2(
            page.right() - page.width() * 0.18,
            page.bottom() - page.height() * 0.16,
        ),
    );
    let stroke = Stroke::new((scale * 0.055).max(1.0), color);

    match kind {
        FileIconKind::Image => {
            painter.circle_filled(
                pos2(mark.right() - scale * 0.12, mark.top() + scale * 0.10),
                scale * 0.07,
                color,
            );
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(mark.left(), mark.bottom()),
                    pos2(
                        mark.left() + mark.width() * 0.42,
                        mark.top() + mark.height() * 0.30,
                    ),
                    pos2(mark.left() + mark.width() * 0.60, mark.bottom()),
                ],
                color,
                Stroke::NONE,
            ));
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(mark.left() + mark.width() * 0.35, mark.bottom()),
                    pos2(
                        mark.left() + mark.width() * 0.68,
                        mark.top() + mark.height() * 0.48,
                    ),
                    pos2(mark.right(), mark.bottom()),
                ],
                color.gamma_multiply(0.72),
                Stroke::NONE,
            ));
        }
        FileIconKind::Video => {
            let screen = Rect::from_min_max(
                pos2(mark.left(), mark.top() + mark.height() * 0.08),
                pos2(mark.right(), mark.bottom() - mark.height() * 0.15),
            );
            painter.rect_stroke(screen, style::RADIUS_ICON_FINE, stroke, StrokeKind::Inside);
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(
                        screen.left() + screen.width() * 0.38,
                        screen.top() + screen.height() * 0.22,
                    ),
                    pos2(screen.right() - screen.width() * 0.20, screen.center().y),
                    pos2(
                        screen.left() + screen.width() * 0.38,
                        screen.bottom() - screen.height() * 0.22,
                    ),
                ],
                color,
                Stroke::NONE,
            ));
        }
        FileIconKind::Audio => {
            let bars = [0.35, 0.72, 0.48, 0.95, 0.58];
            for (i, height) in bars.into_iter().enumerate() {
                let x = mark.left() + mark.width() * (i as f32 + 0.5) / 5.0;
                let half = mark.height() * height * 0.5;
                painter.line_segment(
                    [
                        pos2(x, mark.center().y - half),
                        pos2(x, mark.center().y + half),
                    ],
                    stroke,
                );
            }
        }
        FileIconKind::Archive => {
            let belt_x = mark.center().x;
            painter.line_segment(
                [pos2(belt_x, mark.top()), pos2(belt_x, mark.bottom())],
                stroke,
            );
            for fraction in [0.22, 0.50, 0.78] {
                let y = mark.top() + mark.height() * fraction;
                painter.line_segment([pos2(mark.left(), y), pos2(mark.right(), y)], stroke);
            }
        }
        FileIconKind::Spreadsheet => {
            painter.rect_stroke(mark, style::RADIUS_ICON_FINE, stroke, StrokeKind::Inside);
            for fraction in [1.0 / 3.0, 2.0 / 3.0] {
                let x = mark.left() + mark.width() * fraction;
                let y = mark.top() + mark.height() * fraction;
                painter.line_segment([pos2(x, mark.top()), pos2(x, mark.bottom())], stroke);
                painter.line_segment([pos2(mark.left(), y), pos2(mark.right(), y)], stroke);
            }
        }
        FileIconKind::Presentation => {
            let screen = Rect::from_min_max(
                pos2(mark.left(), mark.top()),
                pos2(mark.right(), mark.bottom() - mark.height() * 0.22),
            );
            painter.rect_stroke(screen, style::RADIUS_ICON_FINE, stroke, StrokeKind::Inside);
            painter.line_segment(
                [
                    pos2(screen.center().x, screen.bottom()),
                    pos2(screen.center().x, mark.bottom()),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(mark.left() + mark.width() * 0.30, mark.bottom()),
                    pos2(mark.right() - mark.width() * 0.30, mark.bottom()),
                ],
                stroke,
            );
        }
        FileIconKind::Code => {
            let mid_x = mark.center().x;
            let mid_y = mark.center().y;
            let left = mark.left() + mark.width() * 0.10;
            let right = mark.right() - mark.width() * 0.10;
            painter.line_segment(
                [
                    pos2(mid_x - mark.width() * 0.10, mid_y - mark.height() * 0.30),
                    pos2(left, mid_y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(left, mid_y),
                    pos2(mid_x - mark.width() * 0.10, mid_y + mark.height() * 0.30),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(mid_x + mark.width() * 0.10, mid_y - mark.height() * 0.30),
                    pos2(right, mid_y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(right, mid_y),
                    pos2(mid_x + mark.width() * 0.10, mid_y + mark.height() * 0.30),
                ],
                stroke,
            );
        }
        FileIconKind::Executable => {
            let gap = mark.width() * 0.10;
            let cell = (mark.width().min(mark.height()) - gap) * 0.5;
            for row in 0..2 {
                for col in 0..2 {
                    let min = pos2(
                        mark.left() + col as f32 * (cell + gap),
                        mark.top() + row as f32 * (cell + gap),
                    );
                    painter.rect_filled(
                        Rect::from_min_size(min, vec2(cell, cell)),
                        style::RADIUS_ICON_FINE,
                        color,
                    );
                }
            }
        }
        FileIconKind::Shortcut => {
            let start = pos2(
                mark.left() + mark.width() * 0.18,
                mark.bottom() - mark.height() * 0.12,
            );
            let end = pos2(
                mark.right() - mark.width() * 0.12,
                mark.top() + mark.height() * 0.12,
            );
            painter.line_segment([start, end], stroke);
            painter.line_segment(
                [
                    end,
                    pos2(end.x - mark.width() * 0.28, end.y + mark.height() * 0.02),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    end,
                    pos2(end.x - mark.width() * 0.02, end.y + mark.height() * 0.28),
                ],
                stroke,
            );
        }
        FileIconKind::Pdf | FileIconKind::Document | FileIconKind::Generic => {
            for fraction in [0.24, 0.50, 0.76] {
                let y = mark.top() + mark.height() * fraction;
                painter.line_segment([pos2(mark.left(), y), pos2(mark.right(), y)], stroke);
            }
        }
    }
}

fn palette(kind: FileIconKind) -> (Color32, Color32) {
    let accent = match kind {
        FileIconKind::Image => Color32::from_rgb(54, 145, 98),
        FileIconKind::Video => Color32::from_rgb(194, 73, 66),
        FileIconKind::Audio => Color32::from_rgb(133, 83, 175),
        FileIconKind::Archive => Color32::from_rgb(181, 123, 43),
        FileIconKind::Pdf => Color32::from_rgb(191, 59, 56),
        FileIconKind::Document => Color32::from_rgb(55, 112, 180),
        FileIconKind::Spreadsheet => Color32::from_rgb(42, 132, 82),
        FileIconKind::Presentation => Color32::from_rgb(202, 112, 42),
        FileIconKind::Code => Color32::from_rgb(44, 128, 156),
        FileIconKind::Executable => Color32::from_rgb(84, 102, 120),
        FileIconKind::Shortcut => Color32::from_rgb(64, 124, 190),
        FileIconKind::Generic => Color32::from_rgb(112, 126, 142),
    };
    (accent, accent.gamma_multiply(0.35))
}

#[cfg(test)]
mod tests {
    use super::{
        FileIconKind, classify_extension, fitted_folder_rect, should_paint_kind_mark,
    };
    use egui::{Rect, pos2, vec2};

    #[test]
    fn common_file_extensions_map_to_distinct_icon_categories() {
        assert_eq!(classify_extension(Some("PNG")), FileIconKind::Image);
        assert_eq!(classify_extension(Some(".mp4")), FileIconKind::Video);
        assert_eq!(classify_extension(Some("flac")), FileIconKind::Audio);
        assert_eq!(classify_extension(Some("7z")), FileIconKind::Archive);
        assert_eq!(classify_extension(Some("pdf")), FileIconKind::Pdf);
        assert_eq!(classify_extension(Some("docx")), FileIconKind::Document);
        assert_eq!(classify_extension(Some("xlsx")), FileIconKind::Spreadsheet);
        assert_eq!(classify_extension(Some("pptx")), FileIconKind::Presentation);
        assert_eq!(classify_extension(Some("rs")), FileIconKind::Code);
        assert_eq!(classify_extension(Some("exe")), FileIconKind::Executable);
        assert_eq!(classify_extension(Some("lnk")), FileIconKind::Shortcut);
        assert_eq!(classify_extension(Some("ts")), FileIconKind::Code);
        assert_eq!(classify_extension(Some("m2ts")), FileIconKind::Video);
        assert_eq!(classify_extension(Some("unknown")), FileIconKind::Generic);
        assert_eq!(classify_extension(None), FileIconKind::Generic);
    }

    #[test]
    fn folder_icons_keep_the_same_shape_ratio_at_list_tree_and_grid_sizes() {
        let bounds = [
            Rect::from_center_size(pos2(0.0, 0.0), vec2(18.0, 16.0)),
            Rect::from_center_size(pos2(0.0, 0.0), vec2(20.0, 18.0)),
            Rect::from_center_size(pos2(0.0, 0.0), vec2(38.0, 40.0)),
        ];
        let fitted = bounds.map(fitted_folder_rect);
        let ratio = fitted[0].width() / fitted[0].height();
        for rect in fitted {
            assert!((rect.width() / rect.height() - ratio).abs() < f32::EPSILON * 8.0);
            assert_eq!(rect.center(), pos2(0.0, 0.0));
        }
    }

    #[test]
    fn executable_icon_mark_is_drawn_at_table_and_list_sizes() {
        assert!(should_paint_kind_mark(FileIconKind::Executable, 16.0));
        assert!(!should_paint_kind_mark(FileIconKind::Generic, 16.0));
        assert!(should_paint_kind_mark(FileIconKind::Generic, 26.0));
    }
}
