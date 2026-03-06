use crate::render::AsciiCell;

/// A single ASCII video frame transmitted over the network.
pub struct AsciiFrame {
    pub width: u16,
    pub height: u16,
    pub data: Vec<u8>, // ASCII character bytes (space = 0x20)
}

/// Encode a rendered ASCII grid into a length-prefixed byte packet.
///
/// Wire format:
///   [width: u16 LE][height: u16 LE][data_len: u32 LE][data bytes...]
pub fn encode_frame(grid: &[Vec<AsciiCell>]) -> Vec<u8> {
    let height = grid.len() as u16;
    let width = grid.first().map(|r| r.len()).unwrap_or(0) as u16;

    let mut data: Vec<u8> = Vec::with_capacity((width as usize) * (height as usize));
    for row in grid {
        for cell in row {
            // Encode as a single ASCII byte; fall back to space for non-ASCII.
            let b = if cell.ch.is_ascii() {
                cell.ch as u8
            } else {
                b' '
            };
            data.push(b);
        }
    }

    let data_len = data.len() as u32;
    let mut out = Vec::with_capacity(8 + data.len());
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// Attempt to parse one frame from a byte buffer.
///
/// Returns `Some((frame, consumed_bytes))` if a complete frame is present,
/// or `None` if more data is needed.
pub fn decode_frame(buf: &[u8]) -> Option<(AsciiFrame, usize)> {
    const HEADER_LEN: usize = 8; // 2 + 2 + 4
    if buf.len() < HEADER_LEN {
        return None;
    }
    let width = u16::from_le_bytes([buf[0], buf[1]]);
    let height = u16::from_le_bytes([buf[2], buf[3]]);
    let data_len = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;

    let total = HEADER_LEN + data_len;
    if buf.len() < total {
        return None;
    }
    let data = buf[HEADER_LEN..total].to_vec();
    Some((AsciiFrame { width, height, data }, total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::AsciiCell;

    fn make_grid(width: usize, height: usize, ch: char) -> Vec<Vec<AsciiCell>> {
        (0..height)
            .map(|_| {
                (0..width)
                    .map(|_| AsciiCell { ch, color: None })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn encode_decode_roundtrip() {
        let grid = make_grid(4, 3, 'A');
        let encoded = encode_frame(&grid);
        let (frame, consumed) = decode_frame(&encoded).expect("decode failed");
        assert_eq!(consumed, encoded.len());
        assert_eq!(frame.width, 4);
        assert_eq!(frame.height, 3);
        assert_eq!(frame.data.len(), 12);
        assert!(frame.data.iter().all(|&b| b == b'A'));
    }

    #[test]
    fn empty_frame_roundtrip() {
        let grid: Vec<Vec<AsciiCell>> = vec![];
        let encoded = encode_frame(&grid);
        let (frame, consumed) = decode_frame(&encoded).expect("decode failed");
        assert_eq!(consumed, encoded.len());
        assert_eq!(frame.width, 0);
        assert_eq!(frame.height, 0);
        assert_eq!(frame.data.len(), 0);
    }

    #[test]
    fn partial_buffer_returns_none() {
        let grid = make_grid(5, 5, 'X');
        let encoded = encode_frame(&grid);
        // Trim the last byte — must return None.
        let partial = &encoded[..encoded.len() - 1];
        assert!(decode_frame(partial).is_none());
        // Header-only (4 bytes) — also None.
        assert!(decode_frame(&encoded[..4]).is_none());
        // Empty — also None.
        assert!(decode_frame(&[]).is_none());
    }

    #[test]
    fn multi_frame_consecutive_decode() {
        let grid1 = make_grid(2, 2, 'A');
        let grid2 = make_grid(3, 1, 'B');
        let mut buf = encode_frame(&grid1);
        buf.extend(encode_frame(&grid2));

        let (f1, n1) = decode_frame(&buf).expect("first decode failed");
        assert_eq!(f1.width, 2);
        assert_eq!(f1.height, 2);
        assert!(f1.data.iter().all(|&b| b == b'A'));

        let (f2, n2) = decode_frame(&buf[n1..]).expect("second decode failed");
        assert_eq!(f2.width, 3);
        assert_eq!(f2.height, 1);
        assert!(f2.data.iter().all(|&b| b == b'B'));

        assert_eq!(n1 + n2, buf.len());
    }
}
