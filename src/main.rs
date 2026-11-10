//! dupscout - find duplicate files and show how much space they waste.
//!
//! Strategy (cheap checks first, so most files are never fully read):
//!   1. Group files by size. A file with a unique size cannot have a twin.
//!   2. Hash the contents of files that share a size.
//!   3. Compare byte-by-byte to confirm, so a hash collision can never
//!      produce a false report.

use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process;

const BUF_SIZE: usize = 64 * 1024;

struct Config {
    root: PathBuf,
    min_size: u64,
}

struct DupGroup {
    size: u64,
    paths: Vec<PathBuf>,
}

impl DupGroup {
    /// Space that could be freed by keeping only one copy.
    fn wasted(&self) -> u64 {
        self.size * (self.paths.len() as u64 - 1)
    }
}

fn print_help() {
    println!("dupscout - find duplicate files\n");
    println!("USAGE:");
    println!("    dupscout [OPTIONS] [DIRECTORY]\n");
    println!("OPTIONS:");
    println!("    -m, --min-size <BYTES>   Ignore files smaller than this (default: 1)");
    println!("    -h, --help               Show this help\n");
    println!("dupscout only reports duplicates. It never deletes anything.");
}

fn parse_args() -> Result<Config, String> {
    let mut root = PathBuf::from(".");
    let mut min_size: u64 = 1;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                process::exit(0);
            }
            "-m" | "--min-size" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--min-size needs a value".to_string())?;
                min_size = value
                    .parse()
                    .map_err(|_| format!("invalid size: {}", value))?;
            }
            s if s.starts_with('-') => return Err(format!("unknown option: {}", s)),
            _ => root = PathBuf::from(&arg),
        }
    }
    Ok(Config { root, min_size })
}

/// Walk `dir` recursively and bucket regular files by size.
/// Symlinks are skipped so we never loop or count a file twice.
fn collect(dir: &Path, min_size: u64, by_size: &mut HashMap<u64, Vec<PathBuf>>, errors: &mut u32) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skipped {}: {}", dir.display(), e);
            *errors += 1;
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                *errors += 1;
                continue;
            }
        };
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("skipped {}: {}", path.display(), e);
                *errors += 1;
                continue;
            }
        };
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect(&path, min_size, by_size, errors);
        } else if file_type.is_file() && meta.len() >= min_size {
            by_size.entry(meta.len()).or_default().push(path);
        }
    }
}

/// FNV-1a 64-bit hash of a file's contents, read in chunks.
/// It is not cryptographic, which is fine: matches are verified byte-by-byte.
fn hash_file(path: &Path) -> io::Result<u64> {
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; BUF_SIZE];
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    Ok(hash)
}

