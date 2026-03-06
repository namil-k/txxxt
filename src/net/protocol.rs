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
