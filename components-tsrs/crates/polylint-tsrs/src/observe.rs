//! What a program reads, told to the host as it reads it: the ground a later replay of the program stands on.

use std::sync::Arc;
use std::time::SystemTime;

use tsrs_vfs::{Entries, FS, FileInfo};

/// Told of everything the programs of a [`ProgramHost`](crate::ProgramHost) ask of the world outside them: the
/// answers of the file system, the source files and the configs. A host records them to know later whether a program
/// would be built from the same inputs again. Every method does nothing by default, and they are called from the
/// threads the program is built and checked on, in no fixed order: record into sorted or keyed data.
///
/// The file system answers are those the program's own lookups got, cached ones included; names are as tsrs names
/// files. A source file another program of the run parsed first is not read again, so it is told through
/// [`Observer::source_file`] alone, and a config's `include` globs are expanded when the config is parsed, once for
/// the run: a replay checks a config ([`Observer::config`]) by parsing it again.
pub trait Observer: Send + Sync {
    /// `path` was read: its text as tsrs got it (a byte order mark dropped), or `None` when it cannot be read.
    fn file_read(&self, path: &str, text: Option<&str>) {
        let _ = (path, text);
    }

    /// Whether a file is at `path` was asked.
    fn file_exists(&self, path: &str, exists: bool) {
        let _ = (path, exists);
    }

    /// Whether a directory is at `path` was asked.
    fn directory_exists(&self, path: &str, exists: bool) {
        let _ = (path, exists);
    }

    /// `path` was looked up: what is there, or `None` for nothing.
    fn stat(&self, path: &str, info: Option<&FileInfo>) {
        let _ = (path, info);
    }

    /// The real path of `path` was asked and is `real`.
    fn realpath(&self, path: &str, real: &str) {
        let _ = (path, real);
    }

    /// The directory `path` was listed.
    fn directory_listed(&self, path: &str, entries: &Entries) {
        let _ = (path, entries);
    }

    /// The program asked its compiler host for the source file `file_name`: the text it was parsed from, by this
    /// program or by another of the run, or `None` when it cannot be read.
    fn source_file(&self, file_name: &str, text: Option<&str>) {
        let _ = (file_name, text);
    }

    /// The config `file_name` was read, for the program built from it or as a project reference: the configs it
    /// extends, directly or through another, or `None` when it cannot be read.
    fn config(&self, file_name: &str, extended: Option<&[String]>) {
        let _ = (file_name, extended);
    }
}

/// `inner`, telling `observer` every answer it gives.
pub(crate) struct ObservedFs {
    inner: Arc<dyn FS>,
    observer: Arc<dyn Observer>,
}

impl ObservedFs {
    pub(crate) fn new(inner: Arc<dyn FS>, observer: Arc<dyn Observer>) -> Self {
        Self { inner, observer }
    }
}

impl FS for ObservedFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.inner.use_case_sensitive_file_names()
    }

    fn file_exists(&self, path: &str) -> bool {
        let exists = self.inner.file_exists(path);
        self.observer.file_exists(path, exists);
        exists
    }

    fn read_file(&self, path: &str) -> Option<String> {
        let text = self.inner.read_file(path);
        self.observer.file_read(path, text.as_deref());
        text
    }

    fn write_file(&self, path: &str, data: &str) -> Result<(), String> {
        self.inner.write_file(path, data)
    }

    fn append_file(&self, path: &str, data: &str) -> Result<(), String> {
        self.inner.append_file(path, data)
    }

    fn remove(&self, path: &str) -> Result<(), String> {
        self.inner.remove(path)
    }

    fn chtimes(&self, path: &str, a_time: SystemTime, m_time: SystemTime) -> Result<(), String> {
        self.inner.chtimes(path, a_time, m_time)
    }

    fn directory_exists(&self, path: &str) -> bool {
        let exists = self.inner.directory_exists(path);
        self.observer.directory_exists(path, exists);
        exists
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        let entries = self.inner.get_accessible_entries(path);
        self.observer.directory_listed(path, &entries);
        entries
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        let info = self.inner.stat(path);
        self.observer.stat(path, info.as_ref());
        info
    }

    fn realpath(&self, path: &str) -> String {
        let real = self.inner.realpath(path);
        self.observer.realpath(path, &real);
        real
    }
}
