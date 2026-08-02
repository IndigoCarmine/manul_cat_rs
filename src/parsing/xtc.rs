use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

// GROMACS XTC magic number
const XTC_MAGIC: i32 = 1995;

// GROMACS coordinate-compression "magic integers": the per-axis sizes that the
// small-coordinate encoder steps through. Spaced by roughly 2^(1/3) (three axes
// multiply together), NOT powers of two. Faithful port of xdrfile's table.
// magicints[FIRSTIDX - 1] == 0.
const MAGICINTS: [i32; 73] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 10, 12, 16, 20, 25, 32, 40, 50, 64, 80, 101, 128, 161, 203, 256,
    322, 406, 512, 645, 812, 1024, 1290, 1625, 2048, 2580, 3250, 4096, 5060, 6501, 8192, 10321,
    13003, 16384, 20642, 26007, 32768, 41285, 52015, 65536, 82570, 104031, 131072, 165140, 208063,
    262144, 330280, 416127, 524287, 660561, 832255, 1048576, 1321122, 1664510, 2097152, 2642245,
    3329021, 4194304, 5284491, 6658042, 8388607, 10568983, 13316085, 16777216,
];
const FIRSTIDX: i32 = 9;

pub struct XtcFrame {
    pub step: i32,
    pub time: f32,
    pub box_matrix: [[f32; 3]; 3],
    pub positions: Vec<[f32; 3]>,
}

pub struct XtcFile {
    pub frames: Vec<XtcFrame>,
    pub natoms: usize,
}

// --- XDR primitives (big-endian) ---

fn read_i32<R: Read>(r: &mut R) -> io::Result<i32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(i32::from_be_bytes(b))
}

fn read_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn read_f32<R: Read>(r: &mut R) -> io::Result<f32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(f32::from_be_bytes(b))
}

// XDR variable-length opaque: 4-byte count + data + padding to 4-byte boundary
fn read_opaque<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let n = read_u32(r)? as usize;
    let padded = (n + 3) & !3;
    // Grow the buffer from what the file actually yields instead of
    // `vec![0; padded]`: the length is file-supplied and can claim up to 4 GiB,
    // which would be allocated in full before we ever notice the file is short.
    let mut buf = Vec::new();
    let got = r.by_ref().take(padded as u64).read_to_end(&mut buf)?;
    if got != padded {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("truncated XTC payload: expected {padded} bytes, got {got}"),
        ));
    }
    buf.truncate(n);
    Ok(buf)
}

// --- Coordinate decompression (faithful port of xdrfile xdr3dfcoord) ---

/// Smallest number of bits needed to represent `size` distinct values.
fn sizeofint(size: u32) -> i32 {
    let mut num: u32 = 1;
    let mut num_of_bits = 0i32;
    while size >= num && num_of_bits < 32 {
        num_of_bits += 1;
        num = num.wrapping_shl(1);
    }
    num_of_bits
}

/// Number of bits needed to encode three ints with the given per-axis sizes,
/// using the same mixed-radix packing as the encoder (`encodeints`).
fn sizeofints(sizes: &[u32; 3]) -> i32 {
    let mut bytes = [0u32; 32];
    bytes[0] = 1;
    let mut num_of_bytes = 1usize;
    for &size in sizes {
        let mut tmp: u64 = 0;
        let mut bytecnt = 0usize;
        while bytecnt < num_of_bytes {
            tmp += bytes[bytecnt] as u64 * size as u64;
            bytes[bytecnt] = (tmp & 0xff) as u32;
            tmp >>= 8;
            bytecnt += 1;
        }
        while tmp != 0 {
            bytes[bytecnt] = (tmp & 0xff) as u32;
            bytecnt += 1;
            tmp >>= 8;
        }
        num_of_bytes = bytecnt;
    }
    let mut num = 1u32;
    let mut num_of_bits = 0i32;
    num_of_bytes -= 1;
    while bytes[num_of_bytes] >= num {
        num_of_bits += 1;
        num *= 2;
    }
    num_of_bits + num_of_bytes as i32 * 8
}

