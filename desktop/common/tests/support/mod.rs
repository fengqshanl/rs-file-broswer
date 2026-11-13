//! Shared in-process SSH/SFTP server for the desktop test suites.
//!
//! A minimal SSH server speaks real SFTP over a real TCP socket, backed by a
//! temporary directory. It is used by `ssh_e2e` (file API) and `ssh_companion`
//! (target lifecycle through the desktop companion).

use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use russh::{
    server::{Auth, ChannelOpenHandle, Msg, Session, VerifiedPublicKey},
    Channel, ChannelId,
};
use russh_sftp::{
    protocol::{
        Attrs, Data, File as SftpFile, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode,
    },
    server::StatusReply,
};

pub const PASSWORD: &str = "test-secret";
/// Test-only host key (generated for this suite; it protects nothing).
pub const HOST_KEY: &str = include_str!("../fixtures/test_host_key");

// ---------- Fake SFTP server ----------

enum HandleKind {
    File(std::fs::File),
    Dir { entries: Vec<SftpFile>, sent: bool },
}

#[derive(Default)]
struct TestSftp {
    fs_root: PathBuf,
    handles: HashMap<String, HandleKind>,
    next: u64,
}

impl TestSftp {
    fn new(fs_root: PathBuf) -> Self {
        Self {
            fs_root: std::fs::canonicalize(&fs_root).unwrap_or(fs_root),
            ..Default::default()
        }
    }

    /// Maps the POSIX path the client sees onto the temporary directory.
    fn local(&self, remote: &str) -> PathBuf {
        let mut path = self.fs_root.clone();
        for part in remote.split('/') {
            if part.is_empty() || part == "." || part == ".." {
                continue;
            }
            path.push(part);
        }
        path
    }

    fn virtual_path(&self, local: &Path) -> String {
        let relative = local.strip_prefix(&self.fs_root).unwrap_or(local);
        let text = relative.to_string_lossy().replace('\\', "/");
        if text.is_empty() {
            "/".to_string()
        } else {
            format!("/{text}")
        }
    }
}

fn ok_status(id: u32) -> Status {
    Status {
        id,
        status_code: StatusCode::Ok,
        error_message: String::new(),
        language_tag: "en-US".to_string(),
    }
}

fn missing() -> StatusReply {
    StatusReply::new(StatusCode::NoSuchFile)
}

fn attrs_of(meta: &std::fs::Metadata) -> FileAttributes {
    let mode = if meta.is_dir() { 0o040755 } else { 0o100644 };
    FileAttributes {
        size: Some(meta.len()),
        permissions: Some(mode),
        mtime: meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as u32),
        ..Default::default()
    }
}

fn sftp_file(name: String, attrs: FileAttributes) -> SftpFile {
    SftpFile::new(name, attrs)
}

impl russh_sftp::server::Handler for TestSftp {
    type Error = StatusReply;

