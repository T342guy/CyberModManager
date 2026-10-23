use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zip,
    SevenZ,
    Rar,
}

/// Detect the archive type from its magic bytes, falling back to the extension.
pub fn detect(path: &Path) -> Result<Kind> {
    use std::io::Read;
    let mut magic = [0u8; 6];
    let n = File::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .read(&mut magic)?;
    let magic = &magic[..n];
    if magic.starts_with(b"PK\x03\x04") || magic.starts_with(b"PK\x05\x06") {
        return Ok(Kind::Zip);
    }
    if magic.starts_with(b"7z\xBC\xAF\x27\x1C") {
        return Ok(Kind::SevenZ);
    }
    if magic.starts_with(b"Rar!") {
        return Ok(Kind::Rar);
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "zip" => Ok(Kind::Zip),
        "7z" => Ok(Kind::SevenZ),
        "rar" => Ok(Kind::Rar),
        _ => bail!("{} is not a zip, 7z or rar archive", path.display()),
    }
}

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .map(|e| matches!(e.to_string_lossy().to_lowercase().as_str(), "zip" | "7z" | "rar"))
        .unwrap_or(false)
}

/// Extract `archive` into `dest` (which should be an empty directory).
pub fn extract(archive: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let what = || format!("extracting {}", archive.display());
    match detect(archive)? {
        Kind::Zip => {
            let file = File::open(archive).with_context(what)?;
            let mut zip = zip::ZipArchive::new(file).with_context(what)?;
            zip.extract(dest).with_context(what)?;
        }
        Kind::SevenZ => {
            sevenz_rust2::decompress_file(archive, dest).with_context(what)?;
        }
        Kind::Rar => {
            let mut rar = unrar::Archive::new(archive)
                .open_for_processing()
                .with_context(what)?;
            while let Some(header) = rar.read_header().with_context(what)? {
                rar = if header.entry().is_file() {
                    header.extract_with_base(dest).with_context(what)?
                } else {
                    header.skip().with_context(what)?
                };
            }
        }
    }
    make_owner_writable(dest)
}

/// Archives made on Windows can carry the read-only attribute on folders,
/// which extracts as mode 0555 and makes moving files out fail with EACCES.
pub fn make_owner_writable(root: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for entry in walkdir::WalkDir::new(root).into_iter().flatten() {
            let ft = entry.file_type();
            if ft.is_symlink() {
                continue;
            }
            let path = entry.path();
            let mode = entry.metadata()?.permissions().mode();
            let wanted = if ft.is_dir() { mode | 0o700 } else { mode | 0o600 };
            if wanted != mode {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(wanted))
                    .with_context(|| format!("fixing permissions on {}", path.display()))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn extracts_zip() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("m.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("archive/pc/mod/a.archive", opts).unwrap();
            w.write_all(b"hello").unwrap();
            w.finish().unwrap();
        }
        assert_eq!(detect(&zip_path).unwrap(), Kind::Zip);
        let out = tmp.path().join("out");
        extract(&zip_path, &out).unwrap();
        assert_eq!(std::fs::read(out.join("archive/pc/mod/a.archive")).unwrap(), b"hello");
    }

    #[test]
    fn read_only_dirs_become_writable() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("ro.zip");
        {
            let mut w = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let ro = zip::write::SimpleFileOptions::default().unix_permissions(0o555);
            w.add_directory("r6/tweaks/Mod/", ro).unwrap();
            w.start_file("r6/tweaks/Mod/a.yaml", ro).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let out = tmp.path().join("out");
        extract(&zip_path, &out).unwrap();
        // Moving a file out of the directory must work.
        std::fs::rename(out.join("r6/tweaks/Mod/a.yaml"), tmp.path().join("a.yaml")).unwrap();
    }

    #[test]
    fn extracts_7z() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("r6/scripts")).unwrap();
        std::fs::write(src.join("r6/scripts/x.reds"), b"x").unwrap();
        let archive = tmp.path().join("m.7z");
        sevenz_rust2::compress_to_path(&src, &archive).unwrap();
        assert_eq!(detect(&archive).unwrap(), Kind::SevenZ);
        let out = tmp.path().join("out");
        extract(&archive, &out).unwrap();
        assert!(out.join("r6/scripts/x.reds").is_file());
    }
}