/// MSB-first bit reader over the compressed byte stream, mirroring xdrfile's
/// `decodebits`/`decodeints` (which keep their state in the first three ints of
/// the working buffer).
struct BitReader<'a> {
    cbuf: &'a [u8],
    cnt: usize,
    lastbits: u32,
    lastbyte: u32,
}

impl<'a> BitReader<'a> {
    fn new(cbuf: &'a [u8]) -> Self {
        Self {
            cbuf,
            cnt: 0,
            lastbits: 0,
            lastbyte: 0,
        }
    }

    fn next_byte(&mut self) -> u32 {
        let b = self.cbuf.get(self.cnt).copied().unwrap_or(0) as u32;
        self.cnt += 1;
        b
    }

    /// Extract `num_of_bits` bits (MSB-first) and return them as an int.
    fn decode_bits(&mut self, num_of_bits: i32) -> i32 {
        let mask: u32 = if num_of_bits >= 32 {
            u32::MAX
        } else {
            (1u32 << num_of_bits) - 1
        };
        let mut nb = num_of_bits;
        let mut num: u32 = 0;
        while nb >= 8 {
            self.lastbyte = (self.lastbyte << 8) | self.next_byte();
            num |= (self.lastbyte >> self.lastbits) << (nb - 8);
            nb -= 8;
        }
        if nb > 0 {
            if self.lastbits < nb as u32 {
                self.lastbits += 8;
                self.lastbyte = (self.lastbyte << 8) | self.next_byte();
            }
            self.lastbits -= nb as u32;
            num |= (self.lastbyte >> self.lastbits) & ((1u32 << nb) - 1);
        }
        num &= mask;
        num as i32
    }

    /// Decode three packed ints (inverse of `encodeints`) using `num_of_bits`
    /// total bits and the given per-axis radices.
    fn decode_ints(&mut self, num_of_bits: i32, sizes: [u32; 3]) -> [i32; 3] {
        let mut bytes = [0i32; 32];
        let mut nbits = num_of_bits;
        let mut num_of_bytes = 0usize;
        // `num_of_bits` originates from the file (via `smallidx`), so one slot
        // per 8 bits can walk off this fixed scratch buffer. Callers validate it,
        // but stop here too rather than trusting the caller's bit count.
        while nbits > 8 && num_of_bytes < bytes.len() {
            bytes[num_of_bytes] = self.decode_bits(8);
            num_of_bytes += 1;
            nbits -= 8;
        }
        if nbits > 0 && num_of_bytes < bytes.len() {
            bytes[num_of_bytes] = self.decode_bits(nbits);
            num_of_bytes += 1;
        }

        let mut nums = [0i32; 3];
        for i in (1..3).rev() {
            let sz = (sizes[i].max(1)) as i64;
            let mut num: i64 = 0;
            for j in (0..num_of_bytes).rev() {
                num = (num << 8) | bytes[j] as i64;
                let p = num / sz;
                bytes[j] = p as i32;
                num -= p * sz;
            }
            nums[i] = num as i32;
        }
        nums[0] = bytes[0] | (bytes[1] << 8) | (bytes[2] << 16) | (bytes[3] << 24);
        nums
    }
}

#[inline]
fn magicint(idx: i32) -> i32 {
    let i = idx.clamp(0, MAGICINTS.len() as i32 - 1) as usize;
    MAGICINTS[i]
}

