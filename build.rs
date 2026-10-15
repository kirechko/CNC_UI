use std::fs;
use std::path::Path;

fn append_png_chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    png.extend_from_slice(kind);
    png.extend_from_slice(data);

    let mut crc = !0_u32;
    for byte in kind.iter().chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    png.extend_from_slice(&(!crc).to_be_bytes());
}

fn create_tauri_icon(path: &Path) -> std::io::Result<()> {
    let mut raw = Vec::with_capacity(32 * 129);
    for _ in 0..32 {
        raw.push(0); // PNG filter: None
        for _ in 0..32 {
            raw.extend_from_slice(&[40, 126, 170, 255]);
        }
    }

    let mut zlib = vec![0x78, 0x01];
    zlib.push(0x01); // Final uncompressed DEFLATE block.
    zlib.extend_from_slice(&(raw.len() as u16).to_le_bytes());
    zlib.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
    zlib.extend_from_slice(&raw);

    let mut a = 1_u32;
    let mut b = 0_u32;
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&32_u32.to_be_bytes());
    header.extend_from_slice(&32_u32.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA.
    append_png_chunk(&mut png, b"IHDR", &header);
    append_png_chunk(&mut png, b"IDAT", &zlib);
    append_png_chunk(&mut png, b"IEND", &[]);

    fs::write(path, png)
}

fn main() {
    let icon_path = Path::new("icons/icon.png");
    fs::create_dir_all(icon_path.parent().expect("icon path must have a parent"))
        .expect("failed to create Tauri icons directory");
    create_tauri_icon(icon_path).expect("failed to generate Tauri icon");

    tauri_build::build();
}