    fn unimplemented(&self) -> StatusReply {
        StatusReply::new(StatusCode::OpUnsupported)
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, StatusReply> {
        let mut options = std::fs::OpenOptions::new();
        if pflags.contains(OpenFlags::READ) {
            options.read(true);
        }
        if pflags.contains(OpenFlags::WRITE) {
            options.write(true);
        }
        if pflags.contains(OpenFlags::CREATE) {
            options.create(true);
        }
        if pflags.contains(OpenFlags::TRUNCATE) {
            options.truncate(true);
        }
        let file = options.open(self.local(&filename)).map_err(|_| missing())?;
        self.next += 1;
        let handle = format!("h{}", self.next);
        self.handles.insert(handle.clone(), HandleKind::File(file));
        Ok(Handle { id, handle })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, StatusReply> {
        self.handles.remove(&handle);
        Ok(ok_status(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, StatusReply> {
        let Some(HandleKind::File(file)) = self.handles.get_mut(&handle) else {
            return Err(missing());
        };
        let size = file.metadata().map_err(|_| missing())?.len();
        if offset >= size {
            return Err(StatusReply::new(StatusCode::Eof));
        }
        file.seek(SeekFrom::Start(offset)).map_err(|_| missing())?;
        let want = (size - offset).min(u64::from(len)) as usize;
        let mut buffer = vec![0u8; want];
        let read = file.read(&mut buffer).map_err(|_| missing())?;
        buffer.truncate(read);
        Ok(Data { id, data: buffer })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, StatusReply> {
        let Some(HandleKind::File(file)) = self.handles.get_mut(&handle) else {
            return Err(missing());
        };
        file.seek(SeekFrom::Start(offset)).map_err(|_| missing())?;
        file.write_all(&data).map_err(|_| missing())?;
        Ok(ok_status(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, StatusReply> {
        let meta = std::fs::symlink_metadata(self.local(&path)).map_err(|_| missing())?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&meta),
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, StatusReply> {
        let meta = std::fs::metadata(self.local(&path)).map_err(|_| missing())?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&meta),
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, StatusReply> {
        let Some(HandleKind::File(file)) = self.handles.get(&handle) else {
            return Err(missing());
        };
        let meta = file.metadata().map_err(|_| missing())?;
        Ok(Attrs {
            id,
            attrs: attrs_of(&meta),
        })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, StatusReply> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(self.local(&path)).map_err(|_| missing())? {
            let entry = entry.map_err(|_| missing())?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "." || name == ".." {
                continue;
            }
            let meta = entry.metadata().map_err(|_| missing())?;
            entries.push(sftp_file(name, attrs_of(&meta)));
        }
        self.next += 1;
        let handle = format!("d{}", self.next);
        self.handles.insert(
            handle.clone(),
            HandleKind::Dir {
                entries,
                sent: false,
            },
        );
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, StatusReply> {
        let Some(HandleKind::Dir { entries, sent }) = self.handles.get_mut(&handle) else {
            return Err(missing());
        };
        if *sent {
            return Err(StatusReply::new(StatusCode::Eof));
        }
        *sent = true;
        Ok(Name {
            id,
            files: std::mem::take(entries),
        })
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, StatusReply> {
        std::fs::remove_file(self.local(&filename)).map_err(|_| missing())?;
        Ok(ok_status(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, StatusReply> {
        std::fs::create_dir(self.local(&path)).map_err(|_| missing())?;
        Ok(ok_status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, StatusReply> {
        std::fs::remove_dir(self.local(&path)).map_err(|_| missing())?;
        Ok(ok_status(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, StatusReply> {
        let canonical = std::fs::canonicalize(self.local(&path)).map_err(|_| missing())?;
        Ok(Name {
            id,
            files: vec![SftpFile::dummy(self.virtual_path(&canonical))],
        })
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, StatusReply> {
        std::fs::rename(self.local(&oldpath), self.local(&newpath)).map_err(|_| missing())?;
        Ok(ok_status(id))
    }

    async fn setstat(
        &mut self,
        id: u32,
        _path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, StatusReply> {
        Ok(ok_status(id))
    }
}

struct Connection {
    password: String,
    fs_root: PathBuf,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl russh::server::Handler for Connection {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == self.password {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }

    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        self.channels.insert(channel.id(), channel);
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel_id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        let Some(channel) = self.channels.remove(&channel_id) else {
            session.channel_failure(channel_id)?;
            return Ok(());
        };
        session.channel_success(channel_id)?;
        tokio::spawn(russh_sftp::server::run(
            channel.into_stream(),
            TestSftp::new(self.fs_root.clone()),
        ));
        Ok(())
    }
}

pub async fn start_ssh_server(password: &str, fs_root: PathBuf) -> u16 {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind fake ssh server");
    let port = listener.local_addr().unwrap().port();
    let key = russh::keys::PrivateKey::from_openssh(HOST_KEY).expect("parse test host key");
    let config = Arc::new(russh::server::Config {
        keys: vec![key],
        inactivity_timeout: Some(Duration::from_secs(120)),
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::from_millis(0)),
        ..Default::default()
    });
    let password = password.to_string();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _peer)) = listener.accept().await else {
                break;
            };
            let handler = Connection {
                password: password.clone(),
                fs_root: fs_root.clone(),
                channels: HashMap::new(),
            };
            let config = config.clone();
            tokio::spawn(async move {
                if let Err(err) = russh::server::run_stream(config, stream, handler).await {
                    eprintln!("test ssh session ended: {err}");
                }
            });
        }
    });
    port
}
