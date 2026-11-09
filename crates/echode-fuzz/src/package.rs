//! A whole package as the product opens it: `recover` reads `project.json`,
//! the track and content files and the plugin states they name, and replays
//! `journal.jsonl`.
//!
//! Input: an archive of files. A line `--- NAME` starts a file, and the file
//! runs to the newline before the next such line. Names are `project.json`,
//! `journal.jsonl`, `save.pending`, or one file in `tracks/`, `contents/` or
//! `media/objects/`; other names are skipped.

use std::path::Path;

use echode_history::{read_package, recover, write_package};
use echode_model::Project;

use crate::sample;
use crate::scratch::Scratch;

const HEADER: &[u8] = b"--- ";

pub(crate) fn run(data: &[u8]) {
    let scratch = Scratch::new("package");
    let package = scratch.path();
    unpack(package, data);
    if let Ok(project) = recover(package) {
        assert_saves(package, &project);
    }
}

/// Writes an archive's allowed files into `package`. A name the file system
/// refuses, such as one longer than 255 bytes, is skipped like a missing file.
pub(crate) fn unpack(package: &Path, data: &[u8]) {
    std::fs::create_dir_all(package).expect("create the package");
    for (name, bytes) in entries(data) {
        let Some(path) = allowed(package, name) else {
            continue;
        };
        let parent = path.parent().expect("a parent");
        if std::fs::create_dir_all(parent).is_ok() {
            let _ = std::fs::write(&path, bytes);
        }
    }
}

/// An opened project keeps the model's invariants, saves, and reads back as
/// the same project. Otherwise the next save would lose or reject it.
pub(crate) fn assert_saves(package: &Path, project: &Project) {
    if let Err(error) = project.check_invariants() {
        panic!("an opened project breaks the model invariants: {error}");
    }
    if let Err(error) = write_package(package, project) {
        panic!("an opened project does not save: {error}");
    }
    match read_package(package) {
        Ok(again) => assert!(again == *project, "a saved project reads back differently"),
        Err(error) => panic!("a saved project does not read back: {error}"),
    }
}

fn entries(data: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut starts = Vec::new();
    let mut line = 0;
    while line < data.len() {
        let end = data[line..]
            .iter()
            .position(|&byte| byte == b'\n')
            .map_or(data.len(), |offset| line + offset);
        if data[line..end].starts_with(HEADER) {
            starts.push((line, end));
        }
        line = end + 1;
    }
    let mut files = Vec::with_capacity(starts.len());
    for (index, &(header, name_end)) in starts.iter().enumerate() {
        let name = &data[header + HEADER.len()..name_end];
        let body_start = (name_end + 1).min(data.len());
        let body_end = starts.get(index + 1).map_or(data.len(), |&(next, _)| {
            next.saturating_sub(1).max(body_start)
        });
        files.push((name, &data[body_start..body_end]));
    }
    files
}

fn allowed(package: &Path, name: &[u8]) -> Option<std::path::PathBuf> {
    let name = std::str::from_utf8(name).ok()?;
    if matches!(name, "project.json" | "journal.jsonl" | "save.pending") {
        return Some(package.join(name));
    }
    let (folder, file) = name.rsplit_once('/')?;
    if !matches!(folder, "tracks" | "contents" | "media/objects")
        || file.is_empty()
        || file == "."
        || file == ".."
        || file.contains('\0')
    {
        return None;
    }
    Some(package.join(folder).join(file))
}

/// The archive of a package's files, plus `journal` as `journal.jsonl`.
pub(crate) fn archive(package: &Path, journal: Option<&str>) -> Vec<u8> {
    let mut bytes = Vec::new();
    push(
        &mut bytes,
        "project.json",
        &read(&package.join("project.json")),
    );
    for folder in ["tracks", "contents", "media/objects"] {
        let Ok(listing) = std::fs::read_dir(package.join(folder)) else {
            continue;
        };
        let mut names: Vec<_> = listing
            .map(|entry| entry.expect("a folder entry").file_name())
            .filter(|name| !name.to_string_lossy().starts_with('.'))
            .collect();
        names.sort();
        for name in names {
            let name = name.to_string_lossy();
            let path = package.join(folder).join(name.as_ref());
            push(&mut bytes, &format!("{folder}/{name}"), &read(&path));
        }
    }
    if let Some(journal) = journal {
        push(&mut bytes, "journal.jsonl", journal.as_bytes());
    }
    bytes
}

fn push(archive: &mut Vec<u8>, name: &str, body: &[u8]) {
    if !archive.is_empty() {
        archive.push(b'\n');
    }
    archive.extend_from_slice(HEADER);
    archive.extend_from_slice(name.as_bytes());
    archive.push(b'\n');
    archive.extend_from_slice(body);
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

pub(crate) fn seeds() -> Vec<Vec<u8>> {
    let sample = sample::sample();
    let mut with_journal = sample.archive.clone();
    push(
        &mut with_journal,
        "journal.jsonl",
        sample.journal.as_bytes(),
    );
    let empty = Scratch::new("package-empty");
    write_package(empty.path(), &Project::new()).expect("write an empty package");
    vec![
        sample.archive.clone(),
        with_journal,
        archive(empty.path(), None),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_archive_lists_each_file_and_its_bytes() {
        let mut bytes = Vec::new();
        push(&mut bytes, "project.json", b"{}\n");
        push(&mut bytes, "tracks/a.json", b"");
        push(&mut bytes, "journal.jsonl", b"x\n--- y");
        let files = entries(&bytes);
        let files: Vec<_> = files
            .iter()
            .map(|(name, body)| (std::str::from_utf8(name).unwrap(), *body))
            .collect();
        assert_eq!(
            files,
            [
                ("project.json", &b"{}\n"[..]),
                ("tracks/a.json", &b""[..]),
                ("journal.jsonl", &b"x"[..]),
                ("y", &b""[..]),
            ]
        );
    }

    #[test]
    fn names_stay_inside_the_package() {
        let package = Path::new("/p");
        assert!(allowed(package, b"../project.json").is_none());
        assert!(allowed(package, b"tracks/..").is_none());
        assert!(allowed(package, b"tracks/a/b.json").is_none());
        assert!(allowed(package, b"/etc/passwd").is_none());
        assert_eq!(
            allowed(package, b"media/objects/ab"),
            Some(Path::new("/p/media/objects/ab").to_path_buf())
        );
    }
}
