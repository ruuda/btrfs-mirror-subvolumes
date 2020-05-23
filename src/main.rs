extern crate walkdir;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Eq, Ord, Debug, Hash, PartialEq, PartialOrd)]
struct FileInfo {
    len: u64,
    mtime: SystemTime,
}

#[derive(Eq, Ord, Debug, PartialEq, PartialOrd)]
struct CopyFile {
    src: PathBuf,
    dst: PathBuf,
}

#[derive(Eq, Debug, PartialEq)]
struct Diff {
    copies: Vec<CopyFile>,
    deletes: Vec<PathBuf>,
    adds: Vec<PathBuf>,
}

type DirScan = HashMap<FileInfo, Vec<PathBuf>>;

fn scan_dir<P: AsRef<Path>>(dir_path: P) -> io::Result<DirScan> {
    let mut entries: HashMap<FileInfo, Vec<PathBuf>> = HashMap::new();
    let wd = walkdir::WalkDir::new(&dir_path)
        .max_open(128)
        .same_file_system(true);

    for entry_opt in wd {
        let entry = entry_opt?;
        let meta = entry.metadata()?;

        if !meta.is_file() { continue }

        let len = meta.len();
        let mtime = meta.modified()?;
        let file_info = FileInfo { len, mtime };
        let full_path = entry.into_path();
        let rel_path = match full_path.strip_prefix(&dir_path) {
            Ok(p) => p.to_path_buf(),
            Err(e) => panic!("Dir entry is not inside root? {:?}", e),
        };

        match entries.entry(file_info) {
            Entry::Occupied(mut e) => { e.get_mut().push(rel_path); }
            Entry::Vacant(e) => { e.insert(vec![rel_path]); }
        };
    }

    Ok(entries)
}

fn diff(base: &DirScan, mut target: DirScan) -> Diff {
    let mut copies = Vec::new();
    let mut deletes = Vec::new();
    let mut adds = Vec::new();

    for (ref info, ref paths) in base.iter() {
        for path in paths.iter() {
            match target.get(info) {
                None => deletes.push(path.clone()),
                Some(ref target_paths) => {
                    if target_paths.contains(&path) {
                        // Still there, nothing to delete.
                    } else {
                        deletes.push(path.clone());
                    }
                }
            }
        }
    }

    for (info, mut paths) in target.drain() {
        for path in paths.drain(..) {
            match base.get(&info) {
                None => adds.push(path),
                Some(ref base_paths) => {
                    if base_paths.contains(&path) {
                        // Already there with the right size and mtime, we
                        // assume that the file has not changed.
                        // TODO: Optionally confirm contents.
                    } else {
                        // TODO: Check the contents of all, find a good base.
                        // For now we just assume the first one will do.
                        let copy = CopyFile {
                            src: base_paths[0].clone(),
                            dst: path
                        };
                        copies.push(copy);
                    }
                }
            }
        }
    }

    // Ensure the diff is deterministic, independent of hash map order.
    deletes.sort();
    adds.sort();
    copies.sort();

    Diff {
        deletes: deletes,
        copies: copies,
        adds: adds,
    }
}

fn main() -> io::Result<()> {
    let dir_base = env::args().nth(1).unwrap();
    let dir_target = env::args().nth(2).unwrap();

    let entries_base = scan_dir(&dir_base)?;
    let entries_target = scan_dir(&dir_target)?;

    let diff = diff(&entries_base, entries_target);

    for copy in diff.copies.iter() {
        println!("C {:?} -> {:?}", copy.src, copy.dst);
    }
    for del in diff.deletes.iter() {
        println!("D {:?}", del);
    }
    for add in diff.adds.iter() {
        println!("A {:?}", add);
    }

    Ok(())
}
