//! Several plates in one project: each has its own objects and is sliced, exported and
//! sent on its own. The plate being worked on lives in the app's `objects` (so the rest
//! of the app is unchanged); the others wait in `plates`, and picking a tab above the 3D
//! view swaps them. Each plate keeps its slice result.
//!
//! Undo and redo cover the plate being worked on: switching plates starts the history
//! again.

use super::*;

/// A plate that isn't the one being worked on: its objects and its last slice.
#[derive(Default)]
pub(super) struct StoredPlate {
    pub(super) objects: Vec<PlacedObject>,
    sliced: Option<Sliced>,
}

impl PrintCraftApp {
    /// Every plate's objects, in order (the one being worked on from `objects`).
    pub(super) fn all_plates(&self) -> Vec<Vec<PlacedObject>> {
        (0..self.plates.len())
            .map(|i| {
                if i == self.active_plate {
                    self.objects.clone()
                } else {
                    self.plates[i].objects.clone()
                }
            })
            .collect()
    }

    /// Whether plate `i` has nothing on it.
    fn plate_is_empty(&self, i: usize) -> bool {
        if i == self.active_plate {
            self.objects.is_empty()
        } else {
            self.plates.get(i).is_none_or(|p| p.objects.is_empty())
        }
    }

    /// Whether any plate has something on it.
    pub(super) fn any_plate_has_objects(&self) -> bool {
        (0..self.plates.len()).any(|i| !self.plate_is_empty(i))
    }

    /// Makes plate `i` the one being worked on.
    pub(super) fn switch_plate(&mut self, i: usize) {
        if i == self.active_plate || i >= self.plates.len() {
            return;
        }
        self.cancel_slice();
        self.plates[self.active_plate] = StoredPlate {
            objects: std::mem::take(&mut self.objects),
            sliced: self.sliced.take(),
        };
        let next = std::mem::take(&mut self.plates[i]);
        self.objects = next.objects;
        self.sliced = next.sliced;
        self.active_plate = i;
        self.refresh_plate();
        self.selected = None;
        self.drag = Drag::None;
        self.face_hover = None;
        self.reset_history();
        self.status = format!("Plate {}.", i + 1);
    }

    /// Adds an empty plate after the others and switches to it.
    pub(super) fn add_plate(&mut self) {
        self.plates.push(StoredPlate::default());
        self.switch_plate(self.plates.len() - 1);
    }

    /// Removes plate `i` when it is empty and not the only one; says why not otherwise.
    pub(super) fn remove_plate(&mut self, i: usize) {
        if self.plates.len() < 2 || i >= self.plates.len() {
            return;
        }
        if !self.plate_is_empty(i) {
            self.status = format!("Clear plate {} first to remove it.", i + 1);
            return;
        }
        if i == self.active_plate {
            self.switch_plate(if i > 0 { i - 1 } else { 1 });
        }
        self.plates.remove(i);
        if self.active_plate > i {
            self.active_plate -= 1;
        }
        self.status = format!("Removed plate {}.", i + 1);
    }

    /// Puts a project's plates in place, with the first one being worked on (its
    /// objects are already in `objects`).
    pub(super) fn set_more_plates(&mut self, more: Vec<Vec<PlacedObject>>) {
        self.plates = std::iter::once(StoredPlate::default())
            .chain(more.into_iter().map(|objects| StoredPlate {
                objects,
                sliced: None,
            }))
            .collect();
        self.active_plate = 0;
    }

