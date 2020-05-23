extern crate libc;
extern crate walkdir;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::env;
use std::fs;
use std::ffi::OsString;
use std::io;
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
    entries_size_mtime: HashMap<FileInfo, Vec<PathBuf>>,
    entries_size: HashMap<u64, Vec<PathBuf>>,
    entries_name: HashMap<OsString, Vec<PathBuf>>,
}

impl DirScan {
    fn get(&self, path: &Path, info: &FileInfo) -> Option<&[PathBuf]> {
        if let Some(paths) = self.entries_size_mtime.get(info) {
            return Some(&paths[..]);
        }
        if let Some(paths) = self.entries_size.get(&info.len) {
            return Some(&paths[..]);
        }
        if let Some(fname) = path.file_name() {
            if let Some(paths) = self.entries_name.get(fname) {
                return Some(&paths[..]);
            }
        }
        None
    }
}

fn scan_dir<P: AsRef<Path>>(dir_path: P) -> io::Result<DirScan> {
    let mut entries_size_mtime: HashMap<FileInfo, Vec<PathBuf>> = HashMap::new();
    let mut entries_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    let mut entries_name: HashMap<OsString, Vec<PathBuf>> = HashMap::new();

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
        let fname = match full_path.file_name() {
            Some(name) => name.to_os_string(),
            None => panic!("Expected file in directory to have a file name."),
        };

        match entries_size_mtime.entry(file_info) {
            Entry::Occupied(mut e) => { e.get_mut().push(rel_path.clone()); }
            Entry::Vacant(e) => { e.insert(vec![rel_path.clone()]); }
        };
        match entries_size.entry(len) {
            Entry::Occupied(mut e) => { e.get_mut().push(rel_path.clone()); }
            Entry::Vacant(e) => { e.insert(vec![rel_path.clone()]); }
        };
        match entries_name.entry(fname) {
            Entry::Occupied(mut e) => { e.get_mut().push(rel_path); }
            Entry::Vacant(e) => { e.insert(vec![rel_path]); }
        };
    }

    // Sort entries to ensure reproducible results.
    for (_, ref mut v) in entries_size_mtime.iter_mut() { v.sort(); }
    for (_, ref mut v) in entries_size.iter_mut() { v.sort(); }
    for (_, ref mut v) in entries_name.iter_mut() { v.sort(); }

    let result = DirScan {
        entries_size_mtime,
        entries_size,
        entries_name,
    };

    Ok(result)
}

/// Detect potentially moved files, and emit a copy operation for each.
fn diff(base: &DirScan, mut target: DirScan) -> io::Result<Vec<CopyFile>> {
    let mut copies = Vec::new();

    for (info, mut paths) in target.entries_size_mtime.drain() {
        for path in paths.drain(..) {
            match base.get(&path, &info) {
                None => {
                    println!("MISSING {:?}", path);
                },
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

fn clone_paths(src: PathBuf, dst: PathBuf) -> io::Result<()> {
    let parent = dst.parent().expect("Destination should be a subdirectory, so it has a parent.");
    fs::create_dir_all(parent)?;
    println!("open src: {:?}", src);
    let f_src = fs::File::open(src)?;
    println!("open dst: {:?}", dst);
    let f_dst = fs::File::create(dst)?;
    println!("clone");
    clone_file(&f_src, &f_dst)
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
            let mut src_path = dir_base_dst.clone();
            let mut dst_path = dir_target_dst.clone();
            src_path.push(&copy.src);
            dst_path.push(&copy.dst);
            println!("{:?} -> {:?}", src_path, dst_path);
            clone_paths(src_path, dst_path)?;
        }
    }

    Ok(())
}
