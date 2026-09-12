use crate::settings::Panels;

/// Width of the draggable divider between panels.
pub const DIVIDER_WIDTH: f32 = 1.0;

/// Narrowest width a panel can be dragged or squeezed to.
pub const MIN_PANEL_WIDTH: f32 = 200.0;

/// Sidebar, conversation list and reader widths for a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelWidths {
    pub sidebar: f32,
    pub conversations: f32,
    pub reader: f32,
}

/// Applies the persisted split ratios while ensuring that all panes remain
/// usable. The renderer only consumes these dimensions; it does not own them.
pub fn widths(ratios: Panels, window_width: f32) -> PanelWidths {
    let available = (window_width - 2.0 * DIVIDER_WIDTH).max(3.0 * MIN_PANEL_WIDTH);
    let sidebar =
        (available * ratios.sidebar).clamp(MIN_PANEL_WIDTH, available - 2.0 * MIN_PANEL_WIDTH);
    let remaining = available - sidebar;
    let conversations =
        (remaining * ratios.conversations).clamp(MIN_PANEL_WIDTH, remaining - MIN_PANEL_WIDTH);

    PanelWidths {
        sidebar,
        conversations,
        reader: remaining - conversations,
    }
}

/// Converts actual widths back to the stable nested ratios used by the
/// existing settings file.
pub fn ratios(sidebar: f32, conversations: f32, window_width: f32) -> Panels {
    let available = (window_width - 2.0 * DIVIDER_WIDTH).max(1.0);
    let sidebar_ratio = (sidebar / available).clamp(0.1, 0.9);
    let remaining = (available - sidebar).max(1.0);

    Panels {
        sidebar: sidebar_ratio,
        conversations: (conversations / remaining).clamp(0.1, 0.9),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_WINDOW: f32 = 1_100.0;
    const TIGHTEST_WINDOW: f32 = 3.0 * MIN_PANEL_WIDTH + 2.0 * DIVIDER_WIDTH;

    #[test]
    fn default_layout_matches_previous_proportions() {
        let widths = widths(Panels::default(), DEFAULT_WINDOW);

        assert_eq!(widths.sidebar, 219.6);
        assert!(widths.conversations < widths.reader);
        assert_eq!(
            widths.sidebar + widths.conversations + widths.reader + 2.0 * DIVIDER_WIDTH,
            DEFAULT_WINDOW
        );
    }

    #[test]
    fn panels_never_collapse_below_the_minimum() {
        for ratios in [
            Panels {
                sidebar: 0.0,
                conversations: 0.0,
            },
            Panels {
                sidebar: 0.5,
                conversations: 0.5,
            },
            Panels {
                sidebar: 1.0,
                conversations: 1.0,
            },
        ] {
            for window in [TIGHTEST_WINDOW, DEFAULT_WINDOW, 2_560.0] {
                let widths = widths(ratios, window);
                for width in [widths.sidebar, widths.conversations, widths.reader] {
                    assert!(width >= MIN_PANEL_WIDTH, "{width} px in {window} px");
                }
            }
        }
    }

    #[test]
    fn widths_and_ratios_round_trip() {
        let expected = Panels::default();
        let actual = widths(expected, DEFAULT_WINDOW);
        let restored = ratios(actual.sidebar, actual.conversations, DEFAULT_WINDOW);

        assert!((restored.sidebar - expected.sidebar).abs() < f32::EPSILON);
        assert!((restored.conversations - expected.conversations).abs() < f32::EPSILON);
    }

    #[test]
    fn reader_takes_the_extra_space_of_a_larger_window() {
        let normal = widths(Panels::default(), DEFAULT_WINDOW);
        let large = widths(Panels::default(), 2_560.0);

        assert!(large.reader > normal.reader);
    }
}
