//! Deliberately narrow, bounded ZIP policy for bundles produced by our macOS bundler.
//! The caller owns the empty private staging directory and its cleanup on every error.
use crate::download::{Failure, Result};
use std::{
    cell::Cell,
    collections::HashMap,
    ffi::CString,
    fs::{self, File, OpenOptions, Permissions},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

const MAX_ARCHIVE: u64 = 1 << 30;
const MAX_DIRECTORY: u64 = 8 << 20;
const MAX_ENTRIES: usize = 10_000;
const MAX_ENTRY: u64 = 1 << 30;
const MAX_OUTPUT: u64 = 2 << 30;

fn invalid() -> Failure {
    Failure::new("archive", "Invalid or unsupported update archive.")
}

fn check(cancelled: &impl Fn() -> bool) -> Result<()> {
    if cancelled() {
        Err(Failure::new("cancelled", "Update extraction cancelled."))
    } else {
        Ok(())
    }
}

fn io<T>(result: std::io::Result<T>) -> Result<T> {
    result.map_err(|_| invalid())
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_at<const N: usize>(file: &mut File, offset: u64) -> Result<[u8; N]> {
    io(file.seek(SeekFrom::Start(offset)))?;
    let mut bytes = [0; N];
    io(file.read_exact(&mut bytes))?;
    Ok(bytes)
}

#[derive(Debug)]
struct Entry {
    name: String,
    directory: bool,
    mode: u32,
    flags: u16,
    method: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    header: u64,
}

struct Approved {
    entries: Vec<Entry>,
    directory_offset: u64,
    directory_and_end: Vec<u8>,
}

// ZipArchive retries older EOCD candidates on parse failures. During metadata
// construction, expose only our bounded, snapshotted directory/EOCD and a zero
// prefix. Otherwise a fake EOCD/ZIP64 record in compressed data could bypass the
// allocation limits checked by inspect(). Afterwards the same reader exposes
// the original local records and compressed data, without modifying the file.
struct DecoderSource<'a> {
    file: File,
    approved: &'a Approved,
    metadata_only: &'a Cell<bool>,
    cancelled: &'a dyn Fn() -> bool,
}

impl Read for DecoderSource<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if !self.metadata_only.get() {
            return self.file.read(buffer);
        }
        if (self.cancelled)() {
            return Err(std::io::Error::other("Extraction cancelled."));
        }
        let position = self.file.stream_position()?;
        let start = self.approved.directory_offset;
        let end = start + self.approved.directory_and_end.len() as u64;
        let count = buffer.len().min(end.saturating_sub(position) as usize);
        buffer[..count].fill(0);
        let overlap = position.max(start);
        if overlap < position + count as u64 {
            let offset = (overlap - start) as usize;
            let output_offset = (overlap - position) as usize;
            buffer[output_offset..count].copy_from_slice(
                &self.approved.directory_and_end[offset..offset + count - output_offset],
            );
        }
        self.file.seek(SeekFrom::Start(position + count as u64))?;
        Ok(count)
    }
}

impl Seek for DecoderSource<'_> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(position)
    }
}

// Extra fields can change decoder interpretation (ZIP64, Unicode names, encryption,
// Unix link targets). Only inert timestamps and uid/gid metadata are accepted.
fn extras(mut bytes: &[u8]) -> Result<()> {
    while !bytes.is_empty() {
        if bytes.len() < 4 {
            return Err(invalid());
        }
        let tag = u16_at(bytes, 0);
        let length = usize::from(u16_at(bytes, 2));
        if !matches!(tag, 0x5455 | 0x5855 | 0x7855 | 0x7875 | 0x000a) || length > bytes.len() - 4 {
            return Err(invalid());
        }
        bytes = &bytes[4 + length..];
    }
    Ok(())
}

