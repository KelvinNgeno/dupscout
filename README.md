# dupscout
![CI](https://github.com/KelvinNgeno/dupscout/actions/workflows/ci.yml/badge.svg)
Find duplicate files and see how much disk space they waste. Written in Rust
with zero third-party dependencies.

dupscout is **read-only**. It reports duplicates and never deletes anything.

## Build and run

```
cargo build --release
./target/release/dupscout ~/Downloads
./target/release/dupscout --min-size 1048576 ~/Pictures   # only files >= 1 MB
```

## How it works

1. Group files by size (a unique size means no twin, so most files are never read).
2. Hash the contents of files that share a size (FNV-1a, 64-bit).
3. Confirm matches byte-by-byte, so a hash collision cannot cause a false report.

Symlinks are skipped. Unreadable files are reported and skipped, and the
program exits with code 1 if any were skipped.

## Test

```
cargo test
```

## Limitations

- Hard links to the same data are reported as duplicates even though they use no extra space.
- Reports only; no delete or hard-link option yet.

## Roadmap

- [ ] `--json` output
- [ ] Parallel hashing with threads
- [ ] Optional interactive delete