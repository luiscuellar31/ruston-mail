//! The sender's margins, padding and borders, kept only as what the readers
//! use of them: whether a block has room above or below it, and whether a
//! line runs above or below it. Each comes out as one of a few fixed
//! declarations, whatever the sender wrote, so a negative margin cannot pull
//! a quoted message over the reply around it, wherever the sanitized HTML
//! goes.

use cssparser::{Delimiter, ParseError, Parser, ParserInput, Token};

/// What a style says of the room and the lines around its block: above,
/// then below.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Around {
    /// Whether there is room: `Some(true)` where a margin or padding makes
    /// some, `Some(false)` where a margin says there is none, `None` where
    /// the sender said nothing.
    pub(super) room: [Option<bool>; 2],
    /// Whether a line runs across the block there.
    pub(super) line: [bool; 2],
}

/// What `style` says of the room and the lines around its block, whether it
/// is a sender's own or what [`normalize`] made of one.
pub(super) fn around(style: &str) -> Around {
    read(style).0
}

/// `style`, the value of a `style` attribute, with every margin, padding and
/// border declaration replaced by the normalized ones: `margin-top` and
/// `margin-bottom` of `0` or `1em`, and `border-top` and `border-bottom` of
/// `1px solid`. The rest is kept as written, for the sanitizer to filter.
pub(super) fn normalize(style: &str) -> String {
    let (around, kept) = read(style);
    let room = ["top", "bottom"]
        .into_iter()
        .zip(around.room)
        .filter_map(|(side, room)| {
            room.map(|room| format!("margin-{side}:{}", if room { "1em" } else { "0" }))
        });
    let lines = ["top", "bottom"]
        .into_iter()
        .zip(around.line)
        .filter(|(_, line)| *line)
        .map(|(side, _)| format!("border-{side}:1px solid"));
    // The normalized ones come first: a declaration left open at the end, as
    // an unclosed string is, cannot take them in.
    room.chain(lines).chain(kept).collect::<Vec<_>>().join(";")
}

/// The room and lines `style` gives, and its other declarations as written.
///
/// It reads the declarations as ammonia does, with the same parser, so a
/// comment or an escape cannot hide a declaration from it that ammonia would
/// then keep.
fn read(style: &str) -> (Around, Vec<String>) {
    let mut input = ParserInput::new(style);
    let mut parser = Parser::new(&mut input);
    let mut room = Room::default();
    let mut lines = Lines::default();
    let mut kept = Vec::new();
    while !parser.is_exhausted() {
        let start = parser.position();
        let declaration = parser.parse_until_after(Delimiter::Semicolon, |parser| {
            let name = parser.expect_ident()?.to_ascii_lowercase();
            parser.expect_colon()?;
            // Every token is read, or the declaration is refused; what
            // follows `!` is `important`, which changes nothing here.
            let mut values = Vec::new();
            let mut important = false;
            while let Ok(token) = parser.next() {
                important |= matches!(token, Token::Delim('!'));
                if !important {
                    values.push(token.clone());
                }
            }
            Ok::<_, ParseError<'_, ()>>((name, values))
        });
        let Ok((name, values)) = declaration else {
            continue;
        };
        if name.starts_with("margin") || name.starts_with("padding") {
            room.read(&name, &values);
        } else if name.starts_with("border") {
            lines.read(&name, &values);
        } else {
            let written = parser.slice_from(start).trim().trim_end_matches(';');
            kept.push(written.trim().to_owned());
        }
    }
    let around = Around {
        room: room.room(),
        line: lines.across(),
    };
    (around, kept)
}

/// Whether a length makes room: a positive one does; zero, a negative one
/// and `auto` do not. `None` for what is not a length, as `calc()` is.
fn makes_room(token: &Token) -> Option<bool> {
    match *token {
        Token::Number { value, .. } | Token::Dimension { value, .. } => Some(value > 0.0),
        Token::Percentage { unit_value, .. } => Some(unit_value > 0.0),
        Token::Ident(ref name) if name.eq_ignore_ascii_case("auto") => Some(false),
        _ => None,
    }
}

/// The top and bottom of a box from a shorthand's one to four values: top,
/// right, bottom, left, the missing ones taken from their opposites.
fn top_and_bottom<T: Copy>(values: &[T]) -> Option<(T, T)> {
    match values {
        [all] | [all, _] => Some((*all, *all)),
        [top, _, bottom] | [top, _, bottom, _] => Some((*top, *bottom)),
        _ => None,
    }
}

/// Room above and below, from margins and padding alike: either puts space
/// between a block and the one beside it.
#[derive(Default)]
struct Room {
    margin: [Option<bool>; 2],
    padding: [Option<bool>; 2],
}

