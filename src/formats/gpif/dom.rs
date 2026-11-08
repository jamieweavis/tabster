//! A tiny arena DOM for GPIF documents, built in one streaming pass with quick-xml.
//!
//! GPIF is element-only XML (no mixed content), so each element keeps just its direct text,
//! attributes and children. Nodes, attribute values and text all live in a few flat buffers, so
//! building the tree costs a handful of amortised allocations regardless of document size.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

const NONE: u32 = u32::MAX;

/// Byte range into [`Doc::text`].
#[derive(Clone, Copy, Default)]
struct Span {
    start: u32,
    end: u32,
}

struct Node {
    name: u32,
    first_child: u32,
    next_sibling: u32,
    text: Span,
    attrs_start: u32,
    attrs_end: u32,
}

struct Attr {
    name: u32,
    value: Span,
}

pub struct Doc {
    nodes: Vec<Node>,
    names: Vec<Box<str>>,
    attrs: Vec<Attr>,
    text: String,
    root: u32,
}

/// Parsing error: a human-readable description.
pub type DomError = String;

impl Doc {
    pub fn parse(xml: &str) -> Result<Doc, DomError> {
        let mut b = Builder {
            doc: Doc {
                nodes: Vec::with_capacity(xml.len() / 24),
                names: Vec::new(),
                attrs: Vec::new(),
                text: String::with_capacity(xml.len() / 8),
                root: NONE,
            },
            name_ids: HashMap::new(),
            stack: Vec::new(),
        };
        let mut reader = Reader::from_str(xml);
        loop {
            let event = reader
                .read_event()
                .map_err(|e| format!("XML error at byte {}: {e}", reader.error_position()))?;
            match event {
                Event::Start(e) => b.open(&e, false)?,
                Event::Empty(e) => b.open(&e, true)?,
                Event::End(_) => {
                    if b.stack.pop().is_none() {
                        return Err("unmatched closing tag".into());
                    }
                }
                Event::Text(t) => b.push_text(&t.xml10_content()),
                Event::CData(t) => b.push_text(&t.xml10_content()),
                Event::GeneralRef(r) => {
                    let resolved = match r.resolve_char_ref() {
                        Ok(Some(c)) => Some(c),
                        Ok(None) => match &*r {
                            "lt" => Some('<'),
                            "gt" => Some('>'),
                            "amp" => Some('&'),
                            "apos" => Some('\''),
                            "quot" => Some('"'),
                            _ => None,
                        },
                        Err(_) => None,
                    };
                    match resolved {
                        Some(c) => b.push_text(c.encode_utf8(&mut [0; 4])),
                        None => b.push_text(&format!("&{};", &*r)),
                    }
                }
                Event::Eof => break,
                Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
            }
        }
        if !b.stack.is_empty() {
            return Err("unexpected end of document (unclosed elements)".into());
        }
        if b.doc.root == NONE {
            return Err("document has no root element".into());
        }
        Ok(b.doc)
    }

    pub fn root(&self) -> El<'_> {
        El {
            doc: self,
            idx: self.root,
        }
    }

    fn span(&self, s: Span) -> &str {
        self.text
            .get(s.start as usize..s.end as usize)
            .unwrap_or("")
    }
}

struct Builder {
    doc: Doc,
    name_ids: HashMap<Box<str>, u32>,
    /// (node index, last child index) of each open element.
    stack: Vec<(u32, u32)>,
}

impl Builder {
    fn intern(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.name_ids.get(name) {
            return id;
        }
        let id = self.doc.names.len() as u32;
        self.doc.names.push(name.into());
        self.name_ids.insert(name.into(), id);
        id
    }

    fn store(&mut self, s: &str) -> Span {
        let start = self.doc.text.len() as u32;
        self.doc.text.push_str(s);
        Span {
            start,
            end: self.doc.text.len() as u32,
        }
    }

