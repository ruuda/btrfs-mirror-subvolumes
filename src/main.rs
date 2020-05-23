extern crate libc;
extern crate walkdir;

use std::cmp;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::env;
use std::fs;
use std::io::BufRead;
use std::io;
use std::mem;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::process;
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

/// Detect potentially moved files, and emit a copy operation for each.
fn diff(base: &DirScan, mut target: DirScan) -> io::Result<Vec<CopyFile>> {
    let mut copies = Vec::new();

    for (info, mut paths) in target.take_entries().drain() {
        for path in paths.drain(..) {
            match base.entries.get(&info) {
                None => continue,
                Some(ref base_paths) => {
                    if base_paths.contains(&path) {
                        // Already there with the same size and mtime, we
                        // assume that the file has not changed.
                    } else {
                        // We assume that if there was a file with the same
                        // size and mtime, the file was moved, so emit a copy
                        // instruction. We do not check the contents of the
                        // file, because that is going to be very slow for big
                        // files. Because the reflink copies are cheap, and this
                        // is only a heuristic, this is fine.
                        let copy = CopyFile {
                            src: base_paths[0].clone(),
                            dst: path,
                        };
                        copies.push(copy);
                    }
                }
            }
        }
    }

    // Ensure the diff is deterministic, independent of hash map order.
    copies.sort();
    Ok(copies)
}

/// Call the FICLONE ioctl to make dst a reflinked copy of src.
fn clone_file(src: &fs::File, dst: &fs::File) -> io::Result<()> {
    // Not documented in "man ioctl_list", and in the header the constant is
    // constructed through a pile of preprocessor macros, but we can get the
    // raw constant by printf-ing it from a simple C program:
    //
    // #include <stdio.h>
    // #include </usr/include/linux/fs.h>
    //
    // int main() {
    //   printf("%x", FICLONE);
    //   return 0;
    // }
    const FICLONE: u64 = 0x40049409;
    let result = unsafe {
        libc::ioctl(
            dst.as_raw_fd(),
            FICLONE,
            src.as_raw_fd(),
        )
    };
    match result {
        -1 => Err(io::Error::last_os_error()),
        _ => Ok(()),
    }
}

const USAGE: &'static str = r#"btrfs-snapsync: Replay likely moves as reflink copies.

Usage:
    btrfs-snapsync apply   <src-base> <src-target> <dst-base> <dst-target>
    btrfs-snapsync dry-run <src-base> <src-target> <dst-base> <dst-target>

Diffs the file hierarchy from src-base to src-target, and detects
potential moves, based on files having the same mtime and size.

For every detected move, create a reflink:
  * With as source, the base file, but in the destination tree.
  * With as target, the target file, but in the destination tree.

In other words, this diffs src-base..src-target and replays that diff on
top of dst-base.

In "apply" mode the reflinks are created. In "dry-run" mode, we print
which reflinks would be created.

This is only a heuristic, but it sets up reflink sharing where possible,
and rsync can later fix everything up (metadata, changed files, new and
deleted files, etc.). When using rsync by itself, it would try to copy
the file, destroying potential sharing.
"#;

fn main() -> io::Result<()> {
    if env::args().len() < 6 {
        println!("{}", USAGE);
        process::exit(1);
    }

    let dry_run = match &env::args().nth(1).unwrap()[..] {
        "dry-run" => true,
        "apply" => false,
        _ => {
            println!("{}", USAGE);
            process::exit(1);
        }
    };

    let dir_base_src = env::args().nth(2).unwrap();
    let dir_target_src = env::args().nth(3).unwrap();

    let dir_base_dst = PathBuf::from(env::args().nth(4).unwrap());
    let dir_target_dst = PathBuf::from(env::args().nth(5).unwrap());

    let entries_base = scan_dir(&dir_base_src)?;
    let entries_target = scan_dir(&dir_target_src)?;

    let copies = diff(&entries_base, entries_target)?;

    for copy in copies.iter() {
        if dry_run {
            println!("{:?} -> {:?}", copy.src, copy.dst);
        } else {
            println!("TODO: For real. {:?} -> {:?}", copy.src, copy.dst);
        }
    }

    Ok(())
}
