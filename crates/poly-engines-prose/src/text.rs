//! Prose text lifted out of a file, with the byte offset every character came from.

/// Text handed to harper plus, for each of its characters, where that character sits in the file.
///
/// Comment delimiters and front matter are dropped before harper sees the text, so a harper span
/// (an index into the characters) cannot be turned into a file position by arithmetic alone.
#[derive(Debug, Default)]
pub(crate) struct Mapped {
    pub(crate) text: String,
    /// The byte offset in the file of each character of `text`.
    origin: Vec<usize>,
}

impl Mapped {
    /// Append `piece`, a contiguous slice of the file that starts at byte `at`.
    pub(crate) fn push_slice(&mut self, piece: &str, at: usize) {
        for (offset, ch) in piece.char_indices() {
            self.text.push(ch);
            self.origin.push(at + offset);
        }
    }

    /// Append `ch`, which has no counterpart of its own in the file, attributed to byte `at`.
    pub(crate) fn push_synthetic(&mut self, ch: char, at: usize) {
        self.text.push(ch);
        self.origin.push(at);
    }

    /// The file byte range covered by the characters `start..end` of `text`, whose characters are `chars`.
    pub(crate) fn byte_range(&self, chars: &[char], start: usize, end: usize) -> Option<(usize, usize)> {
        let last = end.checked_sub(1).filter(|last| *last >= start)?;
        let first = *self.origin.get(start)?;
        let tail = *self.origin.get(last)?;
        Some((first, tail + chars.get(last)?.len_utf8()))
    }
}

/// The file's text from `content`, past a leading front-matter block.
pub(crate) fn markdown_body(content: &str) -> Mapped {
    let start = front_matter_end(content);
    let mut mapped = Mapped::default();
    mapped.push_slice(&content[start..], start);
    mapped
}

/// The byte offset just past a leading `---` or `+++` front-matter block, or 0 when there is none.
fn front_matter_end(content: &str) -> usize {
    let mut lines = content.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return 0;
    };
    let fence = first.trim_end();
    if fence != "---" && fence != "+++" {
        return 0;
    }
    let mut offset = first.len();
    for line in lines {
        offset += line.len();
        if line.trim_end() == fence {
            return offset;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_is_skipped() {
        let body = markdown_body("---\ntitle: x\n---\nHello there.\n");
        assert_eq!(body.text, "Hello there.\n");
        assert_eq!(body.origin[0], "---\ntitle: x\n---\n".len());
    }

    #[test]
    fn an_unclosed_fence_is_not_front_matter() {
        assert_eq!(markdown_body("---\nHello\n").text, "---\nHello\n");
    }

    #[test]
    fn multibyte_ranges_end_after_the_last_character() {
        let mapped = markdown_body("é x");
        let chars: Vec<char> = mapped.text.chars().collect();
        assert_eq!(mapped.byte_range(&chars, 0, 1), Some((0, 2)));
        assert_eq!(mapped.byte_range(&chars, 2, 3), Some((3, 4)));
        assert_eq!(mapped.byte_range(&chars, 2, 2), None);
    }
}
