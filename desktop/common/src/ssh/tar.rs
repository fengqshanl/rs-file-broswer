//! Minimal streaming ustar writer.
//!
//! The core crate uses the blocking `tar` crate against local files; SFTP reads
//! are async, so folder downloads build the archive incrementally here. Names
//! longer than the 100-byte ustar field are emitted as GNU long-name (`L`)
//! entries, which the existing frontend and `scripts/e2e-test.mjs` already
//! understand.

pub const BLOCK: usize = 512;
pub const END_MARKER: &str = "AGF_TAR_END_v1";

fn octal(value: u64, width: usize) -> Vec<u8> {
    let text = format!("{value:0width$o}", width = width);
    text.into_bytes()
}

fn write_field(header: &mut [u8], offset: usize, value: &[u8]) {
    let end = (offset + value.len()).min(header.len());
    header[offset..end].copy_from_slice(&value[..end - offset]);
}

fn header_block(name: &[u8], size: u64, typeflag: u8, mtime: u64, mode: u32) -> [u8; BLOCK] {
    let mut header = [0u8; BLOCK];
    write_field(&mut header, 0, name);
    write_field(&mut header, 100, &octal(mode as u64, 7));
    write_field(&mut header, 108, &octal(0, 7));
    write_field(&mut header, 116, &octal(0, 7));
    let size_field = format!("{:011o}", size);
    write_field(&mut header, 124, size_field.as_bytes());
    let mtime_field = format!("{:011o}", mtime);
    write_field(&mut header, 136, mtime_field.as_bytes());
    // Checksum is computed with the field filled with spaces.
    for byte in &mut header[148..156] {
        *byte = b' ';
    }
    header[156] = typeflag;
    write_field(&mut header, 257, b"ustar\0");
    write_field(&mut header, 263, b"00");
    let sum: u64 = header.iter().map(|byte| *byte as u64).sum();
    let checksum = format!("{sum:06o}\0 ");
    write_field(&mut header, 148, checksum.as_bytes());
    header
}

/// Emits a GNU long-name entry followed by the real header. `is_dir` decides
/// the type flag and trailing slash.
pub fn entry_header(name: &str, size: u64, is_dir: bool, mtime: u64, out: &mut Vec<u8>) {
    let mode = if is_dir { 0o755 } else { 0o644 };
    let is_long = name.len() > 100;
    if is_long {
        let payload = format!("{name}\0");
        out.extend_from_slice(&header_block(
            b"././@LongLink",
            payload.len() as u64,
            b'L',
            mtime,
            0o644,
        ));
        out.extend_from_slice(payload.as_bytes());
        let pad = (BLOCK - payload.len() % BLOCK) % BLOCK;
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    let mut short = name.as_bytes()[..name.len().min(100)].to_vec();
    if is_dir && !short.ends_with(b"/") && short.len() < 100 {
        short.push(b'/');
    }
    let typeflag = if is_dir { b'5' } else { b'0' };
    out.extend_from_slice(&header_block(
        &short,
        if is_dir { 0 } else { size },
        typeflag,
        mtime,
        mode,
    ));
}

/// Padding needed after `size` bytes of file data.
pub fn padding(size: u64) -> usize {
    (BLOCK - (size as usize % BLOCK)) % BLOCK
}

pub fn finish(out: &mut Vec<u8>) {
    out.extend(std::iter::repeat_n(0u8, BLOCK * 2));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_first_name(archive: &[u8]) -> String {
        let name = &archive[..100];
        let end = name.iter().position(|byte| *byte == 0).unwrap_or(100);
        String::from_utf8_lossy(&name[..end]).to_string()
    }

    fn checksum_ok(header: &[u8]) -> bool {
        let stored = String::from_utf8_lossy(&header[148..156])
            .trim()
            .to_string();
        let stored = u64::from_str_radix(stored.trim_end_matches('\0').trim(), 8).unwrap_or(0);
        let mut copy = header.to_vec();
        for byte in &mut copy[148..156] {
            *byte = b' ';
        }
        let sum: u64 = copy.iter().map(|byte| *byte as u64).sum();
        stored == sum
    }

    #[test]
    fn writes_a_valid_directory_header() {
        let mut out = Vec::new();
        entry_header("docs", 0, true, 1_700_000_000, &mut out);
        assert_eq!(out.len(), BLOCK);
        assert!(checksum_ok(&out));
        assert_eq!(parse_first_name(&out), "docs/");
        assert_eq!(out[156], b'5');
    }

    #[test]
    fn long_names_use_a_gnu_longlink_entry() {
        let long = format!("dir/{}", "x".repeat(180));
        let mut out = Vec::new();
        entry_header(&long, 3, false, 0, &mut out);
        // LongLink header + name payload + padding + real header
        assert!(out.len() > BLOCK * 2);
        assert_eq!(out[156], b'L');
        let payload_start = BLOCK;
        let payload = &out[payload_start..payload_start + long.len()];
        assert_eq!(String::from_utf8_lossy(payload), long);
        let real_header_start = BLOCK + ((long.len() + 1 + BLOCK - 1) / BLOCK) * BLOCK;
        assert_eq!(out[real_header_start + 156], b'0');
    }

    #[test]
    fn padding_aligns_to_blocks() {
        assert_eq!(padding(0), 0);
        assert_eq!(padding(1), 511);
        assert_eq!(padding(512), 0);
        assert_eq!(padding(513), 511);
    }

    #[test]
    fn finish_writes_two_zero_blocks() {
        let mut out = Vec::new();
        finish(&mut out);
        assert_eq!(out.len(), BLOCK * 2);
        assert!(out.iter().all(|byte| *byte == 0));
    }
}
