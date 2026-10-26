use super::{HostedResult, INVALID, Table};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Component, Path, PathBuf},
};

/// Reject symlinks in every path component, including supplied ancestors.
pub(super) fn checked_directory(path: &Path) -> HostedResult<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut prefix = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::CurDir | Component::ParentDir) {
            return Err(INVALID.into());
        }
        prefix.push(part);
        let metadata = prefix.symlink_metadata()?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(INVALID.into());
        }
    }
    Ok(path)
}

pub(super) fn new_directory(path: &Path) -> HostedResult<PathBuf> {
    let name = path.file_name().ok_or(INVALID)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = checked_directory(parent)?;
    let destination = parent.join(name);
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&destination)?;
    checked_directory(&destination)
}

pub(super) fn new_file(path: &Path) -> HostedResult<File> {
    checked_directory(path.parent().ok_or(INVALID)?)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

pub(super) fn open_file(root: &Path, relative: &str) -> HostedResult<BufReader<File>> {
    // Manifest paths must already equal a compiled profile path. This also
    // protects the reusable reader against unsafe strings supplied by callers.
    if relative.is_empty()
        || Path::new(relative)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(INVALID.into());
    }
    let path = root.join(relative);
    checked_directory(path.parent().ok_or(INVALID)?)?;
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(INVALID.into());
    }
    let file = File::open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
            return Err(INVALID.into());
        }
    }
    if path.symlink_metadata()?.file_type().is_symlink() {
        return Err(INVALID.into());
    }
    Ok(BufReader::new(file))
}

pub(super) fn bounded_read(root: &Path, relative: &str, limit: usize) -> HostedResult<Vec<u8>> {
    let mut bytes = Vec::new();
    open_file(root, relative)?
        .take(u64::try_from(limit + 1)?)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(INVALID.into());
    }
    Ok(bytes)
}

pub(super) struct RowWriter {
    file: File,
    digest: Sha256,
    table: Table,
}
impl RowWriter {
    pub fn new(root: &Path, prefix: &str, name: &str) -> HostedResult<Self> {
        let relative = format!("{prefix}/{name}.jsonl");
        Ok(Self {
            file: new_file(&root.join(&relative))?,
            digest: Sha256::new(),
            table: Table {
                name: name.into(),
                file: relative,
                rows: 0,
                sha256: String::new(),
            },
        })
    }
    pub fn write(&mut self, bytes: &[u8]) -> HostedResult<()> {
        if bytes.len() > meibo_core::remote_snapshot::MAX_ROW_BYTES {
            return Err(INVALID.into());
        }
        self.file.write_all(bytes)?;
        self.file.write_all(b"\n")?;
        self.digest.update(bytes);
        self.digest.update(b"\n");
        self.table.rows = self.table.rows.checked_add(1).ok_or(INVALID)?;
        Ok(())
    }
    pub fn finish(mut self) -> HostedResult<Table> {
        self.file.flush()?;
        self.file.sync_all()?;
        self.table.sha256 = format!("{:x}", self.digest.finalize());
        Ok(self.table)
    }
}
