use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::error::log;

pub const ARCHIVE_END_MARKER: &[u8] = b"AGF_TAR_END_v1";
pub const ARCHIVE_CHUNK: usize = 64 * 1024;

pub enum ArchiveMessage {
    Data(Bytes),
    End,
}

/// Builds an uncompressed tar archive on a blocking thread and streams it
/// through a bounded channel. Backpressure keeps disk read-ahead small.
pub fn spawn_tar_archive(
    root: PathBuf,
    archive_root_name: String,
) -> mpsc::Receiver<ArchiveMessage> {
    let (tx, rx) = mpsc::channel(8);
    tokio::task::spawn_blocking(
        move || match build_tar(root, archive_root_name, tx.clone()) {
            Ok(mut writer) => {
                let flushed = writer.flush();
                drop(writer);
                if flushed.is_ok() {
                    if tx.blocking_send(ArchiveMessage::End).is_err() {
                        log("目录归档客户端已断开");
                    }
                } else {
                    log("目录归档发送失败");
                }
            }
            Err(err) => log(&format!("目录归档构建失败: {err}")),
        },
    );
    rx
}

fn build_tar(
    root: PathBuf,
    archive_root_name: String,
    tx: mpsc::Sender<ArchiveMessage>,
) -> io::Result<ArchiveWriter> {
    let writer = ArchiveWriter::new(tx);
    let mut builder = tar::Builder::new(writer);
    append_tree(&mut builder, &root, &archive_root_name)?;
    builder.into_inner()
}

fn append_tree(
    builder: &mut tar::Builder<ArchiveWriter>,
    root: &Path,
    root_name: &str,
) -> io::Result<()> {
    let archive_root = PathBuf::from(root_name);
    builder.append_dir(&archive_root, root)?;

    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let reader = match std::fs::read_dir(&directory) {
            Ok(reader) => reader,
            Err(err) => {
                log(&format!(
                    "跳过无法读取的目录 {}: {err}",
                    directory.display()
                ));
                continue;
            }
        };

        let mut entries: Vec<_> = reader.filter_map(|entry| entry.ok()).collect();
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            let relative = match path.strip_prefix(root) {
                Ok(relative) => relative,
                Err(_) => continue,
            };
            let archive_path = archive_root.join(relative);

            if file_type.is_dir() {
                builder.append_dir(&archive_path, &path)?;
                stack.push(path);
            } else if file_type.is_file() {
                let mut file = match std::fs::File::open(&path) {
                    Ok(file) => file,
                    Err(err) => {
                        log(&format!("跳过无法读取的文件 {}: {err}", path.display()));
                        continue;
                    }
                };
                builder.append_file(&archive_path, &mut file)?;
            }
        }
    }
    Ok(())
}

struct ArchiveWriter {
    tx: mpsc::Sender<ArchiveMessage>,
    buffer: Vec<u8>,
    disconnected: bool,
}

impl ArchiveWriter {
    fn new(tx: mpsc::Sender<ArchiveMessage>) -> Self {
        Self {
            tx,
            buffer: Vec::with_capacity(ARCHIVE_CHUNK),
            disconnected: false,
        }
    }

    fn send_chunk(&mut self, chunk: Vec<u8>) -> io::Result<()> {
        if self.disconnected {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "client disconnected",
            ));
        }
        if self
            .tx
            .blocking_send(ArchiveMessage::Data(Bytes::from(chunk)))
            .is_err()
        {
            self.disconnected = true;
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "client disconnected",
            ));
        }
        Ok(())
    }
}

impl Write for ArchiveWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(data);
        while self.buffer.len() >= ARCHIVE_CHUNK {
            let chunk: Vec<u8> = self.buffer.drain(..ARCHIVE_CHUNK).collect();
            self.send_chunk(chunk)?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::take(&mut self.buffer);
        self.send_chunk(chunk)
    }
}

impl Drop for ArchiveWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}
