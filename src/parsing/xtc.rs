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
    let mut buf = vec![0u8; padded];
    r.read_exact(&mut buf)?;
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
            tmp = bytes[bytecnt] as u64 * size as u64 + tmp;
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
        while nbits > 8 {
            bytes[num_of_bytes] = self.decode_bits(8);
            num_of_bytes += 1;
            nbits -= 8;
        }
        if nbits > 0 {
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
) -> Vec<[f32; 3]> {
    let sizeint = [
        (maxint[0] - minint[0] + 1) as u32,
        (maxint[1] - minint[1] + 1) as u32,
        (maxint[2] - minint[2] + 1) as u32,
    ];

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
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(natoms);

    let mut prevcoord = [0i32; 3];
    // `run` persists across iterations exactly as in xdrfile: it is only
    // reassigned when the flag bit is set, so a cleared flag keeps the previous
    // run going. Resetting it per iteration desyncs the whole bitstream.
    let mut run = 0i32;
    let mut i = 0usize;
    while i < natoms {
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
        thiscoord[0] += minint[0];
        thiscoord[1] += minint[1];
        thiscoord[2] += minint[2];

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
                coord[0] += prevcoord[0] - smallnum;
                coord[1] += prevcoord[1] - smallnum;
                coord[2] += prevcoord[2] - smallnum;
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

        smallidx += is_smaller;
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

    let _natoms_outer = read_i32(r)?; // outer natoms (also given inside xdr3dfcoord)
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
