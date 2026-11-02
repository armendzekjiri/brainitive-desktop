//! The unread badge (BRAI-361). macOS: the dock badge; Linux: the launcher badge (Unity API);
//! Windows has no badge API for desktop apps, so the taskbar button gets an **overlay icon**
//! rendered here: a red disc with the count ("1" … "99", "99+"). Pure; drawn into RGBA.

pub const SIZE: u32 = 32;

/// 3×5 pixel glyphs (rows top to bottom, 3 bits each, high bit = left).
const GLYPHS: [(char, [u8; 5]); 11] = [
    ('0', [0b111, 0b101, 0b101, 0b101, 0b111]),
    ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
    ('2', [0b111, 0b001, 0b111, 0b100, 0b111]),
    ('3', [0b111, 0b001, 0b111, 0b001, 0b111]),
    ('4', [0b101, 0b101, 0b111, 0b001, 0b001]),
    ('5', [0b111, 0b100, 0b111, 0b001, 0b111]),
    ('6', [0b111, 0b100, 0b111, 0b101, 0b111]),
    ('7', [0b111, 0b001, 0b010, 0b010, 0b010]),
    ('8', [0b111, 0b101, 0b111, 0b101, 0b111]),
    ('9', [0b111, 0b101, 0b111, 0b001, 0b111]),
    ('+', [0b000, 0b010, 0b111, 0b010, 0b000]),
];

/// What the badge says: `None` for 0 (no badge).
pub fn text(count: i64) -> Option<String> {
    match count {
        i64::MIN..=0 => None,
        1..=99 => Some(count.to_string()),
        _ => Some("99+".into()),
    }
}

/// A `SIZE`×`SIZE` RGBA image: red disc, white count. `None` for 0.
pub fn render(count: i64) -> Option<Vec<u8>> {
    let text = text(count)?;
    let size = SIZE as i32;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    let center = (size as f32 - 1.0) / 2.0;
    let radius = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)).sqrt();
            // 1 px of anti-aliasing at the edge
            let alpha = (radius - d).clamp(0.0, 1.0);
            if alpha > 0.0 {
                put(&mut rgba, x, y, [0xd9, 0x30, 0x25, (alpha * 255.0) as u8]);
            }
        }
    }
    // glyphs scaled ×3 (one digit) / ×2 (more), 1 px gap per scale step
    let scale: i32 = if text.len() == 1 { 3 } else { 2 };
    let glyph_w = 3 * scale;
    let gap = scale;
    let width = text.len() as i32 * glyph_w + (text.len() as i32 - 1) * gap;
    let height = 5 * scale;
    let (left, top) = ((size - width) / 2, (size - height) / 2);
    for (i, ch) in text.chars().enumerate() {
        let Some((_, rows)) = GLYPHS.iter().find(|(c, _)| *c == ch) else {
            continue;
        };
        let x0 = left + i as i32 * (glyph_w + gap);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            put(
                                &mut rgba,
                                x0 + col * scale + dx,
                                top + row as i32 * scale + dy,
                                [255, 255, 255, 255],
                            );
                        }
                    }
                }
            }
        }
    }
    Some(rgba)
}

fn put(rgba: &mut [u8], x: i32, y: i32, px: [u8; 4]) {
    if !(0..SIZE as i32).contains(&x) || !(0..SIZE as i32).contains(&y) {
        return;
    }
    let i = ((y as u32 * SIZE + x as u32) * 4) as usize;
    rgba[i..i + 4].copy_from_slice(&px);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn badge_text() {
        assert_eq!(text(0), None);
        assert_eq!(text(-3), None);
        assert_eq!(text(7).as_deref(), Some("7"));
        assert_eq!(text(99).as_deref(), Some("99"));
        assert_eq!(text(100).as_deref(), Some("99+"));
    }

    #[test]
    fn renders_a_red_disc_with_white_digits() {
        assert!(render(0).is_none());
        for n in [1, 8, 42, 120] {
            let img = render(n).unwrap();
            assert_eq!(img.len(), (SIZE * SIZE * 4) as usize);
            // corners transparent, the edge of the disc red
            assert_eq!(px(&img, 0, 0)[3], 0);
            assert_eq!(px(&img, SIZE - 1, SIZE - 1)[3], 0);
            assert_eq!(&px(&img, 2, SIZE / 2)[..3], &[0xd9, 0x30, 0x25]);
            // some white pixels for the count
            let white = img.chunks(4).filter(|p| p == &[255, 255, 255, 255]).count();
            assert!(white > 10, "{n}: {white}");
        }
        // "8" is a full 3×5 glyph minus 2 holes = 13 cells of 3×3 pixels
        let eight = render(8).unwrap();
        assert_eq!(eight.chunks(4).filter(|p| p == &[255, 255, 255, 255]).count(), 13 * 9);
    }
}
