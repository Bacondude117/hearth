//! Custom server icons: any picture the user chooses, center-cropped to a square and stored as
//! PNG in the instance folder. Games that show an icon to players (Minecraft Java's
//! `server-icon.png`, 64x64) get a copy in the server folder too.

use std::path::{Path, PathBuf};

use base64::Engine;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};

use crate::error::{Error, IoContext, Result};

/// Size of the icon Hearth shows in its own UI.
pub const UI_SIZE: u32 = 128;
/// Refuse absurdly large files before decoding them.
const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;
pub const ICON_FILE: &str = "icon.png";

pub fn icon_path(instance_dir: &Path) -> PathBuf {
    instance_dir.join(ICON_FILE)
}

fn load_square(src: &Path) -> Result<DynamicImage> {
    let size = std::fs::metadata(src).at(src)?.len();
    if size > MAX_FILE_BYTES {
        return Err(Error::InvalidInput("That image is too large (25 MB max).".into()));
    }
    let img = image::ImageReader::open(src)
        .at(src)?
        .with_guessed_format()
        .at(src)?
        .decode()
        .map_err(|_| Error::InvalidInput("Hearth can't read that image. Use a PNG, JPEG, WebP or GIF.".into()))?;
    let side = img.width().min(img.height());
    if side == 0 {
        return Err(Error::InvalidInput("That image is empty.".into()));
    }
    let (x, y) = ((img.width() - side) / 2, (img.height() - side) / 2);
    Ok(img.crop_imm(x, y, side, side))
}

fn save_png(img: &DynamicImage, size: u32, dest: &Path) -> Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).at(dir)?;
    }
    img.resize_exact(size, size, FilterType::Lanczos3)
        .to_rgba8()
        .save_with_format(dest, ImageFormat::Png)
        .map_err(|e| Error::InvalidInput(format!("Couldn't save the icon: {e}")))
}

/// Stores `src` as the server's icon. `game_icon` is the game's own icon file (name, size)
/// inside the server folder, if the game has one.
pub fn set_icon(src: &Path, instance_dir: &Path, server_dir: &Path, game_icon: Option<(&str, u32)>) -> Result<()> {
    let img = load_square(src)?;
    save_png(&img, UI_SIZE, &icon_path(instance_dir))?;
    if let Some((name, size)) = game_icon {
        save_png(&img, size, &server_dir.join(name))?;
    }
    Ok(())
}

pub fn clear_icon(instance_dir: &Path, server_dir: &Path, game_icon: Option<(&str, u32)>) -> Result<()> {
    let mut targets = vec![icon_path(instance_dir)];
    if let Some((name, _)) = game_icon {
        targets.push(server_dir.join(name));
    }
    for p in targets {
        if p.exists() {
            std::fs::remove_file(&p).at(&p)?;
        }
    }
    Ok(())
}

/// Changes whenever the icon changes, so the UI knows to reload it.
pub fn icon_version(instance_dir: &Path) -> Option<u64> {
    let m = std::fs::metadata(icon_path(instance_dir)).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(t.as_millis() as u64 ^ m.len())
}

/// The icon as a `data:image/png;base64,...` URL for the web view.
pub fn icon_data_url(instance_dir: &Path) -> Result<Option<String>> {
    let p = icon_path(instance_dir);
    if !p.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&p).at(&p)?;
    Ok(Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_image(path: &Path, w: u32, h: u32, format: ImageFormat) {
        let img = image::RgbaImage::from_fn(w, h, |x, _| {
            if x < w / 2 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        });
        DynamicImage::ImageRgba8(img).save_with_format(path, format).unwrap();
    }

    #[test]
    fn crops_resizes_and_writes_the_game_icon() {
        let t = tempfile::tempdir().unwrap();
        let (inst, server) = (t.path().join("inst"), t.path().join("inst/server"));
        let src = t.path().join("photo.jpg");
        write_test_image(&src, 300, 200, ImageFormat::Jpeg);

        set_icon(&src, &inst, &server, Some(("server-icon.png", 64))).unwrap();
        let ui = image::open(icon_path(&inst)).unwrap();
        assert_eq!((ui.width(), ui.height()), (UI_SIZE, UI_SIZE));
        let game = image::open(server.join("server-icon.png")).unwrap();
        assert_eq!(
            (game.width(), game.height()),
            (64, 64),
            "Minecraft requires exactly 64x64"
        );
        assert_eq!(
            image::ImageFormat::from_path(server.join("server-icon.png")).unwrap(),
            ImageFormat::Png
        );

        let v1 = icon_version(&inst).unwrap();
        assert!(icon_data_url(&inst)
            .unwrap()
            .unwrap()
            .starts_with("data:image/png;base64,iVBOR"));

        clear_icon(&inst, &server, Some(("server-icon.png", 64))).unwrap();
        assert!(!icon_path(&inst).exists() && !server.join("server-icon.png").exists());
        assert_eq!(icon_version(&inst), None);
        assert_eq!(icon_data_url(&inst).unwrap(), None);
        let _ = v1;
    }

    #[test]
    fn games_without_an_icon_file_only_get_the_ui_icon() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("a.png");
        write_test_image(&src, 50, 80, ImageFormat::Png);
        set_icon(&src, t.path(), &t.path().join("server"), None).unwrap();
        assert!(icon_path(t.path()).exists());
        assert!(!t.path().join("server").exists());
    }

    #[test]
    fn rejects_non_images() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("notes.png");
        std::fs::write(&src, b"definitely not a png").unwrap();
        let e = set_icon(&src, t.path(), t.path(), None).unwrap_err();
        assert!(e.to_string().contains("can't read that image"), "{e}");
    }
}