    /// The other plates' objects, for noticing unsaved changes.
    pub(super) fn other_plates_fingerprint(&self) -> String {
        self.plates
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.active_plate)
            .map(|(i, p)| {
                let objects: String = p.objects.iter().map(object_fingerprint).collect();
                format!("#{i}{objects}")
            })
            .collect()
    }

    /// The tabs above the 3D view: one per plate (with its number of objects), a "+"
    /// to add one, and "×" on an empty plate to remove it.
    pub(super) fn plate_tabs(&mut self, ui: &mut Ui) {
        let mut pick = None;
        let mut remove = None;
        let mut add = false;
        ui.horizontal(|ui| {
            for i in 0..self.plates.len() {
                let count = if i == self.active_plate {
                    self.objects.len()
                } else {
                    self.plates[i].objects.len()
                };
                let label = match count {
                    0 => format!("Plate {}", i + 1),
                    n => format!("Plate {} ({n})", i + 1),
                };
                let on = i == self.active_plate;
                if ui.selectable_label(on, label).clicked() {
                    pick = Some(i);
                }
                if on && self.plates.len() > 1 {
                    let empty = count == 0;
                    let r = ui
                        .add_enabled(empty, egui::Button::new("×").small())
                        .on_hover_text("Remove this plate")
                        .on_disabled_hover_text("Clear the plate first to remove it");
                    if r.clicked() {
                        remove = Some(i);
                    }
                }
            }
            if ui
                .small_button("+")
                .on_hover_text("Add a plate: each plate is sliced and printed on its own")
                .clicked()
            {
                add = true;
            }
        });
        if let Some(i) = pick {
            self.switch_plate(i);
        }
        if let Some(i) = remove {
            self.remove_plate(i);
        }
        if add {
            self.add_plate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Mesh;

    fn cube(name: &str) -> PlacedObject {
        PlacedObject::new(name, Mesh::cube(Vec3::new(10.0, 10.0, 10.0)))
    }

    fn names(objects: &[PlacedObject]) -> Vec<String> {
        objects.iter().map(|o| o.name.clone()).collect()
    }

    #[test]
    fn test_plates_keep_their_own_objects() {
        let mut app = PrintCraftApp::for_tests();
        assert_eq!(app.plates.len(), 1);
        app.add_object(cube("a"));
        app.add_plate();
        assert_eq!((app.plates.len(), app.active_plate), (2, 1));
        assert!(app.objects.is_empty(), "a new plate starts empty");
        app.add_object(cube("b"));
        app.add_object(cube("c"));
        app.add_plate();
        app.add_object(cube("d"));
        app.switch_plate(0);
        assert_eq!(names(&app.objects), ["a"]);
        assert_eq!(
            app.plate.len(),
            1,
            "the plate's triangles follow its objects"
        );
        let all: Vec<Vec<String>> = app.all_plates().iter().map(|p| names(p)).collect();
        assert_eq!(all, [vec!["a"], vec!["b", "c"], vec!["d"]]);
        // The project holds all three, plate 1 first; opening it gives them back.
        let p = app.current_project();
        assert_eq!(names(&p.objects), ["a"]);
        assert_eq!(p.more_plates.len(), 2);
        let mut other = PrintCraftApp::for_tests();
        other.apply_project(p);
        let all: Vec<Vec<String>> = other.all_plates().iter().map(|p| names(p)).collect();
        assert_eq!(all, [vec!["a"], vec!["b", "c"], vec!["d"]]);
        assert_eq!(other.active_plate, 0);
    }

    #[test]
    fn test_plates_changes_on_another_plate_are_unsaved_changes() {
        let mut app = PrintCraftApp::for_tests();
        app.add_object(cube("a"));
        app.add_plate();
        app.add_object(cube("b"));
        app.switch_plate(0);
        app.saved_fingerprint = app.project_fingerprint();
        assert!(!app.has_unsaved_changes());
        app.plates[1].objects[0].x += 5.0;
        assert!(app.has_unsaved_changes());
    }

    #[test]
    fn test_plates_only_empty_plates_are_removed() {
        let mut app = PrintCraftApp::for_tests();
        app.add_object(cube("a"));
        app.add_plate();
        app.add_object(cube("b"));
        app.add_plate();
        // Plate 3 is empty and being worked on: it goes, and plate 2 is shown.
        app.remove_plate(2);
        assert_eq!((app.plates.len(), app.active_plate), (2, 1));
        assert_eq!(names(&app.objects), ["b"]);
        // Plate 1 has an object: it stays, with a message.
        app.remove_plate(0);
        assert_eq!(app.plates.len(), 2);
        assert!(app.status.contains("Clear plate 1"), "{}", app.status);
        // Emptied, it goes; the plate being worked on keeps its place.
        app.plates[0].objects.clear();
        app.remove_plate(0);
        assert_eq!((app.plates.len(), app.active_plate), (1, 0));
        assert_eq!(names(&app.objects), ["b"]);
        // The last plate never goes.
        app.objects.clear();
        app.remove_plate(0);
        assert_eq!(app.plates.len(), 1);
    }
}
