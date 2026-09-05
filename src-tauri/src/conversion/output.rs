use std::{
    ffi::OsString,
    io::{self, Read},
    path::{Path, PathBuf},
};

use same_file::Handle;

/// A newly published output, owned until its history update succeeds.
/// Dropping it rolls back only this publication, never a collision candidate.
pub(crate) struct PublishedOutput {
    path: PathBuf,
    identity: Handle,
    committed: bool,
}

impl PublishedOutput {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn commit(mut self) -> PathBuf {
        self.committed = true;
        self.path.clone()
    }
}

impl Drop for PublishedOutput {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // Be conservative if the published path was removed or replaced before
        // rollback. Keep the original handle open so its identity cannot be reused.
        let is_regular_file = std::fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.file_type().is_file());
        if is_regular_file
            && Handle::from_path(&self.path).is_ok_and(|current| current == self.identity)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Stage on the destination filesystem, then publish with an atomic no-replace
/// operation. A PDF becomes visible only after its entire contents are written.
/// The preferred name is merely a candidate; only publication claims a name.
pub(crate) fn publish_output(
    preferred: &Path,
    mut contents: impl Read,
) -> io::Result<PublishedOutput> {
    let parent = preferred.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "output has no parent folder")
    })?;
    let stem = preferred
        .file_stem()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "output has no file name"))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".file-converter-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    io::copy(&mut contents, temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    let identity = Handle::from_file(temporary.as_file().try_clone()?)?;

    for attempt in 0..1000 {
        let candidate = if attempt == 0 {
            preferred.to_path_buf()
        } else {
            let mut name = OsString::from(stem);
            name.push(format!("-{attempt}"));
            if let Some(extension) = preferred.extension() {
                name.push(".");
                name.push(extension);
            }
            parent.join(name)
        };
        match temporary.persist_noclobber(&candidate) {
            Ok(_) => {
                return Ok(PublishedOutput {
                    path: candidate,
                    identity,
                    committed: false,
                });
            }
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                temporary = error.file;
            }
            Err(error) => return Err(error.error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Could not allocate an unused output name after 1000 attempts",
    ))
}

/// Native engines already own a complete cache file. A no-clobber hard link
/// publishes that file without a second full copy when the filesystem supports
/// it. Cross-volume and link-unsupported destinations use local staging instead.
pub(crate) fn publish_staged_output(
    preferred: &Path,
    staged: &Path,
) -> io::Result<PublishedOutput> {
    publish_staged_with_link(preferred, staged, |source, destination| {
        std::fs::hard_link(source, destination)
    })
}