impl Room {
    fn read(&mut self, name: &str, values: &[Token]) {
        let lengths: Vec<Option<bool>> = values.iter().map(makes_room).collect();
        let (kind, side) = match name.split_once('-') {
            Some((kind, side)) => (kind, Some(side)),
            None => (name, None),
        };
        let sides = match kind {
            "margin" => &mut self.margin,
            "padding" => &mut self.padding,
            _ => return,
        };
        match (side, &lengths[..]) {
            (None, _) => {
                if let Some((top, bottom)) = top_and_bottom(&lengths) {
                    *sides = [top, bottom];
                }
            }
            (Some("top"), [one]) => sides[0] = *one,
            (Some("bottom"), [one]) => sides[1] = *one,
            _ => {}
        }
    }

    /// Above and below: room where a margin or padding makes some, none
    /// where a margin says so, and `None` where the sender said nothing.
    fn room(&self) -> [Option<bool>; 2] {
        [0, 1].map(|side| {
            if self.margin[side] == Some(true) || self.padding[side] == Some(true) {
                Some(true)
            } else if self.margin[side] == Some(false) {
                Some(false)
            } else {
                None
            }
        })
    }
}

/// One side of a border: whether it has width, and whether its style draws
/// it. `None` is what the sender did not say.
#[derive(Clone, Copy, Default)]
struct Side {
    width: Option<bool>,
    drawn: Option<bool>,
}

impl Side {
    /// Whether it shows: drawn, and of a width, which is `medium` unless
    /// the sender said otherwise.
    fn shows(self) -> bool {
        self.drawn == Some(true) && self.width != Some(false)
    }
}

/// The four sides of a border, top, right, bottom and left.
#[derive(Default)]
struct Lines([Side; 4]);

/// Whether a border style draws a line; `None` for what is not a style.
fn draws(token: &Token) -> Option<bool> {
    let Token::Ident(name) = token else {
        return None;
    };
    match name.to_ascii_lowercase().as_str() {
        "none" | "hidden" => Some(false),
        "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => {
            Some(true)
        }
        _ => None,
    }
}

/// Whether a border width is more than none; `None` for what is not one.
fn wide(token: &Token) -> Option<bool> {
    match token {
        Token::Ident(name)
            if matches!(&*name.to_ascii_lowercase(), "thin" | "medium" | "thick") =>
        {
            Some(true)
        }
        Token::Number { value, .. } | Token::Dimension { value, .. } => Some(*value > 0.0),
        _ => None,
    }
}

impl Lines {
    fn read(&mut self, name: &str, values: &[Token]) {
        let sides: &[usize] = match name {
            "border" | "border-width" | "border-style" => &[0, 1, 2, 3],
            "border-top" | "border-top-width" | "border-top-style" => &[0],
            "border-right" | "border-right-width" | "border-right-style" => &[1],
            "border-bottom" | "border-bottom-width" | "border-bottom-style" => &[2],
            "border-left" | "border-left-width" | "border-left-style" => &[3],
            // Colors, corners and the rest change no line.
            _ => return,
        };
        if name.ends_with("-width") {
            let widths: Vec<Option<bool>> = values.iter().map(wide).collect();
            self.set(sides, &widths, |side, width| side.width = width);
        } else if name.ends_with("-style") {
            let styles: Vec<Option<bool>> = values.iter().map(draws).collect();
            self.set(sides, &styles, |side, drawn| side.drawn = drawn);
        } else {
            // A shorthand sets what it leaves out to its initial value: a
            // medium width, and no style.
            let side = Side {
                width: Some(values.iter().find_map(wide).unwrap_or(true)),
                drawn: Some(values.iter().find_map(draws).unwrap_or(false)),
            };
            for index in sides {
                self.0[*index] = side;
            }
        }
    }

    /// Sets `sides` from a longhand's values: one for each side named, or,
    /// for all four, one to four values as a shorthand gives them.
    fn set(
        &mut self,
        sides: &[usize],
        values: &[Option<bool>],
        apply: fn(&mut Side, Option<bool>),
    ) {
        match (sides, values) {
            ([one], [value]) => apply(&mut self.0[*one], *value),
            ([_, _, _, _], _) => {
                let four = match values {
                    [all] => [*all; 4],
                    [vertical, horizontal] => [*vertical, *horizontal, *vertical, *horizontal],
                    [top, horizontal, bottom] => [*top, *horizontal, *bottom, *horizontal],
                    [top, right, bottom, left] => [*top, *right, *bottom, *left],
                    _ => return,
                };
                for (side, value) in self.0.iter_mut().zip(four) {
                    apply(side, value);
                }
            }
            _ => {}
        }
    }

    /// Above and below, whether a line runs across the block: one that
    /// shows, on a block with no line down either side, which would make it
    /// a box, as a button or a card is.
    fn across(&self) -> [bool; 2] {
        let [top, right, bottom, left] = self.0;
        let across = !right.shows() && !left.shows();
        [across && top.shows(), across && bottom.shows()]
    }
}

#[cfg(test)]
mod tests;