fn path_name(raw: &[u8]) -> Result<(String, bool)> {
    if raw.is_empty() || raw.len() > 1024 {
        return Err(invalid());
    }
    let directory = raw.ends_with(b"/");
    let raw = if directory {
        &raw[..raw.len() - 1]
    } else {
        raw
    };
    let name = std::str::from_utf8(raw).map_err(|_| invalid())?;
    let parts: Vec<_> = name.split('/').collect();
    if parts.len() > 32 || parts[0] != "Resolved.app" || (parts.len() == 1 && !directory) {
        return Err(invalid());
    }
    for part in parts {
        if part.is_empty()
            || part.len() > 255
            || matches!(part, "." | "..")
            || part.ends_with(['.', ' '])
            || !part.bytes().all(|b| {
                (b' '..=b'~').contains(&b)
                    && !matches!(b, b'\\' | b':' | b'<' | b'>' | b'"' | b'|' | b'?' | b'*')
            })
        {
            return Err(invalid());
        }
        let stem = part.split('.').next().unwrap().to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(invalid());
        }
    }
    Ok((name.to_owned(), directory))
}

// Include implicit parents: a later file cannot replace one, nor can its spelling
// change on a case-insensitive filesystem. One later explicit directory is fine.
fn register(entries: &[Entry], cancelled: &impl Fn() -> bool) -> Result<()> {
    let mut paths: HashMap<String, (String, bool, bool)> = HashMap::new();
    for entry in entries {
        check(cancelled)?;
        let parts: Vec<_> = entry.name.split('/').collect();
        let mut path = String::new();
        for (index, part) in parts.iter().enumerate() {
            check(cancelled)?;
            if index != 0 {
                path.push('/');
            }
            path.push_str(part);
            let explicit = index + 1 == parts.len();
            let directory = !explicit || entry.directory;
            match paths.entry(path.to_ascii_lowercase()) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert((path.clone(), directory, explicit));
                }
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    let (spelling, was_directory, was_explicit) = slot.get_mut();
                    if spelling != &path
                        || !*was_directory
                        || !directory
                        || (explicit && *was_explicit)
                    {
                        return Err(invalid());
                    }
                    *was_explicit |= explicit;
                }
            }
        }
    }
    Ok(())
}

