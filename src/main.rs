extern crate walkdir;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::env;
use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Eq, Ord, Debug, Hash, PartialEq, PartialOrd)]
struct FileInfo {
    len: u64,
    mtime: SystemTime,
}

fn main() -> io::Result<()> {
    let base_dir = env::args().nth(1).unwrap();
    let target_dir = env::args().nth(2).unwrap();
    let base_wd = walkdir::WalkDir::new(&base_dir).max_open(128);
    let target_wd = walkdir::WalkDir::new(&target_dir).max_open(128);

    let mut entries: HashMap<FileInfo, Vec<PathBuf>> = HashMap::new();
    let mut n = 0_u64;

    for entry_opt in base_wd {
        let entry = entry_opt?;
        let meta = entry.metadata()?;
        let len = meta.len();
        let mtime = meta.modified()?;
        let file_info = FileInfo { len, mtime };
        let path = entry.into_path();
        match entries.entry(file_info) {
            Entry::Occupied(mut e) => { e.get_mut().push(path); }
            Entry::Vacant(e) => { e.insert(vec![path]); }
        };
        n += 1;
    }

    println!("{} {}", n, entries.len());

    for (ref k, ref vs) in entries.iter() {
        if vs.len() > 1 {
            println!("{:?} {:?}", k, vs);
        }
    }

    Ok(())
}