fn publish_staged_with_link(
    preferred: &Path,
    staged: &Path,
    link: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<PublishedOutput> {
    let contents = std::fs::File::open(staged)?;
    // Never publish a cache symlink itself. The copy path safely writes the
    // opened contents into a fresh destination file.
    if !std::fs::symlink_metadata(staged)?.file_type().is_file() {
        return publish_output(preferred, contents);
    }
    contents.sync_all()?;
    let identity = Handle::from_file(contents.try_clone()?)?;
    match link(staged, preferred) {
        Ok(()) => Ok(PublishedOutput {
            path: preferred.to_path_buf(),
            identity,
            committed: false,
        }),
        // The shared publisher both retries occupied names and provides the
        // streaming fallback for EXDEV, unsupported hard links, and other
        // filesystem limitations. No destination gets overwritten.
        Err(_) => publish_output(preferred, contents),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn native_staging_publishes_without_copying_on_the_same_filesystem() {
        let directory = tempfile::tempdir().unwrap();
        let staged = directory.path().join("staged.pdf");
        let preferred = directory.path().join("report.pdf");
        std::fs::write(&staged, b"%PDF-native").unwrap();
        let published = publish_staged_output(&preferred, &staged).unwrap();
        assert!(same_file::is_same_file(&staged, published.path()).unwrap());
        let actual = published.commit();
        std::fs::remove_file(staged).unwrap();
        assert_eq!(std::fs::read(actual).unwrap(), b"%PDF-native");
    }

    #[test]
    fn cross_volume_publication_falls_back_to_destination_local_staging() {
        let cache = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let staged = cache.path().join("output.pdf");
        let preferred = destination.path().join("report.pdf");
        std::fs::write(&staged, b"%PDF-cross-volume").unwrap();
        let published = publish_staged_with_link(&preferred, &staged, |_, _| {
            Err(io::Error::from(io::ErrorKind::CrossesDevices))
        })
        .unwrap();
        assert!(!same_file::is_same_file(&staged, published.path()).unwrap());
        let actual = published.commit();
        assert_eq!(std::fs::read(actual).unwrap(), b"%PDF-cross-volume");
        assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 1);
    }

    #[test]
    fn native_staging_collision_falls_back_without_overwriting() {
        let directory = tempfile::tempdir().unwrap();
        let staged = directory.path().join("staged.pdf");
        let preferred = directory.path().join("report.pdf");
        std::fs::write(&staged, b"%PDF-native").unwrap();
        std::fs::write(&preferred, b"existing").unwrap();
        let published = publish_staged_output(&preferred, &staged).unwrap();
        assert_ne!(published.path(), preferred);
        drop(published);
        assert_eq!(std::fs::read(preferred).unwrap(), b"existing");
        assert_eq!(std::fs::read(staged).unwrap(), b"%PDF-native");
    }

    #[test]
    fn collisions_preserve_files_and_directories() {
        let directory = tempfile::tempdir().unwrap();
        let preferred = directory.path().join("report.pdf");
        std::fs::write(&preferred, b"existing").unwrap();
        std::fs::create_dir(directory.path().join("report-1.pdf")).unwrap();
        let published = publish_output(&preferred, &b"new PDF"[..]).unwrap();
        assert_eq!(published.path(), directory.path().join("report-2.pdf"));
        assert_eq!(std::fs::read(&preferred).unwrap(), b"existing");
        let new_path = published.commit();
        assert_eq!(std::fs::read(new_path).unwrap(), b"new PDF");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_collisions_are_not_followed() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.pdf");
        let preferred = directory.path().join("report.pdf");
        std::fs::write(&target, b"original").unwrap();
        std::os::unix::fs::symlink(&target, &preferred).unwrap();
        std::os::unix::fs::symlink(
            directory.path().join("missing"),
            directory.path().join("report-1.pdf"),
        )
        .unwrap();
        let published = publish_output(&preferred, &b"converted"[..]).unwrap();
        assert_eq!(published.path(), directory.path().join("report-2.pdf"));
        drop(published);
        assert_eq!(std::fs::read(target).unwrap(), b"original");
        assert!(std::fs::symlink_metadata(preferred)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(std::fs::symlink_metadata(directory.path().join("report-1.pdf")).is_ok());
    }

    #[test]
    fn concurrent_publications_claim_distinct_complete_files() {
        let directory = tempfile::tempdir().unwrap();
        let preferred = directory.path().join("report.pdf");
        let barrier = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let preferred = preferred.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let bytes = vec![index; 8192];
                    barrier.wait();
                    let path = publish_output(&preferred, bytes.as_slice())
                        .unwrap()
                        .commit();
                    (path, bytes)
                })
            })
            .collect();
        let mut paths = std::collections::HashSet::new();
        for worker in workers {
            let (path, bytes) = worker.join().unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert!(paths.insert(path));
        }
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 8);
    }

    #[test]
    fn failed_copy_leaves_no_partial_output_or_temp_file() {
        struct BrokenReader(bool);
        impl Read for BrokenReader {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if self.0 {
                    return Err(io::Error::other("injected read failure"));
                }
                self.0 = true;
                buffer[..4].copy_from_slice(b"%PDF");
                Ok(4)
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let preferred = directory.path().join("report.pdf");
        std::fs::write(&preferred, b"existing").unwrap();
        assert!(publish_output(&preferred, BrokenReader(false)).is_err());
        assert_eq!(std::fs::read(&preferred).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn rollback_removes_only_the_owned_output() {
        let directory = tempfile::tempdir().unwrap();
        let preferred = directory.path().join("report.pdf");
        std::fs::write(&preferred, b"existing").unwrap();
        let published = publish_output(&preferred, &b"new"[..]).unwrap();
        let published_path = published.path().to_path_buf();
        drop(published);
        assert!(!published_path.exists());
        assert_eq!(std::fs::read(&preferred).unwrap(), b"existing");

        let published = publish_output(&preferred, &b"new"[..]).unwrap();
        let replaced_path = published.path().to_path_buf();
        std::fs::remove_file(&replaced_path).unwrap();
        std::fs::write(&replaced_path, b"replacement").unwrap();
        drop(published);
        assert_eq!(std::fs::read(replaced_path).unwrap(), b"replacement");
    }
}
