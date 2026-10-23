//! Out-of-band data attached to buffer cells, such as hyperlinks.
//!
//! This module is only available with the `cell-extensions` feature.
//!
//! A [`Cell`](crate::buffer::Cell) holds one grapheme cluster and its style. Some terminal
//! features don't fit that model because they are written as escape sequences *around* the
//! symbol rather than as part of it. [OSC 8 hyperlinks] are the main example: the link has to be
//! opened before the first linked cell is printed and closed after the last one.
//!
//! Extensions store that data next to the symbol, in a [`CellExt`]. Backends write it around the
//! symbol as run state, the same way they handle colors, so it never affects the symbol's text or
//! width. Extensions take part in cell equality, so a cell whose only change is its extension is
//! redrawn by the next diff.
//!
//! [OSC 8 hyperlinks]: https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda

// Extensions are shared between cells with `Arc`, which needs atomic pointers. Fail with a clear
// message instead of an unresolved import on targets without them (e.g. thumbv6m-none-eabi).
#[cfg(not(target_has_atomic = "ptr"))]
compile_error!(
    "the `cell-extensions` feature requires a target with atomic pointer support \
     (`target_has_atomic = \"ptr\"`)"
);

#[cfg(target_has_atomic = "ptr")]
use alloc::sync::Arc;
use core::fmt;
use core::hash::{Hash, Hasher};

use compact_str::CompactString;

/// An [OSC 8] terminal hyperlink.
///
/// A hyperlink has a target URI and an optional `id`. Terminals use the `id` to treat cells that
/// are not printed in one contiguous run, such as a link that wraps onto a second line or a link
/// redrawn by a partial diff, as a single link for hover highlighting.
///
/// Both values are sanitized on construction so they can't inject escape sequences: control
/// characters (C0, DEL and C1) are removed from the URI and the `id`, and `;` and `:` (the OSC 8
/// parameter separators) are also removed from the `id`.
///
/// Attach a hyperlink to a cell with [`Cell::set_hyperlink`].
///
/// # Example
///
/// ```
/// use ratatui_core::buffer::Hyperlink;
///
/// let link = Hyperlink::new("https://ratatui.rs").with_id("docs");
/// assert_eq!(link.uri(), "https://ratatui.rs");
/// assert_eq!(link.id(), Some("docs"));
/// ```
///
/// [OSC 8]: https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda
/// [`Cell::set_hyperlink`]: crate::buffer::Cell::set_hyperlink
#[derive(Clone, Eq)]
pub struct Hyperlink {
    uri: CompactString,
    id: Option<CompactString>,
}

impl Hyperlink {
    /// Creates a hyperlink to the given URI, removing any control characters from it.
    ///
    /// The URI is written to the terminal as is (minus control characters). Per the OSC 8
    /// specification it should be a URI with any characters outside printable ASCII
    /// percent-encoded.
    pub fn new<T: AsRef<str>>(uri: T) -> Self {
        Self {
            uri: sanitize(uri.as_ref(), char::is_control),
            id: None,
        }
    }

    /// Sets the `id` parameter of the hyperlink.
    ///
    /// Control characters, `;` and `:` are removed. An `id` that is empty after sanitizing is
    /// treated as no `id`.
    #[must_use = "method moves the value of self and returns the modified value"]
    pub fn with_id<T: AsRef<str>>(mut self, id: T) -> Self {
        let id = sanitize(id.as_ref(), |c| c.is_control() || c == ';' || c == ':');
        self.id = (!id.is_empty()).then_some(id);
        self
    }

    /// Returns the target URI of the hyperlink.
    #[must_use]
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Returns the `id` parameter of the hyperlink, if any.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }
}

impl PartialEq for Hyperlink {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self, other) || (self.uri == other.uri && self.id == other.id)
    }
}

impl Hash for Hyperlink {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uri.hash(state);
        self.id.hash(state);
    }
}

impl fmt::Debug for Hyperlink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Hyperlink");
        debug.field("uri", &self.uri.as_str());
        if let Some(id) = &self.id {
            debug.field("id", &id.as_str());
        }
        debug.finish()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Hyperlink {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        HyperlinkRepr {
            uri: self.uri.clone(),
            id: self.id.clone(),
        }
        .serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Hyperlink {
    /// Deserializes a hyperlink, sanitizing it the same way as [`Hyperlink::new`] and
    /// [`Hyperlink::with_id`].
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let repr = HyperlinkRepr::deserialize(deserializer)?;
        let link = Self::new(repr.uri);
        Ok(match repr.id {
            Some(id) => link.with_id(id),
            None => link,
        })
    }
}

#[cfg(feature = "serde")]
#[derive(serde::Serialize, serde::Deserialize)]
struct HyperlinkRepr {
    uri: CompactString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<CompactString>,
}

