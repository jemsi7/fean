//! The code editor: syntax colours, line numbers, problems in place, find, and jumping to a line.

use eframe::egui;
use egui::text::{CCursor, CCursorRange, LayoutJob};
use egui::{Color32, FontId, Rect, RichText, Sense, Shape, TextFormat};
use fean_core::syntax::KEYWORDS;

use crate::app::{palette, Diag, IdeApp};

const DECLARATIONS: &[&str] = &[
    "model", "actor", "asset", "enum", "time", "param", "cell", "slot", "assume", "invariant", "def", "event",
    "contract", "instance", "batch", "proc", "program", "weight", "value", "query",
];
const TYPES: &[&str] = &["Bool", "Time", "Actor", "Asset", "Num", "Qty"];

struct Colors {
    plain: Color32,
    declaration: Color32,
    keyword: Color32,
    ty: Color32,
    number: Color32,
    comment: Color32,
    expectation: Color32,
    symbol: Color32,
}

fn colors(dark: bool) -> Colors {
    if dark {
        Colors {
            plain: Color32::from_rgb(226, 232, 240),
            declaration: Color32::from_rgb(96, 165, 250),
            keyword: Color32::from_rgb(196, 181, 253),
            ty: Color32::from_rgb(94, 234, 212),
            number: Color32::from_rgb(253, 186, 116),
            comment: Color32::from_rgb(148, 163, 184),
            expectation: Color32::from_rgb(134, 239, 172),
            symbol: Color32::from_rgb(203, 213, 225),
        }
    } else {
        Colors {
            plain: Color32::from_rgb(15, 23, 42),
            declaration: Color32::from_rgb(29, 78, 216),
            keyword: Color32::from_rgb(124, 58, 237),
            ty: Color32::from_rgb(15, 118, 110),
            number: Color32::from_rgb(180, 83, 9),
            comment: Color32::from_rgb(100, 116, 139),
            expectation: Color32::from_rgb(21, 128, 61),
            symbol: Color32::from_rgb(71, 85, 105),
        }
    }
}

/// Colour a model text. This scanner never fails: what it does not know stays plain.
pub fn highlight(text: &str, font: &FontId, dark: bool) -> LayoutJob {
    let c = colors(dark);
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let bytes = text.as_bytes();
    let add = |job: &mut LayoutJob, from: usize, to: usize, color: Color32, italics: bool| {
        if from < to {
            job.append(&text[from..to], 0.0, TextFormat { font_id: font.clone(), color, italics, ..Default::default() });
        }
    };
    let mut pos = 0;
    while pos < bytes.len() {
        let b = bytes[pos];
        if b == b'/' && bytes.get(pos + 1) == Some(&b'/') {
            let end = text[pos..].find('\n').map(|n| pos + n).unwrap_or(bytes.len());
            let expected = text[pos..end].trim_start_matches('/').trim_start().starts_with("=>");
            add(&mut job, pos, end, if expected { c.expectation } else { c.comment }, !expected);
            pos = end;
        } else if b.is_ascii_digit() || (b == b'@' && bytes.get(pos + 1).is_some_and(|d| d.is_ascii_digit())) {
            let mut end = pos + 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            add(&mut job, pos, end, c.number, false);
            pos = end;
        } else if b.is_ascii_alphabetic() || b == b'_' {
            let mut end = pos + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            let word = &text[pos..end];
            let color = if DECLARATIONS.contains(&word) {
                c.declaration
            } else if TYPES.contains(&word) {
                c.ty
            } else if KEYWORDS.contains(&word) {
                c.keyword
            } else {
                c.plain
            };
            add(&mut job, pos, end, color, false);
            pos = end;
        } else if b.is_ascii_punctuation() {
            add(&mut job, pos, pos + 1, c.symbol, false);
            pos += 1;
        } else {
            // spaces, line ends and anything else (Hangul in a stray place included)
            let mut end = pos + text[pos..].chars().next().map(|ch| ch.len_utf8()).unwrap_or(1);
            while end < bytes.len() && matches!(bytes[end], b' ' | b'\n' | b'\t' | b'\r') {
                end += 1;
            }
            add(&mut job, pos, end, c.plain, false);
            pos = end;
        }
    }
    job
}

/// The character index where a line (1-based) starts.
pub fn line_start(text: &str, line: u32) -> usize {
    let mut seen = 1;
    for (i, ch) in text.chars().enumerate() {
        if seen == line {
            return i;
        }
        if ch == '\n' {
            seen += 1;
        }
    }
    text.chars().count()
}

