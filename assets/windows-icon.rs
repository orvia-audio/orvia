//! Render the shared SVG at Windows icon sizes and link it into the executable.
use resvg::{tiny_skia, usvg};
use std::{env, error::Error, fs, io, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let source = PathBuf::from("../../assets/orvia.svg");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed=../../assets/windows-icon.rs");
    let tree = usvg::Tree::from_data(&fs::read(source)?, &usvg::Options::default())?;
    let dir = PathBuf::from(env::var("OUT_DIR")?);
    let sizes = [16_u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let mut directory = vec![0, 0, 1, 0, sizes.len() as u8, 0];
    let mut images = Vec::new();
    for size in sizes {
        let mut pixmap =
            tiny_skia::Pixmap::new(size, size).ok_or(io::Error::other("Invalid icon size"))?;
        let scale = size as f32 / tree.size().width();
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        let png = pixmap.encode_png()?;
        if size == 64 {
            fs::write(dir.join("orvia.png"), &png)?;
        }
        directory.extend([size as u8, size as u8, 0, 0, 1, 0, 32, 0]);
        directory.extend((png.len() as u32).to_le_bytes());
        directory.extend(((6 + sizes.len() * 16 + images.len()) as u32).to_le_bytes());
        images.extend(png);
    }
    directory.extend(images);
    let icon = dir.join("orvia.ico");
    fs::write(&icon, directory)?;
    let package = env::var("CARGO_PKG_NAME")?;
    let resource = dir.join("orvia.rc");
    // GPUI loads icon 1. The notification area uses a named resource in the engine.
    let id = if package == "orvia-engine" {
        "ORVIA_ICON"
    } else {
        "1"
    };
    fs::write(
        &resource,
        format!(
            "{id} ICON \"{}\"\n",
            icon.display().to_string().replace('\\', "/")
        ),
    )?;
    embed_resource::compile_for(resource, [package], embed_resource::NONE).manifest_required()?;
    Ok(())
}
