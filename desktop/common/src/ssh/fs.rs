//! Remote filesystem operations over SFTP.
//!
//! Logical paths are always `/`-separated and relative to a canonical remote
//! root. They are validated with the same rules the local API uses, then joined
//! to the root; every resolved path is re-checked so a symlink or `..` cannot
//! escape the configured directory.

use std::{path::Path, sync::Arc};

use russh_sftp::{client::fs::File, client::SftpSession};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const COPY_CHUNK: usize = 64 * 1024;
const MAX_LIST_ENTRIES: usize = 50_000;
const MAX_DEPTH: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsErrorKind {
    NotFound,
    Conflict,
    Forbidden,
    Internal,
}

#[derive(Debug)]
pub struct FsError {
    pub kind: FsErrorKind,
    pub message: String,
}

impl FsError {
    pub fn new(kind: FsErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(FsErrorKind::NotFound, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(FsErrorKind::Conflict, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(FsErrorKind::Forbidden, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(FsErrorKind::Internal, message)
    }
}

impl std::fmt::Display for FsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for FsError {}

#[derive(Clone, Debug)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub hidden: bool,
    pub size: u64,
    pub modified: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct RemoteAttr {
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub modified: u64,
}

/// Validates a logical path: no `..`, no NUL, no over-long components.
pub fn clean_rel(input: &str) -> Result<String, FsError> {
    let mut parts = Vec::new();
    for component in input.split('/') {
        match component {
            "" | "." => continue,
            ".." => return Err(FsError::forbidden("路径不合法")),
            _ => {}
        }
        if component.as_bytes().contains(&0) {
            return Err(FsError::forbidden("路径包含非法字符"));
        }
        if component.len() > 255 {
            return Err(FsError::forbidden("路径片段过长"));
        }
        parts.push(component);
    }
    Ok(parts.join("/"))
}

pub fn validate_name(name: &str) -> Result<(), FsError> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(FsError::forbidden("文件名不合法"));
    }
    if name.contains('/') || name.as_bytes().contains(&0) {
        return Err(FsError::forbidden("文件名包含非法字符"));
    }
    if name.len() > 255 {
        return Err(FsError::forbidden("文件名过长"));
    }
    Ok(())
}

pub struct SftpFs {
    sftp: Arc<SftpSession>,
    root: String,
}

impl SftpFs {
    pub fn new(sftp: Arc<SftpSession>, root: String) -> Self {
        Self {
            sftp,
            root: root.trim_end_matches('/').to_string(),
        }
    }

    pub fn root(&self) -> &str {
        if self.root.is_empty() {
            "/"
        } else {
            &self.root
        }
    }

    pub fn session(&self) -> &Arc<SftpSession> {
        &self.sftp
    }

    /// Absolute remote path for a logical path.
    pub fn join(&self, clean: &str) -> String {
        let root = self.root.trim_end_matches('/');
        if clean.is_empty() {
            if root.is_empty() {
                "/".to_string()
            } else {
                root.to_string()
            }
        } else if root.is_empty() {
            format!("/{clean}")
        } else {
            format!("{root}/{clean}")
        }
    }

    fn ensure_inside_root(&self, absolute: &str) -> Result<(), FsError> {
        let root = self.root.trim_end_matches('/');
        if root.is_empty() {
            return Ok(());
        }
        if absolute == root || absolute.starts_with(&format!("{root}/")) {
            Ok(())
        } else {
            Err(FsError::forbidden("路径越界"))
        }
    }

    /// Resolves an existing logical path to its canonical absolute form.
    pub async fn resolve_existing(&self, clean: &str) -> Result<String, FsError> {
        let joined = self.join(clean);
        let canonical = self
            .sftp
            .canonicalize(&joined)
            .await
            .map_err(|_| FsError::not_found("文件或文件夹不存在"))?;
        self.ensure_inside_root(&canonical)?;
        Ok(canonical)
    }

    /// Resolves the parent of a target that may not exist yet.
    pub async fn resolve_parent(&self, clean: &str) -> Result<String, FsError> {
        let trimmed = match clean.rfind('/') {
            Some(index) => &clean[..index],
            None => "",
        };
        let canonical = self.resolve_existing(trimmed).await?;
        Ok(canonical)
    }