    fn open(&mut self, e: &BytesStart<'_>, empty: bool) -> Result<(), DomError> {
        if self.doc.nodes.len() >= NONE as usize - 1 || self.doc.text.len() >= NONE as usize / 2 {
            return Err("document too large".into());
        }
        let name = self.intern(e.local_name().as_ref());
        let attrs_start = self.doc.attrs.len() as u32;
        for a in e.attributes() {
            let a = a.map_err(|err| format!("bad attribute: {err}"))?;
            let key = self.intern(a.key.local_name().as_ref());
            let value = a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|err| format!("bad attribute value: {err}"))?;
            let value = self.store(&value);
            self.doc.attrs.push(Attr { name: key, value });
        }
        let idx = self.doc.nodes.len() as u32;
        self.doc.nodes.push(Node {
            name,
            first_child: NONE,
            next_sibling: NONE,
            text: Span::default(),
            attrs_start,
            attrs_end: self.doc.attrs.len() as u32,
        });
        match self.stack.last_mut() {
            Some((parent, last)) => {
                if *last == NONE {
                    self.doc.nodes[*parent as usize].first_child = idx;
                } else {
                    self.doc.nodes[*last as usize].next_sibling = idx;
                }
                *last = idx;
            }
            None => {
                if self.doc.root != NONE {
                    return Err("multiple root elements".into());
                }
                self.doc.root = idx;
            }
        }
        if !empty {
            self.stack.push((idx, NONE));
        }
        Ok(())
    }

    fn push_text(&mut self, s: &str) {
        let Some(&(idx, _)) = self.stack.last() else {
            return;
        };
        let current = self.doc.nodes[idx as usize].text;
        let contiguous = current.end as usize == self.doc.text.len() && current.end > current.start;
        if contiguous {
            self.doc.text.push_str(s);
            self.doc.nodes[idx as usize].text.end = self.doc.text.len() as u32;
        } else if !s.trim().is_empty() {
            // Formatting whitespace between child elements is dropped; real text starts a span.
            let span = self.store(s);
            self.doc.nodes[idx as usize].text = span;
        }
    }
}

/// A borrowed element handle.
#[derive(Clone, Copy)]
pub struct El<'d> {
    doc: &'d Doc,
    idx: u32,
}

impl<'d> El<'d> {
    fn node(&self) -> &'d Node {
        &self.doc.nodes[self.idx as usize]
    }

    pub fn name(&self) -> &'d str {
        &self.doc.names[self.node().name as usize]
    }

    /// Direct text content, trimmed.
    pub fn text(&self) -> &'d str {
        self.doc.span(self.node().text).trim()
    }

    pub fn attr(&self, name: &str) -> Option<&'d str> {
        let n = self.node();
        self.doc.attrs[n.attrs_start as usize..n.attrs_end as usize]
            .iter()
            .find(|a| &*self.doc.names[a.name as usize] == name)
            .map(|a| self.doc.span(a.value))
    }

    pub fn children(&self) -> Children<'d> {
        Children {
            doc: self.doc,
            next: self.node().first_child,
        }
    }

    pub fn child(&self, name: &str) -> Option<El<'d>> {
        self.children().find(|c| c.name() == name)
    }

    pub fn child_text(&self, name: &str) -> Option<&'d str> {
        self.child(name).map(|c| c.text())
    }
}

pub struct Children<'d> {
    doc: &'d Doc,
    next: u32,
}

impl<'d> Iterator for Children<'d> {
    type Item = El<'d>;

    fn next(&mut self) -> Option<El<'d>> {
        if self.next == NONE {
            return None;
        }
        let el = El {
            doc: self.doc,
            idx: self.next,
        };
        self.next = self.doc.nodes[self.next as usize].next_sibling;
        Some(el)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_tree() {
        let doc = Doc::parse(
            r#"<?xml version="1.0"?><!-- hi --><A x="1 &amp; 2"><B>  hello &amp; bye </B><C/><B><![CDATA[<raw>]]></B>
            </A>"#,
        )
        .unwrap();
        let a = doc.root();
        assert_eq!(a.name(), "A");
        assert_eq!(a.attr("x"), Some("1 & 2"));
        assert_eq!(a.attr("y"), None);
        assert_eq!(a.text(), "");
        let names: Vec<_> = a.children().map(|c| c.name()).collect();
        assert_eq!(names, ["B", "C", "B"]);
        assert_eq!(a.child_text("B"), Some("hello & bye"));
        assert_eq!(a.children().nth(2).unwrap().text(), "<raw>");
        assert!(a.child("C").unwrap().children().next().is_none());
    }

    #[test]
    fn rejects_malformed() {
        assert!(Doc::parse("").is_err());
        assert!(Doc::parse("<A><B></A>").is_err());
        assert!(Doc::parse("<A><B>").is_err());
        assert!(Doc::parse("<A/><B/>").is_err());
        assert!(Doc::parse("</A>").is_err());
        assert!(Doc::parse("just text").is_err());
    }
}