/// Validate normal ZIP geometry *before* allowing the decoder to allocate metadata.
/// V1 accepts no archive/file comments, padding, SFX prefix, ZIP64, or extra records.
fn inspect(file: &mut File, cancelled: &impl Fn() -> bool) -> Result<Approved> {
    check(cancelled)?;
    let length = io(file.metadata())?.len();
    if !(22..=MAX_ARCHIVE).contains(&length) {
        return Err(invalid());
    }
    let end = read_at::<22>(file, length - 22)?;
    let count = usize::from(u16_at(&end, 10));
    let directory_size = u64::from(u32_at(&end, 12));
    let directory_offset = u64::from(u32_at(&end, 16));
    if &end[..4] != b"PK\x05\x06"
        || u16_at(&end, 4) != 0
        || u16_at(&end, 6) != 0
        || usize::from(u16_at(&end, 8)) != count
        || count == 0
        || count > MAX_ENTRIES
        || directory_size > MAX_DIRECTORY
        || directory_offset + directory_size != length - 22
        || u16_at(&end, 20) != 0
    {
        return Err(invalid());
    }
    // This is the only archive-sized allocation; its bound was checked above.
    let mut central = Vec::with_capacity(directory_size as usize + 22);
    central.resize(directory_size as usize, 0);
    io(file.seek(SeekFrom::Start(directory_offset)))?;
    io(file.read_exact(&mut central))?;
    if central
        .windows(4)
        .any(|magic| matches!(magic, b"PK\x05\x06" | b"PK\x06\x06" | b"PK\x06\x07"))
    {
        return Err(invalid());
    }
    let mut cursor = 0usize;
    let mut entries = Vec::with_capacity(count);
    let mut total = 0u64;
    for _ in 0..count {
        check(cancelled)?;
        let h = central.get(cursor..cursor + 46).ok_or_else(invalid)?;
        if &h[..4] != b"PK\x01\x02" || u16_at(h, 6) > 20 || u16_at(h, 34) != 0 || u16_at(h, 32) != 0
        {
            return Err(invalid());
        }
        let flags = u16_at(h, 8);
        let method = u16_at(h, 10);
        // Only deflate tuning bits, data descriptors, and UTF-8 are meaningful.
        if flags & !0x080e != 0 || !matches!(method, 0 | 8) {
            return Err(invalid());
        }
        let name_length = usize::from(u16_at(h, 28));
        let extra_length = usize::from(u16_at(h, 30));
        let end = cursor + 46 + name_length + extra_length;
        let raw = central
            .get(cursor + 46..cursor + 46 + name_length)
            .ok_or_else(invalid)?;
        let (name, directory) = path_name(raw)?;
        extras(
            central
                .get(cursor + 46 + name_length..end)
                .ok_or_else(invalid)?,
        )?;
        let attributes = u32_at(h, 38);
        let mode = attributes >> 16;
        let kind = mode & 0o170000;
        if mode & 0o7000 != 0
            || !matches!(kind, 0 | 0o100000 | 0o040000)
            || (kind != 0 && (kind == 0o040000) != directory)
            || (attributes & 0x10 != 0 && !directory)
            || attributes & 0x08 != 0
        {
            return Err(invalid());
        }
        let size = u64::from(u32_at(h, 24));
        let compressed = u64::from(u32_at(h, 20));
        let header = u64::from(u32_at(h, 42));
        total += size;
        if size > MAX_ENTRY
            || total > MAX_OUTPUT
            || compressed >= u64::from(u32::MAX)
            || header >= directory_offset
            || (directory && size != 0)
            || (method == 0 && compressed != size)
        {
            return Err(invalid());
        }
        entries.push(Entry {
            name,
            directory,
            mode: if directory || mode & 0o111 != 0 {
                0o755
            } else {
                0o644
            },
            flags,
            method,
            crc: u32_at(h, 16),
            compressed,
            size,
            header,
        });
        cursor = end;
    }
    if cursor != central.len() {
        return Err(invalid());
    }
    register(&entries, cancelled)?;
    // Local records must cover the prefix exactly, without overlaps, aliases,
    // hidden entries, ZIP64 records, or a self-extracting stub.
    let mut order: Vec<_> = entries.iter().collect();
    order.sort_unstable_by_key(|entry| entry.header);
    let mut next = 0u64;
    for entry in order {
        check(cancelled)?;
        if entry.header != next {
            return Err(invalid());
        }
        let h = read_at::<30>(file, next)?;
        if &h[..4] != b"PK\x03\x04"
            || u16_at(&h, 4) > 20
            || u16_at(&h, 6) != entry.flags
            || u16_at(&h, 8) != entry.method
        {
            return Err(invalid());
        }
        let name_length = usize::from(u16_at(&h, 26));
        let extra_length = usize::from(u16_at(&h, 28));
        let expected = format!("{}{}", entry.name, if entry.directory { "/" } else { "" });
        if name_length != expected.len() {
            return Err(invalid());
        }
        let mut name = vec![0; name_length];
        io(file.read_exact(&mut name))?;
        if name != expected.as_bytes() {
            return Err(invalid());
        }
        let mut extra = vec![0; extra_length];
        io(file.read_exact(&mut extra))?;
        extras(&extra)?;
        next += 30 + name_length as u64 + extra_length as u64 + entry.compressed;
        if next > directory_offset {
            return Err(invalid());
        }
        let local = (u32_at(&h, 14), u32_at(&h, 18), u32_at(&h, 22));
        let declared = (entry.crc, entry.compressed as u32, entry.size as u32);
        if entry.flags & 8 == 0 {
            if local != declared {
                return Err(invalid());
            }
        } else {
            if local != (0, 0, 0) && local != declared {
                return Err(invalid());
            }
            let first = read_at::<4>(file, next)?;
            let signed = &first == b"PK\x07\x08";
            let descriptor = read_at::<12>(file, next + if signed { 4 } else { 0 })?;
            if (
                u32_at(&descriptor, 0),
                u32_at(&descriptor, 4),
                u32_at(&descriptor, 8),
            ) != declared
            {
                return Err(invalid());
            }
            next += if signed { 16 } else { 12 };
        }
    }
    if next != directory_offset {
        return Err(invalid());
    }
    central.extend_from_slice(&end);
    Ok(Approved {
        entries,
        directory_offset,
        directory_and_end: central,
    })
}

