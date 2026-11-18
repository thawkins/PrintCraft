//! Line icons for the selection tool strip (Move, Scale, Rotate, Face), drawn with the
//! painter so they stay sharp at any size and need no image files. Original drawings using
//! common symbols: a four-way arrow, a small box growing into a large one, a curved arrow,
//! and a tilted face with an arrow down to a plate line.

use crate::gizmo::Tool;
use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke, Vec2};

/// Draws `tool`'s icon centred in `rect`.
pub fn draw(painter: &egui::Painter, rect: Rect, tool: Tool, color: Color32) {
    let s = rect.width().min(rect.height());
    let c = rect.center();
    let stroke = Stroke::new((s * 0.06).max(1.5), color);
    let head = s * 0.16;
    match tool {
        Tool::Move => {
            let l = s * 0.36;
            for d in [
                Vec2::new(1.0, 0.0),
                Vec2::new(-1.0, 0.0),
                Vec2::new(0.0, 1.0),
                Vec2::new(0.0, -1.0),
            ] {
                let tip = c + d * l;
                painter.line_segment([c, tip - d * head * 0.8], stroke);
                arrow_head(painter, tip, d, head, color);
            }
            painter.rect_filled(Rect::from_center_size(c, Vec2::splat(s * 0.14)), 1.0, color);
        }
        Tool::Scale => {
            let outer = Rect::from_center_size(c, Vec2::splat(s * 0.7));
            dashed_rect(painter, outer, stroke);
            let inner = Rect::from_min_size(
                Pos2::new(outer.left(), outer.bottom() - s * 0.32),
                Vec2::splat(s * 0.32),
            );
            painter.rect_stroke(inner, 1.0, stroke, egui::StrokeKind::Middle);
            let from = inner.right_top() + Vec2::new(s * 0.06, -s * 0.06);
            let tip = outer.right_top() + Vec2::new(-s * 0.04, s * 0.04);
            let d = (tip - from).normalized();
            painter.line_segment([from, tip - d * head * 0.8], stroke);
            arrow_head(painter, tip, d, head, color);
        }
        Tool::Rotate => {
            let r = s * 0.3;
            // Three quarters of a circle, opening at the top right, ending in an arrow.
            let (start, end) = (-0.15_f32, -0.15 + 1.5 * std::f32::consts::PI);
            let at = |a: f32| c + Vec2::new(a.cos(), -a.sin()) * r;
            let pts: Vec<Pos2> = (0..=32)
                .map(|i| at(start + (end - start) * i as f32 / 32.0))
                .collect();
            painter.add(Shape::line(pts, stroke));
            // Tangent direction at the end, going the way the arc was drawn (anticlockwise).
            let d = Vec2::new(-end.sin(), -end.cos());
            arrow_head(painter, at(end) + d * head * 0.6, d, head, color);
            painter.circle_filled(c, s * 0.06, color);
        }
        Tool::Face => {
            // A tilted face with an arrow down to the plate line.
            let face = [
                c + Vec2::new(-s * 0.32, -s * 0.12),
                c + Vec2::new(s * 0.08, -s * 0.36),
                c + Vec2::new(s * 0.34, -s * 0.16),
                c + Vec2::new(-s * 0.06, s * 0.08),
            ];
            painter.add(Shape::closed_line(face.to_vec(), stroke));
            let plate = c.y + s * 0.36;
            painter.line_segment(
                [
                    Pos2::new(c.x - s * 0.36, plate),
                    Pos2::new(c.x + s * 0.36, plate),
                ],
                stroke,
            );
            let d = Vec2::new(0.0, 1.0);
            let tip = Pos2::new(c.x + s * 0.02, plate - s * 0.06);
            painter.line_segment(
                [Pos2::new(tip.x, c.y + s * 0.02), tip - d * head * 0.8],
                stroke,
            );
            arrow_head(painter, tip, d, head, color);
        }
    }
}

fn arrow_head(painter: &egui::Painter, tip: Pos2, dir: Vec2, size: f32, color: Color32) {
    let side = Vec2::new(-dir.y, dir.x) * size * 0.6;
    let back = tip - dir * size;
    painter.add(Shape::convex_polygon(
        vec![tip, back + side, back - side],
        color,
        Stroke::NONE,
    ));
}

fn dashed_rect(painter: &egui::Painter, r: Rect, stroke: Stroke) {
    let pts = vec![
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    let dash = r.width() / 7.0;
    painter.extend(Shape::dashed_line(&pts, stroke, dash, dash * 0.6));
}