/// (line, column), both 1-based, of a character index.
pub fn line_of(text: &str, index: usize) -> (usize, usize) {
    let (mut line, mut col) = (1, 1);
    for ch in text.chars().take(index) {
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// The problems to list: what the live check found, and the error of the last run if it differs.
pub fn problems(app: &IdeApp) -> Vec<Diag> {
    let mut out = app.diagnostics.clone();
    if let Some(f) = &app.run_error {
        let line = f.loc.map(|l| l.line);
        if !out.iter().any(|d| d.code == f.code && d.line == line) && app.stale() {
            out.push(Diag { kind: "run".to_string(), code: f.code.clone(), line, message: f.detail.clone() });
        }
    }
    out
}

pub fn draw(app: &mut IdeApp, ui: &mut egui::Ui) {
    let pal = palette(app.dark);
    ui.horizontal(|ui| {
        ui.label(RichText::new("코드 편집기").strong());
        ui.label(RichText::new(app.file_name()).color(pal.muted));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.selectable_label(app.find.open, "찾기").on_hover_text("⌘F").clicked() {
                app.find.open = !app.find.open;
                app.find.focus = app.find.open;
            }
        });
    });
    if app.find.open {
        find_bar(app, ui);
    }
    ui.separator();
    // the list of problems sits under the text
    let list = problems(app);
    egui::TopBottomPanel::bottom("problems_panel").resizable(false).show_inside(ui, |ui| {
        ui.add_space(2.0);
        if list.is_empty() {
            let text = if app.checked() { "문제 없음" } else { "검사 중…" };
            ui.label(RichText::new(text).small().color(if app.checked() { pal.ok } else { pal.muted }));
        } else {
            ui.label(RichText::new(format!("문제 {}건", list.len())).small().strong().color(pal.error));
            for d in &list {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("●").color(pal.error));
                    let place = match d.line {
                        Some(line) => format!("줄 {line}"),
                        None => "줄 없음".to_string(),
                    };
                    let link = ui.link(RichText::new(format!("[{}] {place}", d.code)).strong());
                    if link.on_hover_text("이 줄로 이동").clicked() {
                        if let Some(line) = d.line {
                            app.goto = Some(line);
                        }
                    }
                    ui.label(&d.message);
                });
            }
        }
        ui.add_space(2.0);
    });
    egui::CentralPanel::default().frame(egui::Frame::NONE).show_inside(ui, |ui| text_area(app, ui, &list));
}

fn find_bar(app: &mut IdeApp, ui: &mut egui::Ui) {
    let pal = palette(app.dark);
    ui.horizontal(|ui| {
        ui.label("찾기");
        let field = ui.add(egui::TextEdit::singleline(&mut app.find.query).desired_width(180.0).hint_text("찾을 글"));
        if app.find.focus {
            field.request_focus();
            app.find.focus = false;
        }
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            app.find.go = Some(!ui.input(|i| i.modifiers.shift));
            field.request_focus();
        }
        if ui.button("이전").clicked() {
            app.find.go = Some(false);
        }
        if ui.button("다음").clicked() {
            app.find.go = Some(true);
        }
        if !app.find.query.is_empty() {
            ui.label(RichText::new(format!("{}곳", app.find.count)).color(pal.muted));
        }
        if ui.button("닫기").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.find.open = false;
        }
    });
}

