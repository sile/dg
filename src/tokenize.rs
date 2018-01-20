use std::str;

use {Error, Result};

#[derive(Debug)]
pub struct WordTokenizer<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> WordTokenizer<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        WordTokenizer { bytes, position: 0 }
    }
}
impl<'a> Iterator for WordTokenizer<'a> {
    type Item = Result<(usize, &'a str)>;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(n) = self.bytes
            .iter()
            .skip(self.position)
            .position(|&b| (b as char).is_alphanumeric() || (b >> 6) == 0b11)
        {
            self.position += n;
        } else {
            self.position = self.bytes.len();
        }

        if self.position == self.bytes.len() {
            return None;
        }

        let start = self.position;
        let end = self.bytes
            .iter()
            .skip(start + 1)
            .position(|&b| !(b as char).is_alphanumeric() && b < 0x80)
            .map(|p| p + start + 1)
            .unwrap_or(self.bytes.len());
        self.position = end;
        Some(track!(
            str::from_utf8(&self.bytes[start..end])
                .map_err(Error::from)
                .map(|word| (start, word))
        ))
    }
}
