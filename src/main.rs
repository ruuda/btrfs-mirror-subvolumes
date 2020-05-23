extern crate walkdir;

use std::cmp;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::env;
use std::fs;
use std::io;
use std::io::BufRead;
use std::mem;
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

struct DirScan {
    dir_path: PathBuf,
    entries: HashMap<FileInfo, Vec<PathBuf>>,
}

impl DirScan {
    fn prefix_path<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        let mut result = self.dir_path.clone();
        result.push(path);
        result
    }

    /// Move the entries out, replacing the inner hashmap with an empty one.
    fn take_entries(&mut self) -> HashMap<FileInfo, Vec<PathBuf>> {
        let mut result = HashMap::new();
        mem::swap(&mut result, &mut self.entries);
        result
    }
}

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

    let result = DirScan {
        dir_path: dir_path.as_ref().to_path_buf(),
        entries: entries,
    };

    Ok(result)
}

/// Compare the contents of two files which are assumed to have the same size.
fn are_file_contents_identical<P: AsRef<Path>>(p1: P, p2: P) -> io::Result<bool> {
    println!("Comparing {:?} vs {:?}", p1.as_ref(), p2.as_ref());
    let f1 = fs::File::open(p1)?;
    let f2 = fs::File::open(p2)?;
    let mut r1 = io::BufReader::new(f1);
    let mut r2 = io::BufReader::new(f2);

    loop {
        let b1 = r1.fill_buf()?;
        let b2 = r2.fill_buf()?;

        // Empty buffer indicates EOF. If we got to the end, and both buffers
        // are empty at the same time, then all bytes were equal. If one is
        // empty, then the files were of different sizes, but we should have
        // checked for that in advance, because there'd be no point comparing
        // contents then.
        match (b1.len(), b2.len()) {
            (0, 0) => return Ok(true),
            (0, _) => panic!("Comparing contents, but file sizes were different."),
            (_, 0) => panic!("Comparing contents, but file sizes were different."),
            (n1, n2) => {
                let len = cmp::min(n1, n2);
                if &b1[..len] == &b2[..len] {
                    r1.consume(len);
                    r2.consume(len);
                } else {
                    return Ok(false);
                }
            }
        }
    }
}

fn diff(base: &DirScan, mut target: DirScan) -> io::Result<Diff> {
    let mut copies = Vec::new();
    let mut deletes = Vec::new();
    let mut adds = Vec::new();

    for (ref info, ref paths) in base.entries.iter() {
        for path in paths.iter() {
            match target.entries.get(info) {
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

    for (info, mut paths) in target.take_entries().drain() {
        for path in paths.drain(..) {
            match base.entries.get(&info) {
                None => adds.push(path),
                Some(ref base_paths) => {
                    if base_paths.contains(&path) {
                        // Already there with the right size and mtime, we
                        // assume that the file has not changed.
                        // TODO: Optionally confirm contents.
                    } else {
                        // Until we confirm that we can copy, we'll assume that
                        // this is a new file.
                        adds.push(path);

                        for candidate in base_paths.iter() {
                            let path = adds.pop().expect("We just pushed so 'adds' is nonempty.");
                            if are_file_contents_identical(
                                base.prefix_path(candidate),
                                target.prefix_path(&path),
                            )? {
                                let copy = CopyFile {
                                    src: candidate.clone(),
                                    dst: path,
                                };
                                copies.push(copy);
                                break
                            } else {
                                adds.push(path);
                            }
                        }
                    }
                }
            }
        }
    }

    // Ensure the diff is deterministic, independent of hash map order.
    deletes.sort();
    adds.sort();
    copies.sort();

    let result = Diff {
        deletes: deletes,
        copies: copies,
        adds: adds,
    };
    Ok(result)
}

fn main() -> io::Result<()> {
    let dir_base = env::args().nth(1).unwrap();
    let dir_target = env::args().nth(2).unwrap();

    let entries_base = scan_dir(&dir_base)?;
    let entries_target = scan_dir(&dir_target)?;

    let diff = diff(&entries_base, entries_target)?;

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