// All mutations are relative to already-open directory descriptors. Even a
// replaced path or a symlink cannot redirect a write out of the staging tree.
fn directory_at(parent: &File, name: &str) -> Result<File> {
    let name = CString::new(name).map_err(|_| invalid())?;
    // SAFETY: the descriptor is live and the component is a valid C string.
    let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o755) };
    if created != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
        return Err(invalid());
    }
    // SAFETY: same descriptor/string lifetime; openat returns a new owned fd.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(invalid());
    }
    // SAFETY: fd was successfully opened above and has exactly one owner.
    let directory = unsafe { File::from_raw_fd(fd) };
    io(directory.set_permissions(Permissions::from_mode(0o755)))?;
    Ok(directory)
}

fn file_at(parent: &File, name: &str) -> Result<File> {
    let name = CString::new(name).map_err(|_| invalid())?;
    // SAFETY: live parent descriptor and valid single component; O_EXCL is
    // create_new, while O_NOFOLLOW independently refuses symbolic links.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(invalid());
    }
    // SAFETY: fd was successfully opened above and has exactly one owner.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Extract one `Resolved.app` into an existing, empty, owner-only directory.
/// Never executes archive contents. Partial output belongs to the caller on error.
pub fn extract(
    archive: &Path,
    destination: &Path,
    cancelled: impl Fn() -> bool,
) -> Result<PathBuf> {
    check(&cancelled)?;
    let root = io(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(destination))?;
    let metadata = io(root.metadata())?;
    // SAFETY: geteuid has no preconditions.
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(invalid());
    }
    if io(fs::read_dir(destination))?.next().is_some() {
        return Err(invalid());
    }
    let mut source = io(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(archive))?;
    if !io(source.metadata())?.is_file() {
        return Err(invalid());
    }
    let approved = inspect(&mut source, &cancelled)?;
    let metadata_only = Cell::new(true);
    let decoder = zip::ZipArchive::new(DecoderSource {
        file: source,
        approved: &approved,
        metadata_only: &metadata_only,
        cancelled: &cancelled,
    });
    check(&cancelled)?;
    let mut decoder = decoder.map_err(|_| invalid())?;
    metadata_only.set(false);
    if decoder.len() != approved.entries.len() || decoder.offset() != 0 {
        return Err(invalid());
    }
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    for (index, entry) in approved.entries.iter().enumerate() {
        check(&cancelled)?;
        let mut input = decoder.by_index(index).map_err(|_| invalid())?;
        let expected = format!("{}{}", entry.name, if entry.directory { "/" } else { "" });
        if input.name_raw() != expected.as_bytes()
            || input.encrypted()
            || input.size() != entry.size
            || input.compressed_size() != entry.compressed
            || input.crc32() != entry.crc
            || input.header_start() != entry.header
        {
            return Err(invalid());
        }
        let mut parent = io(root.try_clone())?;
        let parts: Vec<_> = entry.name.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            check(&cancelled)?;
            parent = directory_at(&parent, part)?;
        }
        check(&cancelled)?;
        let name = parts.last().unwrap();
        let mut output = if entry.directory {
            directory_at(&parent, name)?;
            None
        } else {
            Some(file_at(&parent, name)?)
        };
        let mut written = 0u64;
        loop {
            check(&cancelled)?;
            // Read through EOF, even for directories: the maintained decoder
            // performs CRC validation there. Never trust the declared size.
            let count = io(input.read(&mut buffer))?;
            if count == 0 {
                break;
            }
            written += count as u64;
            total += count as u64;
            if written > entry.size || written > MAX_ENTRY || total > MAX_OUTPUT {
                return Err(invalid());
            }
            if let Some(output) = &mut output {
                io(output.write_all(&buffer[..count]))?;
            } else {
                return Err(invalid());
            }
        }
        if written != entry.size {
            return Err(invalid());
        }
        if let Some(output) = output {
            io(output.set_permissions(Permissions::from_mode(entry.mode)))?;
        }
    }
    check(&cancelled)?;
    Ok(destination.join("Resolved.app"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, os::unix::fs::symlink};
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    // The maintained writer supplies compressed data and CRCs. Assemble its
    // independent records so even duplicate names/special modes can be tested.
    fn fixture(items: &[(&str, u32, &[u8])], method: CompressionMethod) -> Vec<u8> {
        let mut local = Vec::new();
        let mut central = Vec::new();
        for (name, mode, data) in items {
            let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
            let options = SimpleFileOptions::default().compression_method(method);
            let writer_name = if name.ends_with('/') {
                format!("{}/", "x".repeat(name.len() - 1))
            } else {
                "x".repeat(name.len())
            };
            if name.ends_with('/') {
                writer.add_directory(writer_name, options).unwrap();
            } else {
                writer.start_file(writer_name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            let mut bytes = writer.finish().unwrap().into_inner();
            let offset = u32_at(&bytes, bytes.len() - 6) as usize;
            bytes[30..30 + name.len()].copy_from_slice(name.as_bytes());
            let mut header = bytes[offset..bytes.len() - 22].to_vec();
            header[46..46 + name.len()].copy_from_slice(name.as_bytes());
            put32(&mut header, 38, mode << 16);
            put32(&mut header, 42, local.len() as u32);
            central.extend(header);
            local.extend_from_slice(&bytes[..offset]);
        }
        let offset = local.len();
        local.extend_from_slice(&central);
        let mut end = [0u8; 22];
        end[..4].copy_from_slice(b"PK\x05\x06");
        put16(&mut end, 8, items.len() as u16);
        put16(&mut end, 10, items.len() as u16);
        put32(&mut end, 12, central.len() as u32);
        put32(&mut end, 16, offset as u32);
        local.extend(end);
        local
    }

    fn put16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn central(bytes: &[u8]) -> usize {
        u32_at(bytes, bytes.len() - 6) as usize
    }

    fn single() -> Vec<u8> {
        fixture(
            &[("Resolved.app/file", 0o100644, b"hello")],
            CompressionMethod::Stored,
        )
    }

    fn run(bytes: &[u8], cancelled: impl Fn() -> bool) -> (tempfile::TempDir, Result<PathBuf>) {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("update.zip");
        let destination = temp.path().join("stage");
        fs::write(&archive, bytes).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::set_permissions(&destination, Permissions::from_mode(0o700)).unwrap();
        let result = extract(&archive, &destination, cancelled);
        (temp, result)
    }

    fn rejects(bytes: &[u8]) {
        let (temp, result) = run(bytes, || false);
        assert!(result.is_err());
        assert!(!temp.path().join("outside").exists());
        assert!(!temp.path().join("file").exists());
    }

    #[test]
    fn current_style_app_and_executable_permissions() {
        for method in [CompressionMethod::Stored, CompressionMethod::Deflated] {
            let bytes = fixture(
                &[
                    ("Resolved.app/", 0o040755, b""),
                    ("Resolved.app/Contents/", 0o040755, b""),
                    ("Resolved.app/Contents/Info.plist", 0o100644, b"<plist/>"),
                    (
                        "Resolved.app/Contents/MacOS/api-tester",
                        0o100755,
                        b"executable",
                    ),
                    ("Resolved.app/Contents/MacOS/", 0o040755, b""),
                ],
                method,
            );
            let (_temp, result) = run(&bytes, || false);
            let app = result.unwrap();
            assert_eq!(
                fs::read(app.join("Contents/Info.plist")).unwrap(),
                b"<plist/>"
            );
            assert_eq!(
                fs::metadata(app.join("Contents/MacOS/api-tester"))
                    .unwrap()
                    .mode()
                    & 0o7777,
                0o755
            );
            assert_eq!(
                fs::metadata(app.join("Contents/Info.plist"))
                    .unwrap()
                    .mode()
                    & 0o7777,
                0o644
            );
            assert_eq!(fs::metadata(&app).unwrap().mode() & 0o7777, 0o755);
        }
    }

    #[test]
    fn rejects_unsafe_paths_without_writes() {
        for name in [
            "../outside",
            "/outside",
            "Resolved.app/../../outside",
            "Resolved.app/./file",
            "Resolved.app//file",
            "Resolved.app\\file",
            "Resolved.app/C:file",
            "Resolved.app/\0file",
            "Resolved.app/\nfile",
            "Other.app/file",
            "resolved.app/file",
            "Resolved.app/file.",
            "Resolved.app/file ",
            "Resolved.app/NUL.txt",
            "Resolved.app/\u{e9}",
            "Resolved.app/a*/file",
        ] {
            assert!(path_name(name.as_bytes()).is_err(), "{name:?}");
            rejects(&fixture(
                &[(name, 0o100644, b"bad")],
                CompressionMethod::Stored,
            ));
        }
        let bytes = fixture(
            &[("Resolved.app/../outside", 0o100644, b"bad")],
            CompressionMethod::Stored,
        );
        let (temp, result) = run(&bytes, || false);
        assert!(result.is_err());
        assert_eq!(fs::read_dir(temp.path().join("stage")).unwrap().count(), 0);
        assert!(!temp.path().join("outside").exists());
    }

    #[test]
    fn path_depth_and_lengths_are_bounded() {
        assert!(path_name(format!("Resolved.app/{}", "a".repeat(256)).as_bytes()).is_err());
        assert!(path_name(format!("Resolved.app/{}file", "a/".repeat(32)).as_bytes()).is_err());
        assert!(
            path_name(format!("Resolved.app/{}", vec!["a".repeat(250); 5].join("/")).as_bytes())
                .is_err()
        );
        assert!(path_name(b"Resolved.app//").is_err());
        assert!(path_name(b"Resolved.app").is_err());
    }

    #[test]
    fn rejects_links_special_files_and_unsafe_permissions() {
        for mode in [
            0o120777, 0o010644, 0o020644, 0o060644, 0o140644, 0o104755, 0o102755, 0o101755,
            0o040755,
        ] {
            rejects(&fixture(
                &[("Resolved.app/file", mode, b"hello")],
                CompressionMethod::Stored,
            ));
        }
        rejects(&fixture(
            &[("Resolved.app/", 0o100644, b"")],
            CompressionMethod::Stored,
        ));
    }

    #[test]
    fn rejects_duplicates_case_aliases_and_implicit_parent_conflicts() {
        for names in [
            ["Resolved.app/file", "Resolved.app/file"],
            ["Resolved.app/file", "Resolved.app/FILE"],
            ["Resolved.app/Dir/a", "Resolved.app/dir/b"],
            ["Resolved.app/Dir/a", "Resolved.app/Dir"],
            ["Resolved.app/Dir", "Resolved.app/Dir/a"],
            ["Resolved.app/Dir/", "Resolved.app/Dir/"],
            ["Resolved.app/Dir/", "Resolved.app/Dir"],
        ] {
            rejects(&fixture(
                &[
                    (
                        names[0],
                        if names[0].ends_with('/') {
                            0o040755
                        } else {
                            0o100644
                        },
                        b"",
                    ),
                    (
                        names[1],
                        if names[1].ends_with('/') {
                            0o040755
                        } else {
                            0o100644
                        },
                        b"",
                    ),
                ],
                CompressionMethod::Stored,
            ));
        }
    }

    #[test]
    fn rejects_eocd_geometry_zip64_multidisk_and_limits() {
        let bytes = single();
        let end = bytes.len() - 22;
        for (offset, value) in [(4, 1), (6, 1), (8, 2), (10, 10_001), (10, 65_535), (20, 1)] {
            let mut bad = bytes.clone();
            put16(&mut bad, end + offset, value);
            rejects(&bad);
        }
        for (offset, value) in [
            (12, (MAX_DIRECTORY + 1) as u32),
            (12, u32::MAX),
            (16, u32::MAX),
        ] {
            let mut bad = bytes.clone();
            put32(&mut bad, end + offset, value);
            rejects(&bad);
        }
        let mut trailing = bytes.clone();
        trailing.extend_from_slice(b"PK\x05\x06");
        rejects(&trailing);
        let mut prefix = b"stub".to_vec();
        prefix.extend_from_slice(&bytes);
        let new_end = prefix.len() - 22;
        put32(&mut prefix, new_end + 16, central(&bytes) as u32 + 4);
        rejects(&prefix);
        let mut zip64 = bytes.clone();
        zip64.splice(end..end, [0u8; 20]);
        zip64[end..end + 4].copy_from_slice(b"PK\x06\x07");
        rejects(&zip64);
        assert!(extras(&[1, 0, 0, 0]).is_err());
        assert!(extras(&[0x75, 0x70, 0, 0]).is_err());
        let temp = tempfile::tempdir().unwrap();
        let mut sparse = File::create(temp.path().join("large.zip")).unwrap();
        sparse.set_len(MAX_ARCHIVE + 1).unwrap();
        assert!(inspect(&mut sparse, &|| false).is_err());
    }

    #[test]
    fn rejects_encryption_unsupported_compression_and_local_disagreement() {
        let bytes = single();
        let c = central(&bytes);
        for flags in [1, 0x40, 0x2000] {
            let mut bad = bytes.clone();
            put16(&mut bad, c + 8, flags);
            put16(&mut bad, 6, flags);
            rejects(&bad);
        }
        let mut bad = bytes.clone();
        put16(&mut bad, c + 10, 12);
        put16(&mut bad, 8, 12);
        rejects(&bad);
        let mut bad = bytes.clone();
        bad[30] = b'X';
        rejects(&bad);
        let mut bad = bytes.clone();
        put16(&mut bad, c + 34, 1);
        rejects(&bad);
        let mut bad = bytes.clone();
        put32(&mut bad, c + 42, 1);
        rejects(&bad);
    }

    #[test]
    fn validates_signed_and_unsigned_data_descriptors() {
        for signed in [false, true] {
            let mut bytes = single();
            let c = central(&bytes);
            let mut descriptor = Vec::new();
            if signed {
                descriptor.extend_from_slice(b"PK\x07\x08");
            }
            descriptor.extend_from_slice(&bytes[c + 16..c + 28]);
            put16(&mut bytes, 6, 8);
            put16(&mut bytes, c + 8, 8);
            bytes[14..26].fill(0);
            bytes.splice(c..c, descriptor.iter().copied());
            let end = bytes.len() - 22;
            put32(&mut bytes, end + 16, (c + descriptor.len()) as u32);
            let (_temp, result) = run(&bytes, || false);
            result.unwrap();
            bytes[c + descriptor.len() - 1] ^= 1;
            rejects(&bytes);
        }
    }

    #[test]
    fn decoder_metadata_view_hides_payload_footer_candidates() {
        let mut payload = vec![0; 64];
        payload[..4].copy_from_slice(b"PK\x05\x06");
        payload[24..28].copy_from_slice(b"PK\x06\x07");
        let bytes = fixture(
            &[("Resolved.app/file", 0o100644, &payload)],
            CompressionMethod::Stored,
        );
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("archive.zip");
        fs::write(&path, &bytes).unwrap();
        let mut file = File::open(&path).unwrap();
        let approved = inspect(&mut file, &|| false).unwrap();
        let metadata_only = Cell::new(true);
        let mut reader = DecoderSource {
            file,
            approved: &approved,
            metadata_only: &metadata_only,
            cancelled: &|| false,
        };
        reader.seek(SeekFrom::Start(0)).unwrap();
        let mut prefix = vec![1; approved.directory_offset as usize];
        reader.read_exact(&mut prefix).unwrap();
        assert!(prefix.iter().all(|byte| *byte == 0));
        let (_temp, result) = run(&bytes, || false);
        assert_eq!(fs::read(result.unwrap().join("file")).unwrap(), payload);
        let mut ambiguous = single();
        let c = central(&ambiguous);
        ambiguous[c + 16..c + 20].copy_from_slice(b"PK\x06\x07");
        ambiguous[14..18].copy_from_slice(b"PK\x06\x07");
        rejects(&ambiguous);
    }

    #[test]
    fn rejects_truncation_and_bad_crc() {
        let bytes = single();
        for length in [0, 4, 21, bytes.len() - 1, bytes.len() - 22] {
            rejects(&bytes[..length]);
        }
        let mut bad = bytes.clone();
        let data = 30 + usize::from(u16_at(&bad, 26)) + usize::from(u16_at(&bad, 28));
        bad[data] ^= 1;
        rejects(&bad);
    }

    #[test]
    fn rejects_declared_and_actual_size_mismatches() {
        let bytes = fixture(
            &[("Resolved.app/file", 0o100644, b"hello")],
            CompressionMethod::Deflated,
        );
        let c = central(&bytes);
        for size in [0, 4, 6, (MAX_ENTRY + 1) as u32, u32::MAX] {
            let mut bad = bytes.clone();
            put32(&mut bad, c + 24, size);
            put32(&mut bad, 22, size);
            rejects(&bad);
        }
        let mut aggregate = fixture(
            &[
                ("Resolved.app/a", 0o100644, b"a"),
                ("Resolved.app/b", 0o100644, b"b"),
                ("Resolved.app/c", 0o100644, b"c"),
            ],
            CompressionMethod::Deflated,
        );
        let mut c = central(&aggregate);
        for _ in 0..3 {
            put32(&mut aggregate, c + 24, MAX_ENTRY as u32);
            c += 46
                + usize::from(u16_at(&aggregate, c + 28))
                + usize::from(u16_at(&aggregate, c + 30));
        }
        rejects(&aggregate);
    }

    #[test]
    fn cancellation_before_metadata_and_during_streaming_and_traversal() {
        let (temp, result) = run(&single(), || true);
        assert_eq!(result.unwrap_err().code, "cancelled");
        assert_eq!(fs::read_dir(temp.path().join("stage")).unwrap().count(), 0);
        let bytes = fixture(
            &[("Resolved.app/a/b/c/file", 0o100644, &vec![b'x'; 256 * 1024])],
            CompressionMethod::Deflated,
        );
        // First discover the deterministic number of cancellation checkpoints.
        let calls = Cell::new(0);
        let (_temp, result) = run(&bytes, || {
            calls.set(calls.get() + 1);
            false
        });
        result.unwrap();
        for stop in 1..=calls.get() {
            let seen = Cell::new(0);
            let (_temp, result) = run(&bytes, || {
                seen.set(seen.get() + 1);
                seen.get() >= stop
            });
            assert_eq!(result.unwrap_err().code, "cancelled", "checkpoint {stop}");
        }
    }

    #[test]
    fn destination_must_be_empty_private_and_not_a_link() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("update.zip");
        fs::write(&archive, single()).unwrap();
        let destination = temp.path().join("stage");
        assert!(extract(&archive, &destination, || false).is_err());
        fs::create_dir(&destination).unwrap();
        fs::set_permissions(&destination, Permissions::from_mode(0o755)).unwrap();
        assert!(extract(&archive, &destination, || false).is_err());
        fs::set_permissions(&destination, Permissions::from_mode(0o700)).unwrap();
        fs::write(destination.join("existing"), b"keep").unwrap();
        assert!(extract(&archive, &destination, || false).is_err());
        assert_eq!(fs::read(destination.join("existing")).unwrap(), b"keep");
        let link = temp.path().join("link");
        symlink(&destination, &link).unwrap();
        assert!(extract(&archive, &link, || false).is_err());
    }

    #[test]
    fn descriptor_relative_writes_refuse_links_and_existing_files() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), temp.path().join("link")).unwrap();
        let root = File::open(temp.path()).unwrap();
        assert!(directory_at(&root, "link").is_err());
        assert!(file_at(&root, "link").is_err());
        fs::write(temp.path().join("existing"), b"keep").unwrap();
        assert!(file_at(&root, "existing").is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    /// Optional real release fixture, without checking large binaries into git.
    /// Run only through the approved bundler verification workflow.
    #[test]
    #[ignore = "requires RESOLVED_UPDATE_ARCHIVE_FIXTURE pointing to a current bundle ZIP"]
    fn real_current_bundle() {
        let archive = std::env::var_os("RESOLVED_UPDATE_ARCHIVE_FIXTURE").unwrap();
        let destination = tempfile::Builder::new()
            .permissions(Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let mut source = File::open(&archive).unwrap();
        inspect(&mut source, &|| false).expect("release ZIP must satisfy bounded metadata policy");
        let app = extract(Path::new(&archive), destination.path(), || false).unwrap();
        assert!(app.join("Contents/Info.plist").is_file());
        assert!(app.join("Contents/MacOS/api-tester").is_file());
    }
}
