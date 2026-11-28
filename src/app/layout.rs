use crate::settings::{MAX_RATIO, MIN_RATIO, Panels};

/// Width of the draggable divider between panels.
pub const DIVIDER_WIDTH: f32 = 1.0;

/// Narrowest width a panel can be dragged or squeezed to.
pub const MIN_PANEL_WIDTH: f32 = 200.0;

/// Narrowest width the reader can be dragged or squeezed to: room for its
/// toolbar on one line, with the label menu, and for the names and dates
/// over a message, at the usual text size.
pub const MIN_READER_WIDTH: f32 = 410.0;

/// The narrowest window that shows the three panes side by side, beside a
/// sidebar `sidebar` points wide at its narrowest: folded into its strip, it
/// leaves the columns room down to a narrower window.
pub fn min_columns_width(sidebar: f32) -> f32 {
    sidebar + MIN_PANEL_WIDTH + MIN_READER_WIDTH + 2.0 * DIVIDER_WIDTH
}

/// Switch to one pane when the columns and dividers cannot fit beside a
/// sidebar `sidebar` points wide at its narrowest.
pub fn compact(window_width: f32, sidebar: f32) -> bool {
    window_width < min_columns_width(sidebar)
}

/// Sidebar, conversation list and reader widths for a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelWidths {
    pub sidebar: f32,
    pub conversations: f32,
    pub reader: f32,
}

/// Applies the persisted split ratios while ensuring that all panes remain
/// usable in the three-pane layout. Below its minimum, the renderer must use
/// the compact layout instead of placing these columns outside the viewport.
pub fn widths(ratios: Panels, window_width: f32) -> PanelWidths {
    let available =
        (window_width - 2.0 * DIVIDER_WIDTH).max(2.0 * MIN_PANEL_WIDTH + MIN_READER_WIDTH);
    let sidebar = (available * ratios.sidebar).clamp(
        MIN_PANEL_WIDTH,
        available - MIN_PANEL_WIDTH - MIN_READER_WIDTH,
    );
    let remaining = available - sidebar;
    let conversations =
        (remaining * ratios.conversations).clamp(MIN_PANEL_WIDTH, remaining - MIN_READER_WIDTH);

    PanelWidths {
        sidebar,
        conversations,
        reader: remaining - conversations,
    }
}

/// Sidebar, conversation list and reader widths with the sidebar folded into
/// a rail of `rail` points: the list keeps its share of what is left.
pub fn widths_beside_rail(ratios: Panels, window_width: f32, rail: f32) -> PanelWidths {
    let remaining =
        (window_width - 2.0 * DIVIDER_WIDTH - rail).max(MIN_PANEL_WIDTH + MIN_READER_WIDTH);
    let conversations =
        (remaining * ratios.conversations).clamp(MIN_PANEL_WIDTH, remaining - MIN_READER_WIDTH);

    PanelWidths {
        sidebar: rail,
        conversations,
        reader: remaining - conversations,
    }
}

/// Converts actual widths back to the stable nested ratios used by the
/// existing settings file.
pub fn ratios(sidebar: f32, conversations: f32, window_width: f32) -> Panels {
    let available = (window_width - 2.0 * DIVIDER_WIDTH).max(1.0);
    let sidebar_ratio = (sidebar / available).clamp(MIN_RATIO, MAX_RATIO);
    let remaining = (available - sidebar).max(1.0);

    Panels {
        sidebar: sidebar_ratio,
        conversations: (conversations / remaining).clamp(MIN_RATIO, MAX_RATIO),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_WINDOW: f32 = 1_100.0;

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
            for window in [min_columns_width(MIN_PANEL_WIDTH), DEFAULT_WINDOW, 2_560.0] {
                let widths = widths(ratios, window);
                for width in [widths.sidebar, widths.conversations] {
                    assert!(width >= MIN_PANEL_WIDTH, "{width} px in {window} px");
                }
                assert!(
                    widths.reader >= MIN_READER_WIDTH,
                    "{widths:?} in {window} px"
                );
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
    fn beside_a_folded_sidebar_the_list_keeps_its_share_of_the_rest() {
        let rail = 51.0;
        let shares = Panels::default();
        for window in [min_columns_width(rail), DEFAULT_WINDOW, 2_560.0] {
            let widths = widths_beside_rail(shares, window, rail);
            assert_eq!(widths.sidebar, rail);
            assert!(widths.conversations >= MIN_PANEL_WIDTH, "{window}");
            assert!(widths.reader >= MIN_READER_WIDTH, "{window}");
            assert_eq!(
                widths.sidebar + widths.conversations + widths.reader + 2.0 * DIVIDER_WIDTH,
                window
            );
            // The share the list is saved with reads back the same, where
            // the reader's room leaves it whole.
            if window == min_columns_width(rail) {
                continue;
            }
            let saved = ratios(rail, widths.conversations, window);
            assert!(
                (saved.conversations - shares.conversations).abs() < 1e-6,
                "{window}"
            );
        }
    }

    #[test]
    fn reader_takes_the_extra_space_of_a_larger_window() {
        let normal = widths(Panels::default(), DEFAULT_WINDOW);
        let large = widths(Panels::default(), 2_560.0);

        assert!(large.reader > normal.reader);
    }

    #[test]
    fn compact_layout_tracks_usable_points_at_every_supported_zoom() {
        let narrowest = min_columns_width(MIN_PANEL_WIDTH);
        for zoom in [1.0, 1.15, 1.3, 1.5, 1.75, 2.0] {
            let available = 820.0 / zoom;
            assert_eq!(compact(available, MIN_PANEL_WIDTH), available < narrowest);
        }
        assert!(compact(narrowest - 1.0, MIN_PANEL_WIDTH));
        assert!(!compact(narrowest, MIN_PANEL_WIDTH));
    }

    #[test]
    fn a_folded_sidebar_keeps_the_columns_down_to_a_narrower_window() {
        let rail = 51.0;
        // Too narrow for the sidebar, the list and a reader its toolbar fits.
        assert!(compact(700.0, MIN_PANEL_WIDTH));
        assert!(!compact(700.0, rail));
        let widths = widths_beside_rail(Panels::default(), 700.0, rail);
        assert!(widths.reader >= MIN_READER_WIDTH, "{widths:?}");
        assert!(compact(min_columns_width(rail) - 1.0, rail));
    }
}
