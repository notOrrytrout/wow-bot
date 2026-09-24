use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};
const OWNER_MARKER: &str = ".wow-bot-owned";
pub fn adopt_managed_dir(path: &Path) -> io::Result<PathBuf> {
    if path.exists() {
        let mut entries = fs::read_dir(path)?;
        let nonempty = entries.next().transpose()?.is_some();
        if nonempty && !path.join(OWNER_MARKER).exists() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "non-empty log directory lacks ownership marker",
            ));
        }
    }
    fs::create_dir_all(path)?;
    if !path.join(OWNER_MARKER).exists() {
        fs::write(path.join(OWNER_MARKER), b"wow-bot managed logs\n")?;
    }
    Ok(path.to_path_buf())
}
pub struct JsonlRunLog {
    file: File,
}
impl JsonlRunLog {
    pub fn open(p: impl AsRef<Path>) -> io::Result<Self> {
        Ok(Self {
            file: OpenOptions::new().create(true).append(true).open(p)?,
        })
    }
    pub fn append_json(&mut self, line: &str) -> io::Result<()> {
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.file.flush()
    }
}