/// Out-of-band data attached to a [`Cell`](crate::buffer::Cell).
///
/// Backends write extensions around the cell's symbol as run state. Extensions never affect the
/// symbol's text or width. Currently the only extension is a [`Hyperlink`].
///
/// A `CellExt` is a cheap to clone, reference-counted handle, so applying the same extension to
/// many cells shares a single allocation, and comparing two cells that share it costs a pointer
/// comparison. Values are immutable: [`with_hyperlink`](Self::with_hyperlink) returns a new value
/// (copying the data only when it is shared).
///
/// # Example
///
/// ```
/// use ratatui_core::buffer::{Cell, CellExt, Hyperlink};
///
/// let ext = CellExt::from(Hyperlink::new("https://ratatui.rs"));
/// let mut cell = Cell::new("r");
/// cell.set_ext(Some(ext.clone()));
/// assert_eq!(cell.ext(), Some(&ext));
/// assert_eq!(
///     cell.hyperlink().map(Hyperlink::uri),
///     Some("https://ratatui.rs")
/// );
/// ```
#[derive(Clone, Default)]
pub struct CellExt(Arc<CellExtData>);

/// The data behind a [`CellExt`]. Private so new kinds of extensions can be added without
/// breaking changes.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
struct CellExtData {
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    hyperlink: Option<Hyperlink>,
}

impl CellExt {
    /// Creates an empty extension.
    ///
    /// [`Cell::set_ext`](crate::buffer::Cell::set_ext) stores an empty extension as `None`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if the extension holds no data.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.hyperlink.is_none()
    }

    /// Returns the hyperlink, if any.
    #[must_use]
    pub fn hyperlink(&self) -> Option<&Hyperlink> {
        self.0.hyperlink.as_ref()
    }

    /// Returns the extension with its hyperlink replaced by `hyperlink`.
    #[must_use = "method moves the value of self and returns the modified value"]
    pub fn with_hyperlink(mut self, hyperlink: Option<Hyperlink>) -> Self {
        if self.0.hyperlink != hyperlink {
            Arc::make_mut(&mut self.0).hyperlink = hyperlink;
        }
        self
    }

    /// Returns `true` if both values point to the same allocation.
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl PartialEq for CellExt {
    /// Compares two extensions, checking whether they share an allocation before comparing their
    /// data.
    fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other) || self.0 == other.0
    }
}

impl Eq for CellExt {}

impl Hash for CellExt {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Debug for CellExt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("CellExt");
        if let Some(hyperlink) = &self.0.hyperlink {
            debug.field("hyperlink", hyperlink);
        }
        debug.finish()
    }
}

impl From<Hyperlink> for CellExt {
    fn from(hyperlink: Hyperlink) -> Self {
        Self::new().with_hyperlink(Some(hyperlink))
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for CellExt {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for CellExt {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        CellExtData::deserialize(deserializer).map(|data| Self(Arc::new(data)))
    }
}

/// Returns `s` without the characters matching `remove`, without allocating when nothing is
/// removed and `s` fits inline.
fn sanitize(s: &str, remove: impl Fn(char) -> bool) -> CompactString {
    if s.contains(&remove) {
        s.chars().filter(|&c| !remove(c)).collect()
    } else {
        CompactString::new(s)
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::buffer::Cell;
    use crate::style::{Color, Style};

    fn link(uri: &str) -> Hyperlink {
        Hyperlink::new(uri)
    }

    #[test]
    fn hyperlink_sanitizes_uri() {
        let link = Hyperlink::new("https://a\x1b]8;;evil\x07.com/\u{9c}x\u{7f}y\n");
        assert_eq!(link.uri(), "https://a]8;;evil.com/xy");
    }

    #[test]
    fn hyperlink_sanitizes_id() {
        let link = Hyperlink::new("https://ratatui.rs").with_id("a;b:c\x1b\\d\u{9c}");
        assert_eq!(link.id(), Some("abc\\d"));
        let link = Hyperlink::new("https://ratatui.rs").with_id(";:\x1b");
        assert_eq!(link.id(), None);
    }

    #[test]
    fn cell_ext_with_hyperlink() {
        let ext = CellExt::new();
        assert!(ext.is_empty());
        assert_eq!(ext.hyperlink(), None);
        let ext = ext.with_hyperlink(Some(link("a")));
        assert!(!ext.is_empty());
        assert_eq!(ext.hyperlink(), Some(&link("a")));
        // copy on write: the shared original is unchanged
        let shared = ext.clone();
        let ext = ext.with_hyperlink(None);
        assert!(ext.is_empty());
        assert_eq!(shared.hyperlink(), Some(&link("a")));
    }

    #[test]
    fn cell_eq_and_hash_include_ext() {
        use core::hash::BuildHasher;
        let hasher = hashbrown::DefaultHashBuilder::default();

        let plain = Cell::new("a");
        let mut linked = Cell::new("a");
        linked.set_hyperlink(Some(&link("a")));
        assert_ne!(plain, linked);
        assert_ne!(hasher.hash_one(&plain), hasher.hash_one(&linked));

        let mut other = Cell::new("a");
        other.set_hyperlink(Some(&link("a")));
        assert_eq!(linked, other);
        assert_eq!(hasher.hash_one(&linked), hasher.hash_one(&other));
    }

    #[test]
    fn cell_set_style_keeps_ext() {
        let mut cell = Cell::new("a");
        cell.set_hyperlink(Some(&link("a")));
        cell.set_style(Style::new().fg(Color::Red))
            .set_bg(Color::Blue);
        assert_eq!(cell.hyperlink(), Some(&link("a")));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_sanitizes_hyperlinks() {
        let link: Hyperlink = serde_json::from_str(r#"{"uri":"a\u001b\\b","id":"x;y"}"#).unwrap();
        assert_eq!(link, Hyperlink::new("a\\b").with_id("xy"));
    }
}
