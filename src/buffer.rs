#[derive(Debug)]
/// A growable byte buffer with a logical read position.
///
/// Consumed bytes remain in the backing allocation until [`Buffer::compact`]
/// is called. This makes consuming bytes cheap while allowing the caller to
/// choose when the remaining bytes should be moved to the front.
pub struct Buffer {
    data: Vec<u8>,
    start: usize,
}

impl Buffer {
    /// Creates an empty buffer with no initial allocation.
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            start: 0,
        }
    }

    /// Returns the number of unread bytes in the buffer.
    pub fn len(&self) -> usize {
        self.data.len() - self.start
    }

    /// Returns `true` when the buffer contains no unread bytes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Appends bytes to the end of the buffer.
    pub fn append(&mut self, data: &[u8]) {
        self.data.extend_from_slice(data)
    }

    /// Advances the logical read position by `len` bytes without copying.
    ///
    /// # Panics
    ///
    /// Panics if `len` is greater than the number of unread bytes.
    pub fn consume(&mut self, len: usize) {
        assert!(
            len <= self.len(),
            "cannot consume {len} bytes from a buffer containing {} unread bytes",
            self.len()
        );
        self.start += len;
    }

    /// Returns all unread bytes as a borrowed slice without consuming them.
    pub fn as_slice(&self) -> &[u8] {
        &self.data[self.start..]
    }

    /// Copies and consumes the first `len` unread bytes.
    ///
    /// Returns `None` and leaves the buffer unchanged when fewer than `len`
    /// unread bytes are available.
    pub fn take(&mut self, len: usize) -> Option<Vec<u8>> {
        if self.len() < len {
            return None;
        }

        let end: usize = self.start + len;

        let Some(data) = self.data.get(self.start..end) else {
            return None;
        };

        // Advance start len bytes
        self.start += len;

        Some(data.to_vec())
    }

    /// Moves unread bytes to the front of the backing allocation.
    ///
    /// Compaction preserves the unread data and resets the logical read
    /// position to zero. It is a physical maintenance operation and is not
    /// performed automatically by `consume`.
    pub fn compact(&mut self) {
        let rem = self.data.len() - self.start;
        self.data.copy_within(self.start.., 0);
        self.data.truncate(rem);
        self.start = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::Buffer;

    #[test]
    fn new_buffer_is_empty() {
        let buffer = Buffer::new();

        assert_eq!(buffer.len(), 0);
        assert!(buffer.is_empty());
    }

    #[test]
    fn append_adds_data_in_order() {
        let mut buffer = Buffer::new();

        buffer.append(b"hello");
        buffer.append(b" world");

        assert_eq!(buffer.len(), 11);
        assert_eq!(buffer.take(11), Some(b"hello world".to_vec()));
    }

    #[test]
    fn append_empty_data_does_not_change_buffer() {
        let mut buffer = Buffer::new();

        buffer.append(b"");

        assert!(buffer.is_empty());
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.as_slice(), b"");
    }

    #[test]
    fn as_slice_returns_only_unread_data_without_consuming() {
        let mut buffer = Buffer::new();
        buffer.append(b"hello world");
        buffer.consume(6);

        assert_eq!(buffer.as_slice(), b"world");
        assert_eq!(buffer.len(), 5);
        assert!(!buffer.is_empty());
    }

    #[test]
    fn consume_advances_without_copying_unread_data() {
        let mut buffer = Buffer::new();
        buffer.append(b"hello world");

        buffer.consume(6);

        assert_eq!(buffer.as_slice(), b"world");
        assert_eq!(buffer.take(5), Some(b"world".to_vec()));
    }

    #[test]
    fn take_returns_and_consumes_requested_bytes() {
        let mut buffer = Buffer::new();
        buffer.append(b"hello world");

        assert_eq!(buffer.take(5), Some(b"hello".to_vec()));
        assert_eq!(buffer.len(), 6);
        assert!(!buffer.is_empty());
        assert_eq!(buffer.take(6), Some(b" world".to_vec()));
        assert!(buffer.is_empty());
    }

    #[test]
    fn take_zero_returns_empty_without_consuming_data() {
        let mut buffer = Buffer::new();
        buffer.append(b"hello");

        assert_eq!(buffer.take(0), Some(Vec::new()));
        assert_eq!(buffer.len(), 5);
        assert_eq!(buffer.take(5), Some(b"hello".to_vec()));
    }

    #[test]
    fn take_returns_none_without_consuming_when_data_is_incomplete() {
        let mut buffer = Buffer::new();
        buffer.append(b"hello");

        assert_eq!(buffer.take(6), None);
        assert_eq!(buffer.len(), 5);
        assert_eq!(buffer.take(5), Some(b"hello".to_vec()));
    }

    #[test]
    #[should_panic(expected = "cannot consume 6 bytes")]
    fn consume_panics_when_requested_length_exceeds_unread_data() {
        let mut buffer = Buffer::new();
        buffer.append(b"hello");

        buffer.consume(6);
    }

    #[test]
    fn append_after_take_preserves_unread_data_order() {
        let mut buffer = Buffer::new();
        buffer.append(b"hello");

        assert_eq!(buffer.take(2), Some(b"he".to_vec()));
        buffer.append(b" world");

        assert_eq!(buffer.len(), 9);
        assert_eq!(buffer.take(9), Some(b"llo world".to_vec()));
    }

    #[test]
    fn compact_moves_unread_data_to_the_front() {
        let mut buffer = Buffer::new();
        buffer.append(b"hello world");
        assert_eq!(buffer.take(6), Some(b"hello ".to_vec()));

        buffer.compact();

        assert_eq!(buffer.start, 0);
        assert_eq!(buffer.data, b"world");
        assert_eq!(buffer.len(), 5);
        assert_eq!(buffer.take(5), Some(b"world".to_vec()));
    }

    #[test]
    fn compact_without_consumed_data_preserves_buffer() {
        let mut buffer = Buffer::new();
        buffer.append(b"hello");

        buffer.compact();

        assert_eq!(buffer.start, 0);
        assert_eq!(buffer.as_slice(), b"hello");
        assert_eq!(buffer.take(5), Some(b"hello".to_vec()));
    }

    #[test]
    fn compact_clears_consumed_storage_when_no_data_remains() {
        let mut buffer = Buffer::new();
        buffer.append(b"hello");
        assert_eq!(buffer.take(5), Some(b"hello".to_vec()));

        assert!(buffer.is_empty());
        assert!(!buffer.data.is_empty());

        buffer.compact();

        assert_eq!(buffer.start, 0);
        assert!(buffer.data.is_empty());
        assert!(buffer.is_empty());
    }
}
