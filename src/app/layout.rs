use iced::widget::pane_grid::{self, Axis, Configuration};

/// The mailbox panels, left to right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Sidebar,
    Conversations,
    Reader,
}

/// Width of the draggable divider between panels.
pub const DIVIDER_WIDTH: f32 = 1.0;

/// Extra grab area around each divider, split evenly on both sides.
pub const DIVIDER_GRAB: f32 = 8.0;

/// Narrowest width a panel can be dragged or squeezed to. Iced enforces it
/// when laying out, so stored split ratios never produce a smaller panel.
pub const MIN_PANEL_WIDTH: f32 = 200.0;

/// Default split positions, matching the previous fixed layout at the default
/// window width: a 220 px sidebar out of 1100 px, with the rest split 3:4
/// between the conversation list and the reader.
const SIDEBAR_RATIO: f32 = 0.2;
const CONVERSATIONS_RATIO: f32 = 3.0 / 7.0;

pub fn default_panels() -> pane_grid::State<Panel> {
    pane_grid::State::with_configuration(Configuration::Split {
        axis: Axis::Vertical,
        ratio: SIDEBAR_RATIO,
        a: Box::new(Configuration::Pane(Panel::Sidebar)),
        b: Box::new(Configuration::Split {
            axis: Axis::Vertical,
            ratio: CONVERSATIONS_RATIO,
            a: Box::new(Configuration::Pane(Panel::Conversations)),
            b: Box::new(Configuration::Pane(Panel::Reader)),
        }),
    })
}

#[cfg(test)]
mod tests {
    use iced::Size;

    use super::*;

    const DEFAULT_WINDOW: Size = Size::new(1_100.0, 700.0);
    /// The narrowest window that fits every panel at its minimum width.
    const TIGHTEST_WINDOW: Size = Size::new(3.0 * MIN_PANEL_WIDTH + 2.0 * DIVIDER_WIDTH, 480.0);
    const LARGE_WINDOW: Size = Size::new(2_560.0, 1_440.0);

    /// Sidebar, conversation list and reader widths for a window size.
    fn widths(panels: &pane_grid::State<Panel>, window: Size) -> [f32; 3] {
        let regions = panels
            .layout()
            .pane_regions(DIVIDER_WIDTH, MIN_PANEL_WIDTH, window);
        let width = |panel| {
            panels
                .iter()
                .find(|(_, kind)| **kind == panel)
                .map(|(pane, _)| regions[pane].width)
                .unwrap()
        };

        [
            width(Panel::Sidebar),
            width(Panel::Conversations),
            width(Panel::Reader),
        ]
    }

    fn resize_every_split(panels: &mut pane_grid::State<Panel>, ratio: f32) {
        let splits: Vec<_> = panels.layout().splits().copied().collect();
        for split in splits {
            panels.resize(split, ratio);
        }
    }

    #[test]
    fn default_layout_matches_previous_proportions() {
        let panels = default_panels();
        let [sidebar, conversations, reader] = widths(&panels, DEFAULT_WINDOW);

        assert_eq!(panels.len(), 3);
        assert_eq!(sidebar, 220.0);
        assert!(conversations < reader);
        assert_eq!(
            sidebar + conversations + reader + 2.0 * DIVIDER_WIDTH,
            DEFAULT_WINDOW.width
        );
    }

    #[test]
    fn panels_never_collapse_below_the_minimum() {
        for ratio in [0.0, 0.5, 1.0] {
            let mut panels = default_panels();
            resize_every_split(&mut panels, ratio);

            for window in [TIGHTEST_WINDOW, DEFAULT_WINDOW, LARGE_WINDOW] {
                for width in widths(&panels, window) {
                    assert!(
                        width >= MIN_PANEL_WIDTH,
                        "{width} px at ratio {ratio} in {window:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn reader_takes_the_extra_space_of_a_larger_window() {
        let panels = default_panels();
        let [_, _, default_reader] = widths(&panels, DEFAULT_WINDOW);
        let [_, _, large_reader] = widths(&panels, LARGE_WINDOW);

        assert!(large_reader > default_reader);
    }
}
