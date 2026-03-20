use crate::render::AsciiCell;

/// A single ASCII video frame transmitted over the network.
pub struct AsciiFrame {
    pub width: u16,
    pub height: u16,
    pub cells: Vec<CellData>,
}

/// Network cell: character (full unicode) + optional color.
pub struct CellData {
    pub ch: char,
    pub color: Option<(u8, u8, u8)>,
}

/// Bytes per cell in wire format: [ch: u32 LE, has_color, r, g, b] = 8 bytes.
const CELL_BYTES: usize = 8;

/// Encode a rendered ASCII grid into a length-prefixed byte packet.
///
/// Wire format (v3 — unicode):
///   [width: u16 LE][height: u16 LE][data_len: u32 LE][cells: N * 8 bytes]
///   Each cell: [ch: u32 LE, has_color(0/1), r, g, b]
pub fn encode_frame(grid: &[Vec<AsciiCell>]) -> Vec<u8> {
    let height = grid.len() as u16;
    let width = grid.first().map(|r| r.len()).unwrap_or(0) as u16;
    let cell_count = (width as u32) * (height as u32);

    let data_len = cell_count as usize * CELL_BYTES;
    let mut out = Vec::with_capacity(8 + data_len);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&(data_len as u32).to_le_bytes());

    for row in grid {
        for cell in row {
            out.extend_from_slice(&(cell.ch as u32).to_le_bytes());
            match cell.color {
                Some((r, g, b)) => {
                    out.push(1);
                    out.push(r);
                    out.push(g);
                    out.push(b);
                }
                None => {
                    out.push(0);
                    out.push(0);
                    out.push(0);
                    out.push(0);
                }
            }
        }
    }
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

    let data = &buf[HEADER_LEN..total];
    let cell_count = data_len / CELL_BYTES;
    let mut cells = Vec::with_capacity(cell_count);

    for i in 0..cell_count {
        let offset = i * CELL_BYTES;
        let ch_u32 = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        let ch = char::from_u32(ch_u32).unwrap_or(' ');
        let has_color = data[offset + 4];
        let color = if has_color != 0 {
            Some((data[offset + 5], data[offset + 6], data[offset + 7]))
        } else {
            None
        };
        cells.push(CellData { ch, color });
    }

    Some((AsciiFrame { width, height, cells }, total))
}

/// Convert an AsciiFrame back to a 2D grid of AsciiCells.
pub fn frame_to_grid(frame: &AsciiFrame) -> Vec<Vec<AsciiCell>> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    if w == 0 || h == 0 {
        return vec![];
    }

    let mut grid = Vec::with_capacity(h);
    for row in 0..h {
        let mut line = Vec::with_capacity(w);
        for col in 0..w {
            let idx = row * w + col;
            if let Some(cell) = frame.cells.get(idx) {
                line.push(AsciiCell { ch: cell.ch, color: cell.color.clone() });
            } else {
                line.push(AsciiCell { ch: ' ', color: None });
            }
        }
        grid.push(line);
    }
    grid
}

/// Rescale a grid to fit target dimensions using nearest-neighbor sampling.
/// Stretches or shrinks both axes independently to fill (target_cols × target_rows).
pub fn rescale_grid(
    grid: &[Vec<AsciiCell>],
    target_cols: usize,
    target_rows: usize,
) -> Vec<Vec<AsciiCell>> {
    if grid.is_empty() || target_cols == 0 || target_rows == 0 {
        return vec![];
    }
    let src_h = grid.len();
    let src_w = grid[0].len();
    if src_w == 0 {
        return vec![];
    }

    let mut result = Vec::with_capacity(target_rows);
    for row in 0..target_rows {
        let src_row = (row * src_h) / target_rows;
        let src_row = src_row.min(src_h - 1);
        let mut line = Vec::with_capacity(target_cols);
        for col in 0..target_cols {
            let src_col = (col * src_w) / target_cols;
            let src_col = src_col.min(src_w - 1);
            line.push(grid[src_row][src_col].clone());
        }
        result.push(line);
    }
    result
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

    fn make_color_grid(width: usize, height: usize, ch: char, rgb: (u8, u8, u8)) -> Vec<Vec<AsciiCell>> {
        (0..height)
            .map(|_| {
                (0..width)
                    .map(|_| AsciiCell { ch, color: Some(rgb) })
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
        assert_eq!(frame.cells.len(), 12);
        assert!(frame.cells.iter().all(|c| c.ch == 'A' && c.color.is_none()));
    }

    #[test]
    fn color_roundtrip() {
        let grid = make_color_grid(2, 2, '#', (255, 128, 0));
        let encoded = encode_frame(&grid);
        let (frame, _) = decode_frame(&encoded).expect("decode failed");
        assert_eq!(frame.cells.len(), 4);
        for cell in &frame.cells {
            assert_eq!(cell.ch, '#');
            assert_eq!(cell.color, Some((255, 128, 0)));
        }
    }

    #[test]
    fn unicode_roundtrip() {
        // Block characters, outline chars, dots — all non-ASCII.
        let chars = vec!['░', '▒', '▓', '█', '─', '╱', '│', '╲', '●', '◉'];
        for ch in chars {
            let grid = make_grid(2, 2, ch);
            let encoded = encode_frame(&grid);
            let (frame, _) = decode_frame(&encoded).expect("decode failed");
            let restored = frame_to_grid(&frame);
            assert_eq!(restored[0][0].ch, ch, "roundtrip failed for {:?}", ch);
        }
    }

    #[test]
    fn frame_to_grid_roundtrip() {
        let grid = make_color_grid(3, 2, 'X', (10, 20, 30));
        let encoded = encode_frame(&grid);
        let (frame, _) = decode_frame(&encoded).unwrap();
        let restored = frame_to_grid(&frame);
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].len(), 3);
        assert_eq!(restored[0][0].ch, 'X');
        assert_eq!(restored[0][0].color, Some((10, 20, 30)));
    }

    #[test]
    fn empty_frame_roundtrip() {
        let grid: Vec<Vec<AsciiCell>> = vec![];
        let encoded = encode_frame(&grid);
        let (frame, consumed) = decode_frame(&encoded).expect("decode failed");
        assert_eq!(consumed, encoded.len());
        assert_eq!(frame.width, 0);
        assert_eq!(frame.height, 0);
        assert_eq!(frame.cells.len(), 0);
    }

    #[test]
    fn partial_buffer_returns_none() {
        let grid = make_grid(5, 5, 'X');
        let encoded = encode_frame(&grid);
        assert!(decode_frame(&encoded[..encoded.len() - 1]).is_none());
        assert!(decode_frame(&encoded[..4]).is_none());
        assert!(decode_frame(&[]).is_none());
    }

    #[test]
    fn rescale_grid_upscale() {
        let grid = make_grid(2, 2, 'A');
        let scaled = rescale_grid(&grid, 4, 4);
        assert_eq!(scaled.len(), 4);
        assert_eq!(scaled[0].len(), 4);
        assert!(scaled.iter().flatten().all(|c| c.ch == 'A'));
    }

    #[test]
    fn rescale_grid_downscale() {
        let grid = make_grid(10, 10, 'B');
        let scaled = rescale_grid(&grid, 3, 3);
        assert_eq!(scaled.len(), 3);
        assert_eq!(scaled[0].len(), 3);
        assert!(scaled.iter().flatten().all(|c| c.ch == 'B'));
    }

    #[test]
    fn rescale_grid_empty() {
        let grid: Vec<Vec<AsciiCell>> = vec![];
        let scaled = rescale_grid(&grid, 5, 5);
        assert!(scaled.is_empty());
    }
}
