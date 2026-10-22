// Yapfast: not part of upstream Slint. See docs/building.md in Yapfast.

//! Colouring parts of a `TextInput`'s text, as the composer shows the people its message
//! mentions: each part in its colour on a translucent pill of it, as a `StyledText` draws a
//! mention.
//!
//! The marks hold for one text. A `TextInput` showing any other, such as the next keystroke's
//! before the application marks it again, or one composing with an input method, draws plain.

use alloc::boxed::Box;
use alloc::rc::Rc;
use core::ops::Range;
use core::pin::Pin;

use crate::Property;
use crate::SharedString;
use crate::graphics::Color;

/// Byte ranges of a text, each with its colour.
pub type Marks = Rc<[(Range<usize>, Color)]>;

#[derive(Clone, Default, PartialEq)]
struct Marked {
    text: SharedString,
    marks: Option<Marks>,
}

std::thread_local! {
    /// The text marked and its marks. The text inputs that shape while reading it shape again
    /// when it changes.
    static MARKED: Pin<Box<Property<Marked>>> = Box::pin(Property::new(Marked::default()));
}

/// Marks byte ranges of `text` with colours, in place of the marks before. Empty marks clear
/// them.
pub fn mark(text: SharedString, marks: impl IntoIterator<Item = (Range<usize>, Color)>) {
    let marks: Marks = marks.into_iter().filter(|(range, _)| !range.is_empty()).collect();
    let marked = if marks.is_empty() {
        Marked::default()
    } else {
        Marked { text, marks: Some(marks) }
    };
    MARKED.with(|property| property.as_ref().set(marked));
}

/// The marks of `text`, if it's the text marked.
pub fn marks_of(text: &str) -> Option<Marks> {
    MARKED.with(|property| {
        let marked = property.as_ref().get();
        (marked.text == text).then_some(marked.marks).flatten()
    })
}
