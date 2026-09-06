//! Provider-neutral object storage contract used by the BYOS persistence layer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::{NovaError, Result};

/// Opaque version returned by reads and writes and used for conditional commits.
pub type ObjectVersion = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub bytes: Vec<u8>,
    pub version: ObjectVersion,
}

/// Minimal durable-object operations required by remote manifests and artifacts.
pub trait StorageBackend: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Object>>;
    fn put(&self, key: &str, bytes: &[u8]) -> Result<ObjectVersion>;
    fn put_if_version(
        &self,
        key: &str,
        bytes: &[u8],
        expected: Option<&str>,
    ) -> Result<ObjectVersion>;
    fn list(&self, prefix: &str) -> Result<Vec<String>>;
    fn delete(&self, key: &str) -> Result<()>;
}

fn validate_key(key: &str) -> Result<()> {
    let path = Path::new(key);
    if key.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(NovaError::Parse(format!("invalid object key '{key}'")));
    }
    Ok(())
}

fn version(bytes: &[u8]) -> String {
    format!(
        "{:08x}-{}",
        crate::storage::format::crc32(bytes),
        bytes.len()
    )
}

/// Filesystem implementation for development and backward-compatible deployments.
pub struct LocalStorage {
    root: PathBuf,
}

impl LocalStorage {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn path(&self, key: &str) -> Result<PathBuf> {
        validate_key(key)?;
        Ok(self.root.join(key))
    }
}

impl StorageBackend for LocalStorage {
    fn get(&self, key: &str) -> Result<Option<Object>> {
        let path = self.path(key)?;
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(Object {
                version: version(&bytes),
                bytes,
            })),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn put(&self, key: &str, bytes: &[u8]) -> Result<ObjectVersion> {
        let path = self.path(key)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("novadb.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(tmp, path)?;
        Ok(version(bytes))
    }

    fn put_if_version(
        &self,
        key: &str,
        bytes: &[u8],
        expected: Option<&str>,
    ) -> Result<ObjectVersion> {
        let actual = self.get(key)?.map(|object| object.version);
        if actual.as_deref() != expected {
            return Err(NovaError::Conflict(format!("object '{key}' changed")));
        }
        self.put(key, bytes)
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        validate_key(prefix)?;
        let mut out = Vec::new();
        collect_keys(&self.root, &self.root, &mut out)?;
        out.retain(|key| key.starts_with(prefix));
        out.sort();
        Ok(out)
    }

    fn delete(&self, key: &str) -> Result<()> {
        match std::fs::remove_file(self.path(key)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

fn collect_keys(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_keys(root, &entry.path(), out)?;
        } else {
            out.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

/// Deterministic backend for storage contract and failure-path tests.
#[derive(Clone, Default)]
pub struct MemoryStorage(Arc<RwLock<HashMap<String, Vec<u8>>>>);

impl StorageBackend for MemoryStorage {
    fn get(&self, key: &str) -> Result<Option<Object>> {
        validate_key(key)?;
        Ok(self
            .0
            .read()
            .unwrap()
            .get(key)
            .cloned()
            .map(|bytes| Object {
                version: version(&bytes),
                bytes,
            }))
    }
    fn put(&self, key: &str, bytes: &[u8]) -> Result<ObjectVersion> {
        validate_key(key)?;
        self.0.write().unwrap().insert(key.into(), bytes.to_vec());
        Ok(version(bytes))
    }
    fn put_if_version(
        &self,
        key: &str,
        bytes: &[u8],
        expected: Option<&str>,
    ) -> Result<ObjectVersion> {
        let mut objects = self.0.write().unwrap();
        let actual = objects.get(key).map(|value| version(value));
        if actual.as_deref() != expected {
            return Err(NovaError::Conflict(format!("object '{key}' changed")));
        }
        objects.insert(key.into(), bytes.to_vec());
        Ok(version(bytes))
    }
    fn list(&self, prefix: &str) -> Result<Vec<String>> {
        validate_key(prefix)?;
        let mut keys: Vec<_> = self
            .0
            .read()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        keys.sort();
        Ok(keys)
    }
    fn delete(&self, key: &str) -> Result<()> {
        validate_key(key)?;
        self.0.write().unwrap().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract(backend: &dyn StorageBackend) {
        assert!(backend.get("db/manifest.json").unwrap().is_none());
        let v1 = backend
            .put_if_version("db/manifest.json", b"one", None)
            .unwrap();
        assert_eq!(
            backend.get("db/manifest.json").unwrap().unwrap().bytes,
            b"one"
        );
        assert!(backend
            .put_if_version("db/manifest.json", b"bad", None)
            .is_err());
        let v2 = backend
            .put_if_version("db/manifest.json", b"two", Some(&v1))
            .unwrap();
        assert_ne!(v1, v2);
        backend.put("db/wal/1.log", b"wal").unwrap();
        assert_eq!(backend.list("db/").unwrap().len(), 2);
        backend.delete("db/wal/1.log").unwrap();
        assert_eq!(backend.list("db/").unwrap(), vec!["db/manifest.json"]);
    }

    #[test]
    fn memory_contract() {
        contract(&MemoryStorage::default());
    }
    #[test]
    fn local_contract() {
        let dir = tempfile::tempdir().unwrap();
        contract(&LocalStorage::new(dir.path()).unwrap());
    }
    #[test]
    fn traversal_is_rejected() {
        assert!(MemoryStorage::default().put("../secret", b"x").is_err());
    }
}
