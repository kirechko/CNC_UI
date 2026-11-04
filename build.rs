use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
        raw.extend_from_slice(&[0]); // PNG filter: None
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

/// Expands one HTML template while rejecting recursive or escaping partial includes.
fn render_html_template(
    template_path: &Path,
    partial_root: &Path,
    include_stack: &mut Vec<PathBuf>,
) -> io::Result<String> {
    let canonical_path = template_path.canonicalize()?;
    if include_stack.contains(&canonical_path) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("cyclic HTML include: {}", canonical_path.display()),
        ));
    }

    include_stack.push(canonical_path);
    let template = fs::read_to_string(template_path)?;
    let mut output = String::with_capacity(template.len());
    for line in template.split_inclusive('\n') {
        let directive = line.trim();
        if let Some(relative) = directive
            .strip_prefix("<!-- @include ")
            .and_then(|value| value.strip_suffix(" -->"))
        {
            let relative_path = Path::new(relative);
            if relative_path.is_absolute()
                || relative_path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid HTML partial path: {relative}"),
                ));
            }
            let partial_path = partial_root.join(relative_path);
            if !partial_path
                .canonicalize()?
                .starts_with(partial_root.canonicalize()?)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("HTML partial escapes its source directory: {relative}"),
                ));
            }
            output.push_str(&render_html_template(
                &partial_path,
                partial_root,
                include_stack,
            )?);
        } else {
            output.push_str(line);
        }
    }
    include_stack.pop();
    Ok(output)
}

/// Merges the language's thematic JSON fragments into its legacy flat dictionary file.
fn build_locale_dictionaries(source: &Path) -> io::Result<()> {
    let locales = source.join("locales");
    let parts_root = locales.join("parts");
    println!("cargo:rerun-if-changed={}", parts_root.display());

    for language in ["en", "ru", "zh", "fa", "vi", "id", "hi", "pt", "sw", "ko"] {
        let parts_dir = parts_root.join(language);
        let mut entries = serde_json::Map::new();
        let mut part_paths: Vec<_> = fs::read_dir(&parts_dir)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<_>>()?;
        part_paths.sort();
        if part_paths.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no locale fragments found for {language}"),
            ));
        }

        for part_path in part_paths {
            println!("cargo:rerun-if-changed={}", part_path.display());
            let fragment: serde_json::Map<String, serde_json::Value> =
                serde_json::from_slice(&fs::read(&part_path)?).map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid locale fragment {}: {error}", part_path.display()),
                    )
                })?;
            for (key, value) in fragment {
                if entries.insert(key.clone(), value).is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("duplicate locale key {key:?} while assembling {language}"),
                    ));
                }
            }
        }

        let mut output = serde_json::to_vec_pretty(&serde_json::Value::Object(entries))
            .map_err(io::Error::other)?;
        output.push(b'\n');
        fs::write(locales.join(format!("{language}.json")), output)?;
    }
    Ok(())
}

/// Copies static frontend assets while omitting source-only templates and locale fragments.
fn copy_frontend_assets(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let is_locale_parts = path.file_name().is_some_and(|name| name == "parts")
            && path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "locales");
        if name == "index.html" || name == "partials" || is_locale_parts {
            continue;
        }
        let target = destination.join(name);
        if path.is_dir() {
            copy_frontend_assets(&path, &target)?;
        } else {
            fs::copy(path, target)?;
        }
    }
    Ok(())
}

/// Generates the assembled frontend directory consumed by the web server and Tauri.
fn build_frontend() {
    println!("cargo:rerun-if-changed=web");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    let source = Path::new("web");
    build_locale_dictionaries(source).expect("failed to assemble locale dictionaries");
    let destination = Path::new("web-dist");
    if destination.exists() {
        fs::remove_dir_all(destination).expect("failed to clear generated frontend directory");
    }
    copy_frontend_assets(source, destination).expect("failed to copy frontend assets");

    let mut include_stack = Vec::new();
    let html = render_html_template(&source.join("index.html"), source, &mut include_stack)
        .expect("failed to assemble web/index.html from partials");
    fs::write(destination.join("index.html"), html)
        .expect("failed to write assembled web-dist/index.html");
}

fn main() {
    build_frontend();
    // Skip Tauri asset generation for browser-only server binaries.
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_none() {
        return;
    }

    let icon_path = Path::new("icons/icon.png");
    fs::create_dir_all(icon_path.parent().expect("icon path must have a parent"))
        .expect("failed to create Tauri icons directory");
    create_tauri_icon(icon_path).expect("failed to generate Tauri icon");

    tauri_build::build();
}
