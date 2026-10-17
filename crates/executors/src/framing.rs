//! Bounded newline records assembled before decoding UTF-8 or protocol fields.
const LIMIT: usize = 8 << 20;
#[derive(Default)]
pub(crate) struct Lines {
    bytes: Vec<u8>,
    closed: bool,
    pub(crate) overflow: bool,
}
impl Lines {
    pub(crate) fn push(&mut self, mut input: &[u8]) -> Vec<Vec<u8>> {
        let mut records = Vec::new();
        while !self.closed && !input.is_empty() {
            let newline = input.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(input.len(), |index| index + 1);
            let room = LIMIT - self.bytes.len();
            if count > room || (count == room && newline.is_none()) {
                self.overflow = true;
                self.closed = true;
                self.bytes.clear();
                break;
            }
            self.bytes.extend_from_slice(&input[..count]);
            input = &input[count..];
            if newline.is_some() {
                records.push(std::mem::take(&mut self.bytes));
            }
        }
        records
    }
    pub(crate) fn finish(&mut self) -> Option<Vec<u8>> {
        if self.closed {
            return None;
        }
        self.closed = true;
        (!self.bytes.is_empty()).then(|| std::mem::take(&mut self.bytes))
    }
}