    pub async fn list(&self, clean: &str) -> Result<(Vec<RemoteEntry>, bool), FsError> {
        let directory = self.resolve_existing(clean).await?;
        let mut entries = Vec::new();
        let mut truncated = false;
        let mut reader = self
            .sftp
            .read_dir(&directory)
            .await
            .map_err(|err| FsError::internal(format!("读取目录失败：{err}")))?;
        while let Some(entry) = reader.next() {
            if entries.len() >= MAX_LIST_ENTRIES {
                truncated = true;
                break;
            }
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let file_type = entry.file_type();
            let metadata = entry.metadata();
            let is_symlink = file_type.is_symlink();
            let mut is_dir = file_type.is_dir();
            let mut size = metadata.size.unwrap_or(0);
            let mut modified = metadata.mtime.map(u64::from).unwrap_or(0);
            if is_symlink {
                // STAT follows the link; a broken link is shown as a file.
                if let Ok(target) = self.sftp.metadata(entry.path()).await {
                    is_dir = target.is_dir();
                    size = target.size.unwrap_or(0);
                    modified = target.mtime.map(u64::from).unwrap_or(0);
                }
            }
            entries.push(RemoteEntry {
                hidden: name.starts_with('.'),
                name,
                is_dir,
                is_symlink,
                size,
                modified,
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok((entries, truncated))
    }

    pub async fn stat(&self, clean: &str) -> Result<Option<RemoteAttr>, FsError> {
        let joined = self.join(clean);
        let attrs = match self.sftp.symlink_metadata(&joined).await {
            Ok(attrs) => attrs,
            Err(_) => return Ok(None),
        };
        let is_symlink = attrs.is_symlink();
        let mut is_dir = attrs.is_dir();
        let mut size = attrs.size.unwrap_or(0);
        let mut modified = attrs.mtime.map(u64::from).unwrap_or(0);
        if is_symlink {
            if let Ok(target) = self.sftp.metadata(&joined).await {
                is_dir = target.is_dir();
                size = target.size.unwrap_or(0);
                modified = target.mtime.map(u64::from).unwrap_or(0);
            }
        }
        Ok(Some(RemoteAttr {
            is_dir,
            is_symlink,
            size,
            modified,
        }))
    }

    /// Creates every missing component; returns whether anything was created.
    pub async fn ensure_dir(&self, clean: &str) -> Result<bool, FsError> {
        if clean.is_empty() {
            return Ok(false);
        }
        let mut current = self.root.trim_end_matches('/').to_string();
        let mut created = false;
        for component in clean.split('/').filter(|part| !part.is_empty()) {
            current.push('/');
            current.push_str(component);
            match self.sftp.symlink_metadata(&current).await {
                Ok(attrs) => {
                    if !attrs.is_dir() {
                        return Err(FsError::conflict("路径中存在同名文件"));
                    }
                }
                Err(_) => {
                    self.sftp
                        .create_dir(&current)
                        .await
                        .map_err(|err| FsError::internal(format!("创建目录失败：{err}")))?;
                    created = true;
                }
            }
        }
        Ok(created)
    }

    pub async fn rename(&self, clean: &str, new_name: &str) -> Result<(), FsError> {
        validate_name(new_name)?;
        let parent = match clean.rfind('/') {
            Some(index) => &clean[..index],
            None => "",
        };
        let canonical_parent = self.resolve_existing(parent).await?;
        let source = self.resolve_existing(clean).await?;
        let target = format!("{}/{}", canonical_parent.trim_end_matches('/'), new_name);
        self.ensure_inside_root(&target)?;
        if source == target {
            return Ok(());
        }
        if self.sftp.symlink_metadata(&target).await.is_ok() {
            return Err(FsError::conflict("同名文件或文件夹已存在"));
        }
        self.sftp
            .rename(&source, &target)
            .await
            .map_err(|err| FsError::internal(format!("重命名失败：{err}")))?;
        Ok(())
    }

    pub async fn delete(&self, clean: &str) -> Result<(), FsError> {
        let canonical = self.resolve_existing(clean).await?;
        if canonical == self.root.trim_end_matches('/') {
            return Err(FsError::forbidden("不能删除根目录"));
        }
        let attrs = self
            .sftp
            .symlink_metadata(&canonical)
            .await
            .map_err(|err| FsError::internal(format!("读取目标失败：{err}")))?;
        if !attrs.is_dir() || attrs.is_symlink() {
            return self
                .sftp
                .remove_file(&canonical)
                .await
                .map_err(|err| FsError::internal(format!("删除失败：{err}")));
        }
        let mut dirs = Vec::new();
        let mut stack = vec![(canonical, 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if depth > MAX_DEPTH {
                return Err(FsError::internal("目录层级过深"));
            }
            let mut reader = self
                .sftp
                .read_dir(&dir)
                .await
                .map_err(|err| FsError::internal(format!("读取目录失败：{err}")))?;
            dirs.push(dir);
            while let Some(entry) = reader.next() {
                let name = entry.file_name();
                if name == "." || name == ".." {
                    continue;
                }
                let path = entry.path();
                let file_type = entry.file_type();
                if file_type.is_dir() && !file_type.is_symlink() {
                    stack.push((path, depth + 1));
                } else {
                    let _ = self.sftp.remove_file(&path).await;
                }
            }
        }
        for dir in dirs.into_iter().rev() {
            self.sftp
                .remove_dir(&dir)
                .await
                .map_err(|err| FsError::internal(format!("删除目录失败：{err}")))?;
        }
        Ok(())
    }

    pub async fn open_read(&self, clean: &str) -> Result<(File, u64), FsError> {
        let canonical = self.resolve_existing(clean).await?;
        let attrs = self
            .sftp
            .metadata(&canonical)
            .await
            .map_err(|_| FsError::not_found("文件不存在"))?;
        if attrs.is_dir() {
            return Err(FsError::new(FsErrorKind::Conflict, "该路径不是文件"));
        }
        let file = self
            .sftp
            .open(&canonical)
            .await
            .map_err(|err| FsError::internal(format!("打开文件失败：{err}")))?;
        Ok((file, attrs.size.unwrap_or(0)))
    }

    /// Streams a local file to `clean`. The caller has already checked conflicts.
    pub async fn upload(&self, local_path: &Path, clean: &str) -> Result<u64, FsError> {
        let target = self.join(clean);
        let mut source = tokio::fs::File::open(local_path)
            .await
            .map_err(|err| FsError::internal(format!("读取本地临时文件失败：{err}")))?;
        let mut remote = self
            .sftp
            .create(&target)
            .await
            .map_err(|err| FsError::internal(format!("创建远程文件失败：{err}")))?;
        let mut buffer = vec![0u8; COPY_CHUNK];
        let mut written = 0u64;
        loop {
            let read = source
                .read(&mut buffer)
                .await
                .map_err(|err| FsError::internal(format!("读取临时文件失败：{err}")))?;
            if read == 0 {
                break;
            }
            remote
                .write_all(&buffer[..read])
                .await
                .map_err(|err| FsError::internal(format!("写入远程文件失败：{err}")))?;
            written += read as u64;
        }
        // fsync is an OpenSSH extension; a server without it still has the data
        // in flight, so treat the failure as non-fatal.
        let _ = remote.sync_all().await;
        let _ = remote.close().await;
        Ok(written)
    }

    pub async fn available_space(&self, clean: &str) -> Option<u64> {
        let path = self.resolve_existing(clean).await.ok()?;
        let stat = self.sftp.fs_info(path).await.ok()??;
        Some(stat.blocks_avail.saturating_mul(stat.fragment_size))
    }

    pub async fn realpath(&self, absolute: &str) -> Result<String, FsError> {
        self.sftp
            .canonicalize(absolute)
            .await
            .map_err(|_| FsError::not_found("目录不存在"))
    }

    pub async fn is_dir(&self, absolute: &str) -> bool {
        self.sftp
            .metadata(absolute)
            .await
            .map(|attrs| attrs.is_dir())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_keeps_normal_paths() {
        assert_eq!(clean_rel("/a/b/./c").unwrap(), "a/b/c");
        assert_eq!(clean_rel("/").unwrap(), "");
        assert!(clean_rel("a/../b").is_err());
        assert!(clean_rel("a/\0b").is_err());
        assert!(validate_name("ok.txt").is_ok());
        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
    }
}
