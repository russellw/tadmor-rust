//! A minimal PDF writer: pages of text, lines and filled rectangles in the
//! standard-14 Helvetica fonts, which every reader has, so nothing is
//! embedded. Text is encoded as WinAnsi. Content streams are written
//! uncompressed, since the standard library has no deflate; the files are
//! larger than tadmor's but every reader takes them.

use std::fmt::Write;

use crate::pdf_metrics::{HELVETICA, HELVETICA_BOLD};

#[derive(Clone, Copy, PartialEq)]
pub enum Font {
    Helvetica,
    HelveticaBold,
}

impl Font {
    fn resource(self) -> &'static str {
        match self {
            Font::Helvetica => "/F1",
            Font::HelveticaBold => "/F2",
        }
    }

    /// The width of `s` set at `size` points.
    pub fn width(self, size: f64, s: &str) -> f64 {
        let widths = if self == Font::Helvetica { &HELVETICA } else { &HELVETICA_BOLD };
        let units: u32 = win_ansi(s).iter().map(|b| u32::from(widths[*b as usize])).sum();
        f64::from(units) * size / 1000.0
    }
}

/// `s` in WinAnsiEncoding, with characters it lacks as '?'.
fn win_ansi(s: &str) -> Vec<u8> {
    const SPECIALS: [(char, u8); 27] = [
        ('€', 0x80), ('‚', 0x82), ('ƒ', 0x83), ('„', 0x84), ('…', 0x85), ('†', 0x86), ('‡', 0x87), ('ˆ', 0x88), ('‰', 0x89),
        ('Š', 0x8A), ('‹', 0x8B), ('Œ', 0x8C), ('Ž', 0x8E), ('‘', 0x91), ('’', 0x92), ('“', 0x93), ('”', 0x94), ('•', 0x95),
        ('–', 0x96), ('—', 0x97), ('˜', 0x98), ('™', 0x99), ('š', 0x9A), ('›', 0x9B), ('œ', 0x9C), ('ž', 0x9E), ('Ÿ', 0x9F),
    ];
    s.chars()
        .map(|c| match c as u32 {
            0..0x80 | 0xA0..=0xFF => c as u8,
            _ => SPECIALS.iter().find(|(s, _)| *s == c).map_or(b'?', |(_, b)| *b),
        })
        .collect()
}

/// A number as PDF writes it: no exponent, no trailing zeros.
fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_string() } else { s.to_string() }
}

pub struct Page {
    width: f64,
    height: f64,
    content: Vec<u8>,
}

impl Page {
    pub fn text(&mut self, font: Font, size: f64, x: f64, y: f64, s: &str) {
        self.text_gray(font, size, x, y, 0.0, s);
    }

    pub fn text_gray(&mut self, font: Font, size: f64, x: f64, y: f64, gray: f64, s: &str) {
        let start = format!("BT {} {} Tf {} g {} {} Td (", font.resource(), num(size), num(gray), num(x), num(y));
        self.content.extend_from_slice(start.as_bytes());
        for b in win_ansi(s) {
            match b {
                b'(' | b')' | b'\\' => self.content.extend_from_slice(&[b'\\', b]),
                b'\n' => self.content.extend_from_slice(b"\\n"),
                b'\r' => self.content.extend_from_slice(b"\\r"),
                _ => self.content.push(b),
            }
        }
        self.content.extend_from_slice(b") Tj ET\n");
    }

    pub fn line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, width: f64, gray: f64) {
        let op = format!("{} w {} G {} {} m {} {} l S\n", num(width), num(gray), num(x1), num(y1), num(x2), num(y2));
        self.content.extend_from_slice(op.as_bytes());
    }
}

#[derive(Default)]
pub struct Doc {
    pages: Vec<Page>,
}

impl Doc {
    pub fn add_page(&mut self, width: f64, height: f64) -> &mut Page {
        self.pages.push(Page { width, height, content: Vec::new() });
        self.pages.last_mut().unwrap()
    }

    pub fn pages_mut(&mut self) -> &mut [Page] {
        &mut self.pages
    }

    /// The finished file: catalog, page tree, the two fonts, then each page
    /// and its content stream, with a cross-reference table.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::new();
        let mut object = |out: &mut Vec<u8>, body: &[u8]| {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        };
        let kids: String = (0..self.pages.len()).map(|i| format!("{} 0 R ", 5 + 2 * i)).collect();
        object(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
        object(&mut out, format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", self.pages.len()).as_bytes());
        object(&mut out, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");
        object(&mut out, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>");
        for (i, page) in self.pages.iter().enumerate() {
            let page_dict = format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
                num(page.width),
                num(page.height),
                6 + 2 * i
            );
            object(&mut out, page_dict.as_bytes());
            let mut stream = format!("<< /Length {} >>\nstream\n", page.content.len()).into_bytes();
            stream.extend_from_slice(&page.content);
            stream.extend_from_slice(b"\nendstream");
            object(&mut out, &stream);
        }
        let xref = out.len();
        let mut tail = format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
        for offset in &offsets {
            let _ = write!(tail, "{offset:010} 00000 n \n");
        }
        let _ = write!(tail, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1);
        out.extend_from_slice(tail.as_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_is_well_formed() {
        let mut doc = Doc::default();
        doc.add_page(595.28, 841.89).text(Font::Helvetica, 12.0, 72.0, 700.0, "Total (net) \\ 12€");
        let bytes = doc.bytes();
        assert!(bytes.starts_with(b"%PDF-1.4"));
        let shown: &[u8] = b"(Total \\(net\\) \\\\ 12\x80) Tj";
        assert!(bytes.windows(shown.len()).any(|w| w == shown), "escaped and WinAnsi-encoded");
        // The cross-reference offsets point at their objects. (Search the
        // bytes: the file is not UTF-8.)
        let find = |needle: &[u8]| bytes.windows(needle.len()).position(|w| w == needle).unwrap();
        let xref = find(b"\nxref\n") + 1;
        let tail = std::str::from_utf8(&bytes[xref..]).unwrap();
        let startxref: usize = tail.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        assert_eq!(startxref, xref);
        for (i, line) in tail.lines().skip(3).take(6).enumerate() {
            let offset: usize = line[..10].parse().unwrap();
            assert!(bytes[offset..].starts_with(format!("{} 0 obj", i + 1).as_bytes()), "object {}", i + 1);
        }
    }

    #[test]
    fn widths_and_encoding() {
        assert_eq!(Font::Helvetica.width(10.0, "A"), 6.67);
        assert!(Font::HelveticaBold.width(10.0, "Total") > Font::Helvetica.width(10.0, "Total"));
        assert_eq!(win_ansi("é€✓"), [0xE9, 0x80, b'?']);
        assert_eq!(num(595.28), "595.28");
        assert_eq!(num(12.0), "12");
    }
}
