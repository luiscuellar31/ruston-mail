//! Puts the main window back where it was left, on a screen still there.

use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};
use winit::dpi::{LogicalPosition, PhysicalPosition};
use winit::window::Window;

use crate::settings;

/// How much of the window's top edge must land on one screen for a saved
/// place to be used: enough to see it and drag the window by it.
const REACHABLE: Vec2 = Vec2::new(100.0, 30.0);

/// The window's top-left corner in the desktop's own coordinates: points on
/// macOS, where screens of different densities share one space of points,
/// and pixels elsewhere. `None` where the system does not say, as on
/// Wayland.
pub(super) fn position(window: &Window) -> Option<[f32; 2]> {
    let position = window.outer_position().ok()?;
    Some(if cfg!(target_os = "macos") {
        let logical = position.to_logical::<f32>(window.scale_factor());
        [logical.x, logical.y]
    } else {
        [position.x as f32, position.y as f32]
    })
}

/// Reopens the window as it was left: at its place, when that is on a
/// connected screen, and then maximized if it was. Done before the window
/// first shows, so it never appears anywhere else.
pub(super) fn restore(window: &Window, saved: settings::Window) {
    if let Some([x, y]) = saved.position
        && reachable(pos2(x, y), outer_width(window), &screens(window))
    {
        if cfg!(target_os = "macos") {
            window.set_outer_position(LogicalPosition::new(x, y));
        } else {
            window.set_outer_position(PhysicalPosition::new(x as i32, y as i32));
        }
    }
    if saved.maximized {
        window.set_maximized(true);
    }
}

fn outer_width(window: &Window) -> f32 {
    let size = window.outer_size();
    if cfg!(target_os = "macos") {
        size.to_logical::<f32>(window.scale_factor()).width
    } else {
        size.width as f32
    }
}

/// Every connected screen, in the coordinates of [`position`].
fn screens(window: &Window) -> Vec<Rect> {
    window
        .available_monitors()
        .map(|screen| {
            let (position, size) = (screen.position(), screen.size());
            if cfg!(target_os = "macos") {
                let scale = screen.scale_factor();
                let position = position.to_logical::<f32>(scale);
                let size = size.to_logical::<f32>(scale);
                Rect::from_min_size(pos2(position.x, position.y), vec2(size.width, size.height))
            } else {
                Rect::from_min_size(
                    pos2(position.x as f32, position.y as f32),
                    vec2(size.width as f32, size.height as f32),
                )
            }
        })
        .collect()
}

/// Whether a window at `position` shows enough of its top edge on one screen
/// to be seen and dragged, so a place on a screen that has since been
/// disconnected is not used.
fn reachable(position: Pos2, width: f32, screens: &[Rect]) -> bool {
    let top = Rect::from_min_size(position, vec2(width, REACHABLE.y));
    screens.iter().any(|screen| {
        let shared = screen.intersect(top);
        shared.width() >= REACHABLE.x && shared.height() >= REACHABLE.y
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_place_is_used_only_while_its_screen_is_connected() {
        let laptop = Rect::from_min_size(pos2(0.0, 0.0), vec2(1512.0, 982.0));
        let external = Rect::from_min_size(pos2(1512.0, -200.0), vec2(2560.0, 1440.0));
        let width = 1100.0;

        assert!(reachable(pos2(200.0, 120.0), width, &[laptop]));
        // A window left on the external screen, with that screen still there
        // and once it is gone.
        assert!(reachable(pos2(2000.0, 0.0), width, &[laptop, external]));
        assert!(!reachable(pos2(2000.0, 0.0), width, &[laptop]));
        // Mostly past the left edge, it can still be caught by its top.
        assert!(reachable(pos2(-950.0, 300.0), width, &[laptop]));
        // Its top above every screen, it could not be dragged back.
        assert!(!reachable(pos2(200.0, -20.0), width, &[laptop]));
        assert!(!reachable(pos2(200.0, 120.0), width, &[]));
    }
}
