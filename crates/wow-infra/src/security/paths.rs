use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

pub fn safe_join(root: &Path, rel: &Path) -> Result<PathBuf, String> {
    if rel.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("unsafe path".into());
    }
    Ok(root.join(rel))
}

pub fn reject_symlink_path(path: &Path) -> io::Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if let Ok(md) = fs::symlink_metadata(&current) {
            if md.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("protected path traverses symlink: {}", current.display()),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
pub fn secure_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    reject_symlink_path(path)?;
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private directory is accessible by group or others",
        ));
    }
    Ok(())
}

#[cfg(unix)]
pub fn secure_private_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        secure_private_dir(parent)?;
    }
    reject_symlink_path(path)?;
    if !path.exists() {
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    let mode = fs::metadata(path)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private file is accessible by group or others",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn secure_private_dir(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private ACL enforcement is not implemented on this platform",
    ))
}
#[cfg(not(unix))]
pub fn secure_private_file(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "private ACL enforcement is not implemented on this platform",
    ))
}