fn decompress_coords(
    buf: &[u8],
    natoms: usize,
    precision: f32,
    minint: [i32; 3],
    maxint: [i32; 3],
    smallidx0: i32,
) -> io::Result<Vec<[f32; 3]>> {
    // `minint`/`maxint` are raw i32 from the file, so the span must be computed
    // in i64: plain i32 arithmetic panics on overflow in debug builds and wraps
    // into a bogus size in release.
    let mut sizeint = [0u32; 3];
    for (i, size) in sizeint.iter_mut().enumerate() {
        let span = maxint[i] as i64 - minint[i] as i64 + 1;
        if span < 1 || span > u32::MAX as i64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "XTC coordinate bounds out of range on axis {i}: {} .. {}",
                    minint[i], maxint[i]
                ),
            ));
        }
        *size = span as u32;
    }

    // For huge boxes the per-axis sizes can't be multiplied together; the
    // encoder then stores each axis independently (bitsize == 0 flag).
    let mut bitsizeint = [0i32; 3];
    let bitsize = if (sizeint[0] | sizeint[1] | sizeint[2]) > 0x00ff_ffff {
        bitsizeint[0] = sizeofint(sizeint[0]);
        bitsizeint[1] = sizeofint(sizeint[1]);
        bitsizeint[2] = sizeofint(sizeint[2]);
        0
    } else {
        sizeofints(&sizeint)
    };

    let mut smallidx = smallidx0;
    let mut smaller = magicint((smallidx - 1).max(FIRSTIDX)) / 2;
    let mut smallnum = magicint(smallidx) / 2;
    let mut sizesmall = [magicint(smallidx) as u32; 3];

    let inv_precision = 1.0 / precision;
    let mut reader = BitReader::new(buf);
    // `natoms` is file-supplied and must never size an allocation on its own.
    // The payload bounds the real count: every decoded atom consumes at least a
    // flag bit plus its coordinate bits, so the stream can hold at most ~8 atoms
    // per input byte. Bounding the reservation by that (rather than a looser
    // ~1-bit-per-atom estimate, which over-reserved ~12x) keeps a huge `natoms`
    // from requesting tens of GB up front; Vec growth absorbs any minor
    // under-reservation for the densest real frames without panicking.
    let cap = natoms.min(buf.len().saturating_mul(8).saturating_add(12));
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(cap);

    let mut prevcoord: [i32; 3];
    // `run` persists across iterations exactly as in xdrfile: it is only
    // reassigned when the flag bit is set, so a cleared flag keeps the previous
    // run going. Resetting it per iteration desyncs the whole bitstream.
    let mut run = 0i32;
    let mut i = 0usize;
    while i < natoms {
        // The bit reader fabricates zero bytes past the end of the payload, so a
        // truncated frame (or one that over-declares `natoms`) would otherwise
        // spin out millions of junk atoms. Stop once we read past the real data.
        if reader.cnt > reader.cbuf.len() {
            break;
        }
        let mut thiscoord = if bitsize == 0 {
            [
                reader.decode_bits(bitsizeint[0]),
                reader.decode_bits(bitsizeint[1]),
                reader.decode_bits(bitsizeint[2]),
            ]
        } else {
            reader.decode_ints(bitsize, sizeint)
        };
        i += 1;
        // Wrapping: a corrupt `minint` plus a decoded offset can leave the i32
        // range, which must yield garbage coordinates rather than a debug panic.
        thiscoord[0] = thiscoord[0].wrapping_add(minint[0]);
        thiscoord[1] = thiscoord[1].wrapping_add(minint[1]);
        thiscoord[2] = thiscoord[2].wrapping_add(minint[2]);

        prevcoord = thiscoord;

        let flag = reader.decode_bits(1);
        let mut is_smaller = 0i32;
        if flag == 1 {
            run = reader.decode_bits(5);
            is_smaller = run % 3;
            run -= is_smaller;
            is_smaller -= 1;
        }

        if run > 0 {
            // A run of `run / 3` "small" atoms, encoded as deltas from prevcoord.
            let mut k = 0;
            while k < run {
                let mut coord = reader.decode_ints(smallidx, sizesmall);
                // Wrapping for the same reason as the absolute coordinate above.
                coord[0] = coord[0].wrapping_add(prevcoord[0].wrapping_sub(smallnum));
                coord[1] = coord[1].wrapping_add(prevcoord[1].wrapping_sub(smallnum));
                coord[2] = coord[2].wrapping_add(prevcoord[2].wrapping_sub(smallnum));
                i += 1;
                if k == 0 {
                    // Interchange the first two atoms for better water compression.
                    std::mem::swap(&mut coord, &mut prevcoord);
                    positions.push([
                        prevcoord[0] as f32 * inv_precision,
                        prevcoord[1] as f32 * inv_precision,
                        prevcoord[2] as f32 * inv_precision,
                    ]);
                } else {
                    prevcoord = coord;
                }
                positions.push([
                    coord[0] as f32 * inv_precision,
                    coord[1] as f32 * inv_precision,
                    coord[2] as f32 * inv_precision,
                ]);
                k += 3;
            }
        } else {
            positions.push([
                thiscoord[0] as f32 * inv_precision,
                thiscoord[1] as f32 * inv_precision,
                thiscoord[2] as f32 * inv_precision,
            ]);
        }

        // Clamp instead of a bare `+=`: `smallidx` is also handed to decode_ints
        // as a bit count, where an unbounded value walks off its scratch buffer.
        // Well-formed streams never leave this range, so real files are unaffected.
        smallidx = (smallidx + is_smaller).clamp(FIRSTIDX, MAGICINTS.len() as i32 - 1);
        if is_smaller < 0 {
            smallnum = smaller;
            smaller = if smallidx > FIRSTIDX {
                magicint(smallidx - 1) / 2
            } else {
                0
            };
        } else if is_smaller > 0 {
            smaller = smallnum;
            smallnum = magicint(smallidx) / 2;
        }
        sizesmall = [magicint(smallidx) as u32; 3];
    }

    Ok(positions)
}

