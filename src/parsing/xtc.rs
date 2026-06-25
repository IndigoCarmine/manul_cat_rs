use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

// GROMACS XTC magic number
const XTC_MAGIC: i32 = 1995;

// Small-coordinate encoding table (GROMACS magic numbers)
const SMALL_MAGIC: [u32; 22] = [
    1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072,
    262144, 524288, 1048576, 2097152,
];

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
    let mut buf = vec![0u8; padded];
    r.read_exact(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

// --- Bit-level decoding (LSB-first within each byte) ---

// Extract `num` bits from `buf` at bit offset `*cnt`; advance `*cnt`.
fn decode_bits(buf: &[u8], cnt: &mut usize, num: usize) -> u64 {
    if num == 0 {
        return 0;
    }
    let byte_start = *cnt >> 3;
    let bit_off = *cnt & 7;
    *cnt += num;

    // bit_off (up to 7) + num (up to 64) can span 9 bytes; use u128 to avoid shift overflow.
    let bytes_needed = (bit_off + num + 7) >> 3;
    let mut val = 0u128;
    for i in 0..bytes_needed.min(9) {
        let b = *buf.get(byte_start + i).unwrap_or(&0) as u128;
        val |= b << (i * 8);
    }
    let mask = if num >= 64 { u64::MAX } else { (1u64 << num) - 1 };
    ((val >> bit_off) as u64) & mask
}

// Decode 3 integers packed as mixed-radix in `buf`:
//   packed = x + sizes[0] * (y + sizes[1] * z)
// Requires ceil(log2(sizes[0]*sizes[1]*sizes[2])) bits.
fn decode_ints(buf: &[u8], cnt: &mut usize, sizes: [u32; 3]) -> [i32; 3] {
    let total = sizes[0] as u64 * sizes[1] as u64 * sizes[2] as u64;
    let nbits = if total <= 1 {
        1
    } else {
        64 - (total - 1).leading_zeros() as usize
    };

    let mut packed = decode_bits(buf, cnt, nbits);
    let x = (packed % sizes[0] as u64) as i32;
    packed /= sizes[0] as u64;
    let y = (packed % sizes[1] as u64) as i32;
    packed /= sizes[1] as u64;
    let z = packed as i32;
    [x, y, z]
}

// --- 3DPC decompression ---

fn decompress_coords(
    buf: &[u8],
    natoms: usize,
    precision: f32,
    minint: [i32; 3],
    maxint: [i32; 3],
    smallidx: usize,
) -> Vec<[f32; 3]> {
    let sizeint = [
        (maxint[0] - minint[0] + 1).max(1) as u32,
        (maxint[1] - minint[1] + 1).max(1) as u32,
        (maxint[2] - minint[2] + 1).max(1) as u32,
    ];

    let max_idx = SMALL_MAGIC.len() - 1;
    let mut sidx = smallidx.min(max_idx);
    let mut sizesmall = [SMALL_MAGIC[sidx]; 3];
    let mut small_num = (SMALL_MAGIC[sidx] / 2) as i32;

    let mut cnt = 0usize;
    let mut prev = [0i32; 3];
    let mut positions = Vec::with_capacity(natoms);

    for _ in 0..natoms {
        // Bit layout per atom (GROMACS xdrfile format):
        //   1 bit : is_small
        //   N bits: coordinate (small or large encoding)
        //   1 bit : is_smaller  ← comes AFTER the coordinate, not before
        let is_small = decode_bits(buf, &mut cnt, 1) != 0;

        let coord = if is_small {
            let d = decode_ints(buf, &mut cnt, sizesmall);
            [
                d[0] + prev[0] - small_num,
                d[1] + prev[1] - small_num,
                d[2] + prev[2] - small_num,
            ]
        } else {
            let c = decode_ints(buf, &mut cnt, sizeint);
            [c[0] + minint[0], c[1] + minint[1], c[2] + minint[2]]
        };

        // is_smaller comes after the coordinate data
        let is_smaller = decode_bits(buf, &mut cnt, 1) != 0;

        // Adjust small encoding range for next atom
        if is_small && is_smaller && sidx > 0 {
            sidx -= 1;
            sizesmall = [SMALL_MAGIC[sidx]; 3];
            small_num = (SMALL_MAGIC[sidx] / 2) as i32;
        } else if !is_small && is_smaller && sidx < max_idx {
            sidx += 1;
            sizesmall = [SMALL_MAGIC[sidx]; 3];
            small_num = (SMALL_MAGIC[sidx] / 2) as i32;
        }

        prev = coord;
        positions.push([
            coord[0] as f32 / precision,
            coord[1] as f32 / precision,
            coord[2] as f32 / precision,
        ]);
    }

    positions
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

    let _natoms_outer = read_i32(r)?; // outer natoms (confirmed by xdr3dfcoord)
    let step = read_i32(r)?;
    let time = read_f32(r)?;

    let mut box_matrix = [[0f32; 3]; 3];
    for row in &mut box_matrix {
        for val in row.iter_mut() {
            *val = read_f32(r)?;
        }
    }

    // --- xdr3dfcoord section ---
    let natoms = read_i32(r)? as usize;

    let positions = if natoms <= 9 {
        // Small molecule: uncompressed floats (no precision field)
        let mut pos = Vec::with_capacity(natoms);
        for _ in 0..natoms {
            pos.push([read_f32(r)?, read_f32(r)?, read_f32(r)?]);
        }
        pos
    } else {
        let precision = read_f32(r)?;
        let minint = [read_i32(r)?, read_i32(r)?, read_i32(r)?];
        let maxint = [read_i32(r)?, read_i32(r)?, read_i32(r)?];
        let smallidx = read_i32(r)? as usize;
        let compressed = read_opaque(r)?;
        decompress_coords(&compressed, natoms, precision, minint, maxint, smallidx)
    };

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
        let mut f = File::open(path)?;
        let mut frames = Vec::new();
        let mut natoms = 0usize;

        loop {
            match read_frame(&mut f)? {
                Some(frame) => {
                    if natoms == 0 {
                        natoms = frame.positions.len();
                    }
                    frames.push(frame);
                }
                None => break,
            }
        }

        Ok(Self { frames, natoms })
    }
}
