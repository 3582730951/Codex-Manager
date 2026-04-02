use bytes::Bytes;
use rand::RngCore;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DEFAULT_REQUEST_SPILL_DIR_NAME: &str = "codexmanager-request-spool";

#[derive(Debug)]
pub(crate) enum RequestPayloadBuildError {
    Io(io::Error),
    TooLarge { max_body_bytes: usize },
}

impl RequestPayloadBuildError {
    pub(crate) fn is_too_large(&self) -> bool {
        matches!(self, Self::TooLarge { .. })
    }

    pub(crate) fn max_body_bytes(&self) -> Option<usize> {
        match self {
            Self::TooLarge { max_body_bytes } => Some(*max_body_bytes),
            Self::Io(_) => None,
        }
    }
}

impl std::fmt::Display for RequestPayloadBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::TooLarge { max_body_bytes } => {
                write!(f, "request body too large: content-length>{max_body_bytes}")
            }
        }
    }
}

impl std::error::Error for RequestPayloadBuildError {}

impl From<io::Error> for RequestPayloadBuildError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone)]
enum RequestPayloadStorage {
    Memory(Bytes),
    File(Arc<RequestPayloadFile>),
}

#[derive(Debug)]
struct RequestPayloadFile {
    path: PathBuf,
    len: usize,
}

impl Drop for RequestPayloadFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RequestPayload {
    storage: RequestPayloadStorage,
    len: usize,
}

impl RequestPayload {
    #[allow(dead_code)]
    pub(crate) fn empty() -> Self {
        Self {
            storage: RequestPayloadStorage::Memory(Bytes::new()),
            len: 0,
        }
    }

    pub(crate) fn from_bytes(bytes: Bytes) -> Result<Self, String> {
        Self::from_vec(bytes.to_vec())
    }

    pub(crate) fn from_vec(bytes: Vec<u8>) -> Result<Self, String> {
        let spill_threshold = crate::gateway::request_spill_threshold_bytes();
        let max_body_bytes = crate::gateway::front_proxy_max_body_bytes();
        let mut builder = RequestPayloadBuilder::new(spill_threshold, max_body_bytes)
            .map_err(|err| err.to_string())?;
        builder
            .append_chunk(bytes.as_slice())
            .map_err(|err| err.to_string())?;
        builder.finish().map_err(|err| err.to_string())
    }

    pub(crate) fn from_reader<R: Read>(
        mut reader: R,
        max_body_bytes: usize,
    ) -> Result<Self, RequestPayloadBuildError> {
        let spill_threshold = crate::gateway::request_spill_threshold_bytes();
        let mut builder = RequestPayloadBuilder::new(spill_threshold, max_body_bytes)?;
        let mut chunk = [0_u8; 8192];
        loop {
            let read = reader.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            builder.append_chunk(&chunk[..read])?;
        }
        builder.finish()
    }