// --- Frame reader ---

fn read_frame<R: Read>(r: &mut R) -> io::Result<Option<XtcFrame>> {
    let magic = match read_i32(r) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    };
    if magic != XTC_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid XTC magic: {}", magic),
        ));
    }

    let natoms_outer = read_i32(r)?; // outer natoms (also given inside xdr3dfcoord)
    let step = read_i32(r)?;
    let time = read_f32(r)?;

    let mut box_matrix = [[0f32; 3]; 3];
    for row in &mut box_matrix {
        for val in row.iter_mut() {
            *val = read_f32(r)?;
        }
    }

    // --- xdr3dfcoord section ---
    // Both atom counts come straight from the file and must be sanity-checked
    // before either is cast to usize: `-1 as usize` is ~1.8e19, which drives an
    // allocation big enough to abort the process instead of reporting an error.
    let natoms_inner = read_i32(r)?;
    if natoms_outer < 0 || natoms_inner < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("negative XTC atom count ({natoms_outer}/{natoms_inner})"),
        ));
    }
    // The two copies are redundant in a well-formed file; disagreement means the
    // frame is corrupt or we re-synced on a false magic number mid-file.
    if natoms_outer != natoms_inner {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "inconsistent XTC atom count: header says {natoms_outer}, \
                 coordinate block says {natoms_inner}"
            ),
        ));
    }
    let natoms = natoms_inner as usize;

    let mut positions = if natoms <= 9 {
        // Three atoms or fewer: stored as plain floats (no compression).
        let mut pos = Vec::with_capacity(natoms);
        for _ in 0..natoms {
            pos.push([read_f32(r)?, read_f32(r)?, read_f32(r)?]);
        }
        pos
    } else {
        let precision = read_f32(r)?;
        let minint = [read_i32(r)?, read_i32(r)?, read_i32(r)?];
        let maxint = [read_i32(r)?, read_i32(r)?, read_i32(r)?];
        let smallidx = read_i32(r)?;
        // `smallidx` is used verbatim as a bit count by decode_ints and as an
        // index into MAGICINTS, so it has to be inside the table up front.
        if smallidx < FIRSTIDX || smallidx >= MAGICINTS.len() as i32 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("XTC compression index out of range: {smallidx}"),
            ));
        }
        let compressed = read_opaque(r)?;
        decompress_coords(&compressed, natoms, precision, minint, maxint, smallidx)?
    };

    // A short payload stops decompression early; report it rather than handing
    // back a frame that silently disagrees with its own declared atom count.
    if positions.len() < natoms {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "truncated XTC frame: decoded {} of {natoms} atoms",
                positions.len()
            ),
        ));
    }
    // The run-length branch can overshoot the declared count on a corrupt frame.
    positions.truncate(natoms);

    Ok(Some(XtcFrame {
        step,
        time,
        box_matrix,
        positions,
    }))
}

