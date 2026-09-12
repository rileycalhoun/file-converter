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
    size: u64,
    committed: bool,
}

impl PublishedOutput {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
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
    let mut temporary = output_temporary(preferred)?;
    let size = io::copy(&mut contents, temporary.as_file_mut())?;
    publish_temporary(preferred, temporary, size)
}

/// File-backed sources use the platform's optimized file copy/clone support,
/// while an exclusively created staging directory keeps publication safe.
/// A filesystem clone remains independent when either file is later edited.
pub(crate) fn publish_source_output(
    preferred: &Path,
    source: &Path,
) -> io::Result<PublishedOutput> {
    let parent = preferred.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "output has no parent folder")
    })?;
    let staging_directory = tempfile::Builder::new()
        .prefix(".file-converter-")
        .tempdir_in(parent)?;
    let staged = staging_directory.path().join("output.pdf");
    // Keep this private child name initially absent: APFS can clone into a new
    // file, but falls back to a full copy when the target already exists.
    let size = std::fs::copy(source, &staged)?;
    let temporary = tempfile::NamedTempFile::from_parts(
        std::fs::File::open(&staged)?,
        tempfile::TempPath::try_from_path(staged)?,
    );
    publish_temporary(preferred, temporary, size)
}

/// Exercise actual destination access before expensive conversion. On macOS,
/// this lets the OS request folder consent if it has not been decided yet.
/// Permission bits alone cannot detect privacy restrictions or directory ACLs.
pub(crate) fn check_output_access(preferred: &Path) -> io::Result<()> {
    output_temporary(preferred)?.close()
}

fn output_temporary(preferred: &Path) -> io::Result<tempfile::NamedTempFile> {
    let parent = preferred.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "output has no parent folder")
    })?;
    tempfile::Builder::new()
        .prefix(".file-converter-")
        .suffix(".tmp")
        .tempfile_in(parent)
}