    pub(crate) fn from_json_value(value: &serde_json::Value) -> Result<Self, String> {
        let spill_threshold = crate::gateway::request_spill_threshold_bytes();
        let max_body_bytes = crate::gateway::front_proxy_max_body_bytes();
        let mut builder = RequestPayloadBuilder::new(spill_threshold, max_body_bytes)
            .map_err(|err| err.to_string())?;
        builder
            .serialize_json(value)
            .map_err(|err| err.to_string())?;
        builder.finish().map_err(|err| err.to_string())
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn is_file_backed(&self) -> bool {
        matches!(self.storage, RequestPayloadStorage::File(_))
    }

    pub(crate) fn read_all_bytes(&self) -> Result<Bytes, String> {
        match &self.storage {
            RequestPayloadStorage::Memory(bytes) => Ok(bytes.clone()),
            RequestPayloadStorage::File(file) => {
                let mut reader = File::open(&file.path)
                    .map_err(|err| format!("open request payload file failed: {err}"))?;
                let mut out = Vec::with_capacity(file.len);
                reader
                    .read_to_end(&mut out)
                    .map_err(|err| format!("read request payload file failed: {err}"))?;
                Ok(Bytes::from(out))
            }
        }
    }

    pub(crate) fn read_prefix_bytes(&self, max_bytes: usize) -> Result<Bytes, String> {
        match &self.storage {
            RequestPayloadStorage::Memory(bytes) => {
                let take = max_bytes.min(bytes.len());
                Ok(bytes.slice(..take))
            }
            RequestPayloadStorage::File(file) => {
                let mut reader = File::open(&file.path)
                    .map_err(|err| format!("open request payload file failed: {err}"))?;
                let mut out = vec![0_u8; max_bytes.min(file.len)];
                let read = reader
                    .read(&mut out)
                    .map_err(|err| format!("read request payload file failed: {err}"))?;
                out.truncate(read);
                Ok(Bytes::from(out))
            }
        }
    }

    pub(crate) fn read_json_value(&self) -> Result<serde_json::Value, String> {
        match &self.storage {
            RequestPayloadStorage::Memory(bytes) => serde_json::from_slice(bytes.as_ref())
                .map_err(|err| format!("parse request json failed: {err}")),
            RequestPayloadStorage::File(file) => {
                let reader = File::open(&file.path)
                    .map_err(|err| format!("open request payload file failed: {err}"))?;
                serde_json::from_reader(reader)
                    .map_err(|err| format!("parse request json failed: {err}"))
            }
        }
    }

    pub(crate) fn open_blocking_reader(&self) -> Result<Box<dyn Read + Send>, String> {
        match &self.storage {
            RequestPayloadStorage::Memory(bytes) => Ok(Box::new(Cursor::new(bytes.clone()))),
            RequestPayloadStorage::File(file) => File::open(&file.path)
                .map(|reader| Box::new(reader) as Box<dyn Read + Send>)
                .map_err(|err| format!("open request payload file failed: {err}")),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn open_async_reader(&self) -> Result<tokio::fs::File, String> {
        match &self.storage {
            RequestPayloadStorage::Memory(_) => {
                Err("request payload is not file-backed".to_string())
            }
            RequestPayloadStorage::File(file) => Ok(tokio::fs::File::from_std(
                File::open(&file.path)
                    .map_err(|err| format!("open request payload file failed: {err}"))?,
            )),
        }
    }

    pub(crate) fn contains_bytes(&self, needle: &[u8]) -> Result<bool, String> {
        if needle.is_empty() {
            return Ok(true);
        }
        let mut reader = self.open_blocking_reader()?;
        let mut buf = [0_u8; 8192];
        let mut tail = Vec::new();
        loop {
            let read = reader
                .read(&mut buf)
                .map_err(|err| format!("scan request payload failed: {err}"))?;
            if read == 0 {
                return Ok(false);
            }
            if tail.is_empty() {
                if buf[..read]
                    .windows(needle.len())
                    .any(|window| window == needle)
                {
                    return Ok(true);
                }
            } else {
                let mut combined = Vec::with_capacity(tail.len() + read);
                combined.extend_from_slice(&tail);
                combined.extend_from_slice(&buf[..read]);
                if combined
                    .windows(needle.len())
                    .any(|window| window == needle)
                {
                    return Ok(true);
                }
            }
            let keep = needle.len().saturating_sub(1).min(read);
            tail.clear();
            tail.extend_from_slice(&buf[read - keep..read]);
        }
    }
}

pub(crate) struct RequestPayloadBuilder {
    spill_threshold: usize,
    max_body_bytes: usize,
    memory: Vec<u8>,
    file: Option<File>,
    file_path: Option<PathBuf>,
    len: usize,
}

impl RequestPayloadBuilder {
    pub(crate) fn new(
        spill_threshold: usize,
        max_body_bytes: usize,
    ) -> Result<Self, RequestPayloadBuildError> {
        ensure_spool_dir_exists()?;
        Ok(Self {
            spill_threshold: spill_threshold.max(1),
            max_body_bytes,
            memory: Vec::new(),
            file: None,
            file_path: None,
            len: 0,
        })
    }

    pub(crate) fn append_chunk(&mut self, chunk: &[u8]) -> Result<(), RequestPayloadBuildError> {
        if chunk.is_empty() {
            return Ok(());
        }
        let next_len = self.len.saturating_add(chunk.len());
        if next_len > self.max_body_bytes {
            return Err(RequestPayloadBuildError::TooLarge {
                max_body_bytes: self.max_body_bytes,
            });
        }
        if self.file.is_none() && next_len <= self.spill_threshold {
            self.memory.extend_from_slice(chunk);
            self.len = next_len;
            return Ok(());
        }
        if self.file.is_none() {
            self.promote_to_file()?;
        }
        if let Some(file) = self.file.as_mut() {
            file.write_all(chunk)?;
            self.len = next_len;
        }
        Ok(())
    }

    pub(crate) fn serialize_json(
        &mut self,
        value: &serde_json::Value,
    ) -> Result<(), RequestPayloadBuildError> {
        if self.file.is_some() || self.spill_threshold == 0 {
            if self.file.is_none() {
                self.promote_to_file()?;
            }
            if let Some(file) = self.file.as_mut() {
                file.set_len(0)?;
                file.rewind()?;
                serde_json::to_writer(&mut *file, value).map_err(io::Error::other)?;
                file.flush()?;
                let metadata = file.metadata()?;
                self.len = metadata.len() as usize;
                if self.len > self.max_body_bytes {
                    return Err(RequestPayloadBuildError::TooLarge {
                        max_body_bytes: self.max_body_bytes,
                    });
                }
                self.memory.clear();
                return Ok(());
            }
        }
        let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
        if bytes.len() <= self.spill_threshold {
            if bytes.len() > self.max_body_bytes {
                return Err(RequestPayloadBuildError::TooLarge {
                    max_body_bytes: self.max_body_bytes,
                });
            }
            self.memory = bytes;
            self.len = self.memory.len();
            return Ok(());
        }
        self.promote_to_file()?;
        if let Some(file) = self.file.as_mut() {
            file.set_len(0)?;
            file.rewind()?;
            file.write_all(&bytes)?;
            file.flush()?;
        }
        self.memory.clear();
        self.len = bytes.len();
        if self.len > self.max_body_bytes {
            return Err(RequestPayloadBuildError::TooLarge {
                max_body_bytes: self.max_body_bytes,
            });
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<RequestPayload, RequestPayloadBuildError> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
            let path = self
                .file_path
                .take()
                .expect("file path should exist when file exists");
            return Ok(RequestPayload {
                storage: RequestPayloadStorage::File(Arc::new(RequestPayloadFile {
                    path,
                    len: self.len,
                })),
                len: self.len,
            });
        }
        let memory = std::mem::take(&mut self.memory);
        Ok(RequestPayload {
            storage: RequestPayloadStorage::Memory(Bytes::from(memory)),
            len: self.len,
        })
    }

    fn promote_to_file(&mut self) -> Result<(), RequestPayloadBuildError> {
        if self.file.is_some() {
            return Ok(());
        }
        let path = new_spool_file_path();
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)?;
        if !self.memory.is_empty() {
            file.write_all(&self.memory)?;
            self.memory.clear();
        }
        self.file = Some(file);
        self.file_path = Some(path);
        Ok(())
    }
}

impl Drop for RequestPayloadBuilder {
    fn drop(&mut self) {
        if self.file.is_some() {
            let _ = self.file.take();
        }
        if let Some(path) = self.file_path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn ensure_spool_dir_exists() -> io::Result<()> {
    fs::create_dir_all(request_spill_dir())
}

fn request_spill_dir() -> PathBuf {
    std::env::temp_dir().join(DEFAULT_REQUEST_SPILL_DIR_NAME)
}

fn new_spool_file_path() -> PathBuf {
    let mut random = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut random);
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    request_spill_dir().join(format!("payload-{}-{suffix}.bin", std::process::id()))
}

pub(crate) fn cleanup_request_spool_dir() {
    let dir = request_spill_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let _ = fs::remove_file(path);
    }
}

#[allow(dead_code)]
pub(crate) fn payload_path(payload: &RequestPayload) -> Option<&Path> {
    match &payload.storage {
        RequestPayloadStorage::Memory(_) => None,
        RequestPayloadStorage::File(file) => Some(file.path.as_path()),
    }
}

#[cfg(test)]
mod tests {
    use super::{payload_path, RequestPayload};

    struct EnvGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
            crate::gateway::reload_runtime_config_from_env();
        }
    }

    #[test]
    fn spills_large_payload_to_temp_file() {
        let _spill = EnvGuard::set("CODEXMANAGER_REQUEST_SPILL_THRESHOLD_BYTES", "8");
        let _max = EnvGuard::set("CODEXMANAGER_FRONT_PROXY_MAX_BODY_BYTES", "1024");
        crate::gateway::reload_runtime_config_from_env();

        let payload = RequestPayload::from_vec(vec![b'x'; 64]).expect("build request payload");
        let path = payload_path(&payload)
            .map(std::path::Path::to_path_buf)
            .expect("payload should spill to disk");
        assert!(path.is_file());
        drop(payload);
        assert!(!path.exists());
    }
}