/// Read until `buf` is full or the file ends. Returns bytes read.
fn read_full<R: Read>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        let n = reader.read(&mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

fn files_equal(a: &Path, b: &Path) -> io::Result<bool> {
    let mut fa = File::open(a)?;
    let mut fb = File::open(b)?;
    let mut buf_a = vec![0u8; BUF_SIZE];
    let mut buf_b = vec![0u8; BUF_SIZE];
    loop {
        let na = read_full(&mut fa, &mut buf_a)?;
        let nb = read_full(&mut fb, &mut buf_b)?;
        if na != nb || buf_a[..na] != buf_b[..nb] {
            return Ok(false);
        }
        if na == 0 {
            return Ok(true);
        }
    }
}

/// Scan `root` and return groups of identical files plus a count of
/// items that could not be read.
fn scan(root: &Path, min_size: u64) -> (Vec<DupGroup>, u32) {
    let mut errors: u32 = 0;
    let mut by_size: HashMap<u64, Vec<PathBuf>> = HashMap::new();
    collect(root, min_size, &mut by_size, &mut errors);

    let mut groups: Vec<DupGroup> = Vec::new();

    for (size, paths) in by_size {
        if paths.len() < 2 {
            continue;
        }

        let mut by_hash: HashMap<u64, Vec<PathBuf>> = HashMap::new();
        for path in paths {
            match hash_file(&path) {
                Ok(h) => by_hash.entry(h).or_default().push(path),
                Err(e) => {
                    eprintln!("skipped {}: {}", path.display(), e);
                    errors += 1;
                }
            }
        }

        for (_, candidates) in by_hash {
            if candidates.len() < 2 {
                continue;
            }
            // Confirm with a byte-by-byte comparison.
            let mut confirmed: Vec<Vec<PathBuf>> = Vec::new();
            for path in candidates {
                let mut placed = false;
                for group in confirmed.iter_mut() {
                    match files_equal(&group[0], &path) {
                        Ok(true) => {
                            group.push(path.clone());
                            placed = true;
                            break;
                        }
                        Ok(false) => {}
                        Err(e) => {
                            eprintln!("skipped {}: {}", path.display(), e);
                            errors += 1;
                            placed = true;
                            break;
                        }
                    }
                }
                if !placed {
                    confirmed.push(vec![path]);
                }
            }
            for mut paths in confirmed {
                if paths.len() > 1 {
                    paths.sort();
                    groups.push(DupGroup { size, paths });
                }
            }
        }
    }

    groups.sort_by(|a, b| {
        b.wasted()
            .cmp(&a.wasted())
            .then_with(|| a.paths[0].cmp(&b.paths[0]))
    });
    (groups, errors)
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

fn main() {
    let config = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {}\n", e);
            print_help();
            process::exit(2);
        }
    };

    if !config.root.is_dir() {
        eprintln!("error: {} is not a directory", config.root.display());
        process::exit(2);
    }

    println!("Scanning {} ...\n", config.root.display());
    let (groups, errors) = scan(&config.root, config.min_size);

    if groups.is_empty() {
        println!("No duplicate files found.");
    } else {
        let mut wasted: u64 = 0;
        let mut extra_copies: usize = 0;
        for (i, group) in groups.iter().enumerate() {
            println!(
                "Group {}: {} copies, {} each",
                i + 1,
                group.paths.len(),
                human(group.size)
            );
            for path in &group.paths {
                println!("  {}", path.display());
            }
            println!();
            wasted += group.wasted();
            extra_copies += group.paths.len() - 1;
        }
        println!(
            "{} duplicate file(s) in {} group(s). Reclaimable space: {}",
            extra_copies,
            groups.len(),
            human(wasted)
        );
    }

    if errors > 0 {
        eprintln!("{} item(s) could not be read and were skipped.", errors);
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("dupscout_{}_{}", name, process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_duplicates_across_subfolders() {
        let dir = temp_dir("basic");
        fs::write(dir.join("a.txt"), b"hello").unwrap();
        fs::write(dir.join("b.txt"), b"hello").unwrap();
        fs::write(dir.join("c.txt"), b"hallo").unwrap(); // same size, different content
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("d.txt"), b"hello").unwrap();

        let (groups, errors) = scan(&dir, 1);
        assert_eq!(errors, 0);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].paths.len(), 3);
        assert_eq!(groups[0].wasted(), 10);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unique_files_report_nothing() {
        let dir = temp_dir("unique");
        fs::write(dir.join("a.txt"), b"one").unwrap();
        fs::write(dir.join("b.txt"), b"three").unwrap();
        let (groups, _) = scan(&dir, 1);
        assert!(groups.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn min_size_filters_small_files() {
        let dir = temp_dir("minsize");
        fs::write(dir.join("a.txt"), b"hi").unwrap();
        fs::write(dir.join("b.txt"), b"hi").unwrap();
        let (groups, _) = scan(&dir, 100);
        assert!(groups.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn human_sizes_read_well() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(2048), "2.0 KB");
        assert_eq!(human(5 * 1024 * 1024), "5.0 MB");
    }
}