// --- Public API ---

impl XtcFile {
    pub fn load_from_path(path: &Path) -> io::Result<Self> {
        let f = File::open(path)?;
        Self::load_from_reader(f)
    }

    /// Decode every frame from an arbitrary reader. Split out from
    /// [`Self::load_from_path`] so the app can feed a progress-tracking /
    /// cancellable reader (a huge trajectory is read off the UI thread). The
    /// parse is purely sequential, so a plain `File` and a wrapped reader decode
    /// byte-for-byte the same trajectory.
    pub fn load_from_reader<R: Read>(mut reader: R) -> io::Result<Self> {
        let mut frames = Vec::new();
        let mut natoms = 0usize;

        while let Some(frame) = read_frame(&mut reader)? {
            if frames.is_empty() {
                natoms = frame.positions.len();
            } else if frame.positions.len() != natoms {
                // Every consumer (the base molecule, selections, ndx groups)
                // indexes all frames with one atom count; a frame that disagrees
                // would corrupt those maps later on.
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "XTC frame {} has {} atoms, expected {natoms}",
                        frames.len(),
                        frame.positions.len()
                    ),
                ));
            }
            frames.push(frame);
        }

        Ok(Self { frames, natoms })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one XTC frame header up to (and including) the xdr3dfcoord atom
    /// count, then append `tail` verbatim. Only the counts matter to these tests.
    fn frame(natoms_outer: i32, natoms_inner: i32, tail: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&XTC_MAGIC.to_be_bytes());
        b.extend_from_slice(&natoms_outer.to_be_bytes());
        b.extend_from_slice(&1i32.to_be_bytes()); // step
        b.extend_from_slice(&0f32.to_be_bytes()); // time
        for _ in 0..9 {
            b.extend_from_slice(&0f32.to_be_bytes()); // box matrix
        }
        b.extend_from_slice(&natoms_inner.to_be_bytes());
        b.extend_from_slice(tail);
        b
    }

    /// Compressed-branch tail: precision, minint[3], maxint[3], smallidx, payload.
    fn compressed_tail(minint: [i32; 3], maxint: [i32; 3], smallidx: i32, payload: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&1000f32.to_be_bytes());
        for v in minint.iter().chain(maxint.iter()) {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.extend_from_slice(&smallidx.to_be_bytes());
        b.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        b.extend_from_slice(payload);
        let pad = ((payload.len() + 3) & !3) - payload.len();
        b.extend(std::iter::repeat_n(0u8, pad));
        b
    }

    fn err(bytes: &[u8]) -> String {
        // Matched by hand rather than `expect_err`, which would need XtcFrame: Debug.
        match read_frame(&mut &bytes[..]) {
            Ok(_) => panic!("malformed frame must be rejected, not decoded"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn negative_natoms_is_rejected_not_allocated() {
        // -1 as usize is ~1.8e19 and used to reach Vec::with_capacity.
        let tail = compressed_tail([0, 0, 0], [9, 9, 9], FIRSTIDX, &[0u8; 8]);
        assert!(err(&frame(-1, -1, &tail)).contains("negative"));
    }

    #[test]
    fn mismatched_inner_and_outer_natoms_is_rejected() {
        let tail = compressed_tail([0, 0, 0], [9, 9, 9], FIRSTIDX, &[0u8; 8]);
        assert!(err(&frame(20, 40, &tail)).contains("inconsistent"));
    }

    #[test]
    fn huge_natoms_does_not_drive_the_allocation() {
        // 0x7fffffff atoms with an 8-byte payload: must error, not ask for ~25 GB.
        let tail = compressed_tail([0, 0, 0], [9, 9, 9], FIRSTIDX, &[0u8; 8]);
        assert!(err(&frame(i32::MAX, i32::MAX, &tail)).contains("truncated"));
    }

    #[test]
    fn out_of_range_smallidx_is_rejected() {
        // smallidx 300 was used as a bit count and indexed past bytes[32].
        let tail = compressed_tail([0, 0, 0], [9, 9, 9], 300, &[0xffu8; 8]);
        assert!(err(&frame(20, 20, &tail)).contains("compression index"));
        let tail = compressed_tail([0, 0, 0], [9, 9, 9], -1, &[0xffu8; 8]);
        assert!(err(&frame(20, 20, &tail)).contains("compression index"));
    }

    #[test]
    fn overflowing_coordinate_bounds_do_not_panic() {
        // (maxint - minint + 1) overflowed i32 in debug for both of these, yet
        // the true span still fits in u32, so the fix must accept them without
        // panicking (they then fail later as a short payload, never a crash).
        let tail = compressed_tail([-2_000_000_000, 0, 0], [2_000_000_000, 9, 9], FIRSTIDX, &[0u8; 8]);
        assert!(!err(&frame(20, 20, &tail)).is_empty());
        let tail = compressed_tail([0, 0, 0], [i32::MAX, 9, 9], FIRSTIDX, &[0u8; 8]);
        assert!(!err(&frame(20, 20, &tail)).is_empty());
        // The full i32 range yields span == u32::MAX + 1, which is truly rejected.
        let tail = compressed_tail([i32::MIN, 0, 0], [i32::MAX, 9, 9], FIRSTIDX, &[0u8; 8]);
        assert!(err(&frame(20, 20, &tail)).contains("out of range"));
        // maxint < minint gives a non-positive span.
        let tail = compressed_tail([10, 0, 0], [0, 9, 9], FIRSTIDX, &[0u8; 8]);
        assert!(err(&frame(20, 20, &tail)).contains("out of range"));
    }

    #[test]
    fn truncated_opaque_payload_is_rejected() {
        // Declared payload length far beyond what the file holds.
        let mut tail = compressed_tail([0, 0, 0], [9, 9, 9], FIRSTIDX, &[]);
        let len = tail.len();
        tail[len - 4..].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(!err(&frame(20, 20, &tail)).is_empty());
    }

    #[test]
    fn uncompressed_small_frame_still_decodes() {
        // natoms <= 9 takes the plain-float path; behaviour must be unchanged.
        let mut tail = Vec::new();
        for i in 0..9 {
            tail.extend_from_slice(&(i as f32).to_be_bytes());
        }
        let f = read_frame(&mut &frame(3, 3, &tail)[..])
            .expect("well-formed frame")
            .expect("frame present");
        assert_eq!(f.positions.len(), 3);
        assert_eq!(f.positions[1], [3.0, 4.0, 5.0]);
    }

    #[test]
    fn empty_input_ends_the_stream() {
        assert!(read_frame(&mut &[][..]).expect("clean EOF").is_none());
    }

    #[test]
    fn huge_natoms_reservation_tracks_payload_not_count() {
        // Regression: the up-front `Vec::with_capacity` must be bounded by what
        // the payload can decode, never by an inflated `natoms`. A 1 KiB payload
        // with natoms == i32::MAX previously reserved ~96x too much (driving a
        // multi-GB abort on a real ~20 MB frame). The reserved capacity now has
        // to stay within the payload-derived bound, and the decode still errors
        // out gracefully as a short frame rather than crashing.
        let buf = vec![0xFFu8; 1024];
        let natoms = i32::MAX as usize;
        let positions =
            decompress_coords(&buf, natoms, 1000.0, [0, 0, 0], [9, 9, 9], FIRSTIDX)
                .expect("short payload decodes to a partial frame, not an error here");
        // 1 KiB can decode far fewer than the cap, so no growth past it occurs.
        assert!(positions.len() < natoms);
        assert!(positions.capacity() <= buf.len().saturating_mul(8).saturating_add(12));
    }
}