fn publish_temporary(
    preferred: &Path,
    mut temporary: tempfile::NamedTempFile,
    size: u64,
) -> io::Result<PublishedOutput> {
    let parent = preferred.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "output has no parent folder")
    })?;
    let stem = preferred
        .file_stem()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "output has no file name"))?;
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
                    size,
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
    let size = contents.metadata()?.len();
    match link(staged, preferred) {
        Ok(()) => Ok(PublishedOutput {
            path: preferred.to_path_buf(),
            identity,
            size,
            committed: false,
        }),
        // The shared publisher both retries occupied names and provides the
        // copy fallback for EXDEV, unsupported hard links, and other
        // filesystem limitations. No destination gets overwritten.
        Err(_) => publish_source_output(preferred, staged),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn access_check_leaves_existing_output_and_no_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let preferred = directory.path().join("report.pdf");
        std::fs::write(&preferred, b"existing").unwrap();
        check_output_access(&preferred).unwrap();
        assert_eq!(std::fs::read(&preferred).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(check_output_access(&directory.path().join("missing/report.pdf")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn access_check_reports_unwritable_folder() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = check_output_access(&directory.path().join("report.pdf"));
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn unavailable_destination_preserves_the_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.pdf");
        std::fs::write(&source, b"%PDF-original").unwrap();
        let preferred = directory.path().join("disconnected-volume/output.pdf");
        assert!(publish_source_output(&preferred, &source).is_err());
        assert_eq!(std::fs::read(source).unwrap(), b"%PDF-original");
        assert!(!preferred.exists());
    }

    #[test]
    fn optimized_file_copy_preserves_collisions_and_owns_only_its_temporary() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.pdf");
        let preferred = directory.path().join("output.pdf");
        std::fs::write(&source, b"%PDF-source").unwrap();
        std::fs::write(&preferred, b"existing").unwrap();
        let published = publish_source_output(&preferred, &source).unwrap();
        assert_eq!(published.size(), 11);
        assert_eq!(std::fs::read(published.path()).unwrap(), b"%PDF-source");
        drop(published);
        assert_eq!(std::fs::read(&preferred).unwrap(), b"existing");
        assert_eq!(std::fs::read(&source).unwrap(), b"%PDF-source");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);

        assert!(publish_source_output(&preferred, &directory.path().join("missing.pdf")).is_err());
        assert_eq!(std::fs::read(&preferred).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn streaming_publication_reads_each_input_byte_once() {
        struct CountedReader<'a> {
            remaining: &'a [u8],
            transferred: &'a mut usize,
        }
        impl Read for CountedReader<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let count = self.remaining.read(buffer)?;
                *self.transferred += count;
                Ok(count)
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let bytes = vec![42; 1024 * 1024 + 17];
        let mut transferred = 0;
        let output = publish_output(
            &directory.path().join("output.pdf"),
            CountedReader {
                remaining: &bytes,
                transferred: &mut transferred,
            },
        )
        .unwrap();
        assert_eq!(transferred, bytes.len());
        assert_eq!(output.size(), bytes.len() as u64);
        assert_eq!(std::fs::read(output.path()).unwrap(), bytes);
    }

    #[test]
    #[ignore = "manual I/O benchmark; run with --ignored --nocapture"]
    fn benchmark_pdf_publication() {
        use std::{io::Write, time::Instant};

        let cache = tempfile::tempdir().unwrap();
        let destination = match std::env::var_os("FILE_CONVERTER_BENCH_DESTINATION") {
            Some(path) => tempfile::tempdir_in(path).unwrap(),
            None => tempfile::tempdir().unwrap(),
        };
        let source = cache.path().join("synthetic.pdf");
        // A deterministic 32 MiB payload exercises file transport, not PDF
        // rendering. Fixture generation and verification are outside timings.
        let mut chunk = vec![0; 1024 * 1024];
        let mut seed = 0x12345678u32;
        for byte in &mut chunk {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *byte = seed as u8;
        }
        chunk[..5].copy_from_slice(b"%PDF-");
        let mut fixture = std::fs::File::create(&source).unwrap();
        for _ in 0..32 {
            fixture.write_all(&chunk).unwrap();
        }
        fixture.sync_all().unwrap();
        let size = fixture.metadata().unwrap().len();
        drop(fixture);
        let mut baseline_ms = Vec::new();
        let mut direct_ms = Vec::new();
        let mut staged_ms = Vec::new();
        for trial in 0..3 {
            let cached = cache.path().join(format!("cached-{trial}.pdf"));
            let old_output = destination.path().join(format!("old-{trial}.pdf"));
            let start = Instant::now();
            let first_copy = std::fs::copy(&source, &cached).unwrap();
            let second_copy = std::fs::copy(&cached, &old_output).unwrap();
            std::fs::File::open(&old_output)
                .unwrap()
                .sync_all()
                .unwrap();
            baseline_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(first_copy + second_copy, 2 * size);

            let start = Instant::now();
            let direct = publish_source_output(
                &destination.path().join(format!("direct-{trial}.pdf")),
                &source,
            )
            .unwrap()
            .commit();
            direct_ms.push(start.elapsed().as_secs_f64() * 1000.0);

            let start = Instant::now();
            let staged = publish_staged_output(
                &destination.path().join(format!("staged-{trial}.pdf")),
                &cached,
            )
            .unwrap()
            .commit();
            staged_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(
                std::fs::read(&direct).unwrap(),
                std::fs::read(&source).unwrap()
            );
            assert_eq!(
                std::fs::read(&staged).unwrap(),
                std::fs::read(&source).unwrap()
            );
            if trial == 0 {
                println!("fixture_bytes={size}; baseline_logical_copy_bytes={}; passthrough_logical_copy_bytes={size}; staged_publication_logical_copy_bytes={}",
                    2 * size, if same_file::is_same_file(&cached, &staged).unwrap() { 0 } else { size });
            }
        }
        for samples in [&mut baseline_ms, &mut direct_ms, &mut staged_ms] {
            samples.sort_by(f64::total_cmp);
        }
        println!("median_ms (3 warm-cache trials; final output synchronized): baseline_two_copies={:.3}, passthrough_one_copy={:.3}, staged_publication={:.3}",
            baseline_ms[1], direct_ms[1], staged_ms[1]);
        println!("Logical bytes describe requested file copies, not physical storage traffic or filesystem clone behavior.");
    }

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