fn text_area(app: &mut IdeApp, ui: &mut egui::Ui, list: &[Diag]) {
    let pal = palette(app.dark);
    let dark = app.dark;
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let row_height = ui.fonts_mut(|f| f.row_height(&font));
    let digit = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let lines = app.text.bytes().filter(|b| *b == b'\n').count() + 1;
    let gutter = digit * (lines.to_string().len().max(2) as f32) + 32.0;
    let id = egui::Id::new("fean_editor");
    let window = ui.max_rect();

    egui::ScrollArea::both().id_salt("editor_scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.allocate_exact_size(egui::vec2(gutter, 1.0), Sense::hover());
            let marks = ui.painter().add(Shape::Noop); // drawn behind the text, filled in below
            let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                let job = highlight(buf.as_str(), &font, dark);
                ui.fonts_mut(|f| f.layout_job(job))
            };
            let rows = ((window.height() / row_height) as usize).max(4);
            let output = egui::TextEdit::multiline(&mut app.text)
                .id(id)
                .font(egui::TextStyle::Monospace)
                .code_editor()
                .frame(false)
                .desired_width(f32::INFINITY)
                .desired_rows(rows)
                .lock_focus(true)
                .layouter(&mut layouter)
                .show(ui);
            if output.response.changed() {
                app.touched();
            }
            let origin = output.galley_pos;
            let galley = output.galley.clone();
            // one row per line: the layouter never wraps
            let row_rect = move |line: u32, from: f32, to: f32| {
                let (top, bottom) = match galley.rows.get(line.saturating_sub(1) as usize) {
                    Some(row) => (row.rect().top(), row.rect().bottom()),
                    None => ((line.saturating_sub(1)) as f32 * row_height, line as f32 * row_height),
                };
                Rect::from_min_max(egui::pos2(from, origin.y + top), egui::pos2(to, origin.y + bottom))
            };
            let clip = ui.clip_rect();
            let (left, right) = (clip.left(), clip.right().max(origin.x + output.galley.size().x));

            // where the cursor is
            if let Some(range) = output.cursor_range {
                app.cursor = line_of(&app.text, range.primary.index);
            }
            // find: count the matches, mark them, move to the next one when asked
            let mut shapes: Vec<Shape> = Vec::new();
            if app.find.open && !app.find.query.is_empty() {
                let hits: Vec<usize> = app.text.match_indices(app.find.query.as_str()).map(|(i, _)| i).collect();
                app.find.count = hits.len();
                let chars = app.find.query.chars().count();
                let to_char = |byte: usize| app.text[..byte].chars().count();
                for &hit in hits.iter().take(400) {
                    let start = to_char(hit);
                    let (a, b) = (output.galley.pos_from_cursor(CCursor::new(start)), output.galley.pos_from_cursor(CCursor::new(start + chars)));
                    let rect = Rect::from_min_max(a.min, egui::pos2(b.max.x, a.max.y)).translate(origin.to_vec2());
                    shapes.push(Shape::rect_filled(rect, 2.0, pal.warn.gamma_multiply(0.35)));
                }
                if let Some(forward) = app.find.go.take() {
                    let at = output.cursor_range.map(|r| r.primary.index.max(r.secondary.index)).unwrap_or(0);
                    let starts: Vec<usize> = hits.iter().map(|&h| to_char(h)).collect();
                    let next = if forward {
                        starts.iter().copied().find(|&s| s >= at).or(starts.first().copied())
                    } else {
                        starts.iter().copied().rev().find(|&s| s + chars < at).or(starts.last().copied())
                    };
                    if let Some(start) = next {
                        let mut state = output.state.clone();
                        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(start), CCursor::new(start + chars))));
                        state.store(ui.ctx(), id);
                        let (line, _) = line_of(&app.text, start);
                        ui.scroll_to_rect(row_rect(line as u32, origin.x, origin.x + 10.0), Some(egui::Align::Center));
                    }
                }
            } else {
                app.find.go = None;
                app.find.count = 0;
            }
            // a jump from the results (or from the list of problems)
            if let Some(line) = app.goto.take() {
                let line = line.clamp(1, lines as u32);
                let mut state = output.state.clone();
                state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(line_start(&app.text, line)))));
                state.store(ui.ctx(), id);
                ui.ctx().memory_mut(|m| m.request_focus(id));
                ui.scroll_to_rect(row_rect(line, origin.x, origin.x + 10.0), Some(egui::Align::Center));
                app.flash = Some((line, app.now));
                app.cursor = (line as usize, 1);
            }
            // marks behind the text: the line just jumped to, the lines with problems
            if let Some((line, since)) = app.flash {
                let age = (app.now - since) as f32;
                if age < 2.5 {
                    shapes.push(Shape::rect_filled(row_rect(line, left, right), 0.0, pal.accent.gamma_multiply(0.22 * (1.0 - age / 2.5) + 0.06)));
                    ui.ctx().request_repaint();
                } else {
                    shapes.push(Shape::rect_filled(row_rect(line, left, right), 0.0, pal.accent.gamma_multiply(0.06)));
                }
            }
            for d in list {
                if let Some(line) = d.line {
                    let row = row_rect(line, left, right);
                    shapes.push(Shape::rect_filled(row, 0.0, pal.error.gamma_multiply(0.14)));
                    let y = row.bottom() - 1.0;
                    shapes.push(Shape::line_segment([egui::pos2(origin.x, y), egui::pos2(right, y)], egui::Stroke::new(1.5, pal.error)));
                }
            }
            ui.painter().set(marks, Shape::Vec(shapes));

            // the gutter stays at the left edge while the text scrolls sideways
            let strip = Rect::from_min_max(egui::pos2(left, clip.top()), egui::pos2(left + gutter - 6.0, clip.bottom()));
            let painter = ui.painter().with_clip_rect(clip);
            painter.rect_filled(strip, 0.0, pal.panel);
            painter.line_segment([strip.right_top(), strip.right_bottom()], egui::Stroke::new(1.0, pal.faint));
            let first = (((clip.top() - origin.y) / row_height).floor().max(0.0)) as usize;
            let last = ((((clip.bottom() - origin.y) / row_height).ceil()) as usize).min(lines);
            for line in first + 1..=last {
                let y = row_rect(line as u32, left, right).center().y;
                let problem = list.iter().find(|d| d.line == Some(line as u32));
                let color = if problem.is_some() {
                    pal.error
                } else if line == app.cursor.0 {
                    pal.text
                } else {
                    pal.muted
                };
                painter.text(egui::pos2(strip.right() - 8.0, y), egui::Align2::RIGHT_CENTER, line.to_string(), font.clone(), color);
                if let Some(d) = problem {
                    painter.circle_filled(egui::pos2(strip.left() + 7.0, y), 3.5, pal.error);
                    let spot = Rect::from_center_size(egui::pos2(strip.center().x, y), egui::vec2(strip.width(), row_height));
                    ui.interact(spot, id.with(("problem", line)), Sense::hover()).on_hover_text(format!("[{}] {}", d.code, d.message));
                }
            }
        });
    });
}
