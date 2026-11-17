//! DSD, the 1-bit format of SACD: reads DSF and DFF (DSDIFF) files and converts them to PCM on the fly.
//!
//! Nothing a phone or a browser ships can play DSD directly, so playback goes through `DsdPcm`: a virtual
//! WAV file whose samples are computed from the DSD bits on demand. It can be read and seeked like a real
//! file, so the usual HTTP range streaming works and no converted copy is ever written to disk.
use crate::scan::Parsed;
use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

pub fn is_dsd(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".dsf") || p.ends_with(".dff")
}

#[derive(Clone, Debug, Default)]
pub struct DsdInfo {
    /// DSD sample rate: 2 822 400 Hz for DSD64, twice that for DSD128, and so on.
    pub rate: u32,
    pub channels: usize,
    data_start: u64,
    bytes_per_ch: u64,
    /// DSF keeps each channel in 4096-byte blocks; DFF interleaves channels byte by byte.
    dsf: bool,
    /// Which bit of a byte is earliest in time.
    lsb_first: bool,
    /// DST is DSD with lossless compression (DFF only). It is indexed but can't be decoded here.
    pub dst: bool,
    id3: Option<(u64, u64)>,
    diin: (Option<String>, Option<String>), // (artist, title) from a DFF's own info chunk
}

impl DsdInfo {
    pub fn duration_ms(&self) -> i64 {
        if self.rate == 0 { 0 } else { (self.bytes_per_ch as u128 * 8000 / self.rate as u128) as i64 }
    }
}

fn le32(b: &[u8]) -> u32 { u32::from_le_bytes(b[..4].try_into().unwrap()) }
fn le64(b: &[u8]) -> u64 { u64::from_le_bytes(b[..8].try_into().unwrap()) }
fn be32(b: &[u8]) -> u32 { u32::from_be_bytes(b[..4].try_into().unwrap()) }
fn be64(b: &[u8]) -> u64 { u64::from_be_bytes(b[..8].try_into().unwrap()) }

pub fn probe(path: &str) -> Result<DsdInfo> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut head = [0u8; 16];
    f.read_exact(&mut head)?;
    if &head[..4] == b"DSD " {
        // DSF: "DSD " chunk (28 bytes), "fmt " chunk, "data" chunk, optional ID3v2 tag at the end.
        let mut h = [0u8; 92];
        f.seek(SeekFrom::Start(0))?;
        f.read_exact(&mut h)?;
        if &h[28..32] != b"fmt " {
            bail!("DSF file has no fmt chunk");
        }
        let (meta, fmt_size) = (le64(&h[20..]), le64(&h[32..]));
        let (channels, rate, bits, samples) = (le32(&h[52..]) as usize, le32(&h[56..]), le32(&h[60..]), le64(&h[64..]));
        if channels == 0 || rate == 0 {
            bail!("DSF file has an empty format");
        }
        Ok(DsdInfo {
            rate, channels, dsf: true, lsb_first: bits == 1, dst: false,
            data_start: 28 + fmt_size + 12,
            bytes_per_ch: samples.div_ceil(8),
            id3: (meta > 0 && meta < len).then(|| (meta, len - meta)),
            diin: (None, None),
        })
    } else if &head[..4] == b"FRM8" && &head[12..16] == b"DSD " {
        // DFF: big-endian chunks, each an id, a 64-bit size and a body padded to an even length.
        let mut info = DsdInfo::default();
        let (mut pos, mut data_len) = (16u64, 0u64);
        let mut ck = [0u8; 12];
        while pos + 12 <= len {
            f.seek(SeekFrom::Start(pos))?;
            f.read_exact(&mut ck)?;
            let (id, size) = (&ck[..4], be64(&ck[4..]));
            let body = pos + 12;
            match id {
                b"PROP" | b"DIIN" => {
                    let mut b = vec![0u8; size.min(1 << 20) as usize];
                    f.read_exact(&mut b)?;
                    let mut i = if id == b"PROP" { 4 } else { 0 }; // PROP starts with "SND "
                    while i + 12 <= b.len() {
                        let n = be64(&b[i + 4..]) as usize;
                        let v = &b[(i + 12).min(b.len())..(i + 12 + n).min(b.len())];
                        match &b[i..i + 4] {
                            b"FS  " if v.len() >= 4 => info.rate = be32(v),
                            b"CHNL" if v.len() >= 2 => info.channels = u16::from_be_bytes([v[0], v[1]]) as usize,
                            b"CMPR" if v.len() >= 4 => info.dst = &v[..4] == b"DST ",
                            b"DIAR" if v.len() > 4 => info.diin.0 = Some(String::from_utf8_lossy(&v[4..]).trim().to_string()),
                            b"DITI" if v.len() > 4 => info.diin.1 = Some(String::from_utf8_lossy(&v[4..]).trim().to_string()),
                            _ => {}
                        }
                        i += 12 + n + (n & 1);
                    }
                }
                b"DSD " => (info.data_start, data_len) = (body, size),
                b"DST " => info.dst = true,
                b"ID3 " => info.id3 = Some((body, size)),
                _ => {}
            }
            pos = body + size + (size & 1);
        }
        if info.rate == 0 || info.channels == 0 {
            bail!("DFF file has no sample rate or channel count");
        }
        info.bytes_per_ch = data_len / info.channels as u64;
        Ok(info)
    } else {
        bail!("Not a DSF or DFF file")
    }
}

/* ---------- Tags: DSF and many DFF files carry an ID3v2 tag ---------- */

#[derive(Default)]
struct Id3 {
    text: HashMap<[u8; 4], String>,
    picture: Option<(Vec<u8>, bool)>, // (image bytes, is PNG)
}

fn decode(enc: u8, b: &[u8]) -> String {
    let s = match enc {
        0 => b.iter().map(|&c| c as char).collect(),
        1 | 2 => {
            let (be, body) = match (enc, b) {
                (1, [0xFE, 0xFF, r @ ..]) => (true, r),
                (1, [0xFF, 0xFE, r @ ..]) => (false, r),
                (2, _) => (true, b),
                _ => (false, b),
            };
            let u: Vec<u16> = body.chunks_exact(2).map(|p| if be { u16::from_be_bytes([p[0], p[1]]) } else { u16::from_le_bytes([p[0], p[1]]) }).collect();
            String::from_utf16_lossy(&u)
        }
        _ => String::from_utf8_lossy(b).into_owned(),
    };
    s.split('\0').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("; ")
}

fn read_id3(path: &str, at: (u64, u64)) -> Option<Id3> {
    let mut f = File::open(path).ok()?;
    let mut b = vec![0u8; at.1.min(24 << 20) as usize];
    f.seek(SeekFrom::Start(at.0)).ok()?;
    f.read_exact(&mut b).ok()?;
    if b.len() < 10 || &b[..3] != b"ID3" || b[3] < 3 {
        return None; // ID3v2.2 and older are too rare in DSD files to carry a second parser for
    }
    let syncsafe = |s: &[u8]| s[..4].iter().fold(0usize, |n, &x| n << 7 | (x & 0x7F) as usize);
    let (v4, end) = (b[3] >= 4, (10 + syncsafe(&b[6..])).min(b.len()));
    let mut i = 10;
    if b[5] & 0x40 != 0 && i + 4 <= end {
        i += if v4 { syncsafe(&b[i..]) } else { 4 + be32(&b[i..]) as usize }; // skip the extended header
    }
    let mut out = Id3::default();
    while i + 10 <= end && b[i] != 0 {
        let id: [u8; 4] = b[i..i + 4].try_into().ok()?;
        let n = if v4 { syncsafe(&b[i + 4..]) } else { be32(&b[i + 4..]) as usize };
        let body = b.get(i + 10..i + 10 + n)?;
        i += 10 + n;
        if body.is_empty() {
            continue;
        }
        if id[0] == b'T' && &id != b"TXXX" {
            out.text.insert(id, decode(body[0], &body[1..]));
        } else if &id == b"APIC" && out.picture.is_none() {
            // encoding, MIME (zero-terminated), picture type, description (zero-terminated), image
            let wide = body[0] == 1 || body[0] == 2;
            let Some(m) = body[1..].iter().position(|&c| c == 0) else { continue };
            let mut d = 1 + m + 2; // past MIME, its terminator and the type byte
            while d < body.len() {
                if !wide && body[d] == 0 { d += 1; break; }
                if wide && d + 1 < body.len() && body[d] == 0 && body[d + 1] == 0 { d += 2; break; }
                d += if wide { 2 } else { 1 };
            }
            if let Some(img) = body.get(d..).filter(|x| x.len() > 8) {
                out.picture = Some((img.to_vec(), img.starts_with(&[0x89, b'P', b'N', b'G'])));
            }
        }
    }
    Some(out)
}

/// Embedded artwork of a DSD file, if its ID3 tag has any.
pub fn cover(path: &str) -> Option<(Vec<u8>, bool)> {
    read_id3(path, probe(path).ok()?.id3?)?.picture
}

/// Index entry for a DSF/DFF file.
pub(crate) fn parse(path: &Path, mtime: i64, size: i64) -> Result<Parsed> {
    let p = path.to_string_lossy().into_owned();
    let info = probe(&p)?;
    let id3 = info.id3.and_then(|at| read_id3(&p, at)).unwrap_or_default();
    let tag = |k: &[u8; 4]| id3.text.get(k).cloned().filter(|s| !s.is_empty());
    let number = |k: &[u8; 4]| tag(k).and_then(|s| s.split(['/', ';']).next().and_then(|n| n.trim().parse::<i64>().ok()));
    let stem = || path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let parent = || path.parent().and_then(Path::file_name).map(|s| s.to_string_lossy().into_owned());
    let artist = tag(b"TPE1").or(info.diin.0.clone()).unwrap_or_else(|| "Unknown Artist".into());
    Ok(Parsed {
        path: p, mtime, size,
        title: tag(b"TIT2").or(info.diin.1.clone()).unwrap_or_else(stem),
        album_artist: tag(b"TPE2").unwrap_or_else(|| artist.clone()),
        album: tag(b"TALB").or_else(parent).unwrap_or_else(|| "Unknown Album".into()),
        artist,
        genres: tag(b"TCON").map(|g| g.split(';').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default(),
        track_no: number(b"TRCK").unwrap_or(0),
        disc_no: number(b"TPOS").unwrap_or(1),
        year: tag(b"TDRC").or(tag(b"TYER")).and_then(|y| y.get(..4).and_then(|y| y.parse().ok())).filter(|y| *y > 0),
        duration_ms: info.duration_ms(),
        codec: if info.dst { "DST" } else { "DSD" },
        sample_rate: info.rate as i64,
        bit_depth: 1,
        bitrate: (info.rate as i64 * info.channels as i64) / 1000,
        channels: info.channels as i64,
        tier: 3,
    })
}

/* ---------- DSD -> PCM ---------- */

/// Low-pass filter for one decimation ratio, stored as lookup tables: one table per input byte position,
/// so eight 1-bit samples are filtered with a single lookup.
struct Fir {
    tables: Vec<[f32; 256]>,
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)) * (x / (2.0 * k));
        sum += term;
        k += 1.0;
    }
    sum
}

fn fir(decim: usize) -> Arc<Fir> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Arc<Fir>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    cache.entry(decim).or_insert_with(|| {
        // Kaiser-windowed sinc, about 90 dB stopband. Passband to ~30 kHz, fully down by the output's
        // Nyquist frequency, which also strips DSD's ultrasonic shaping noise.
        let n = 40 * decim;
        let (fc, beta, mid) = (0.4116 / decim as f64, 8.96, (n as f64 - 1.0) / 2.0);
        let mut h: Vec<f64> = (0..n).map(|i| {
            let (x, r) = (i as f64 - mid, (i as f64 - mid) / mid);
            let sinc = if x == 0.0 { 1.0 } else { (2.0 * std::f64::consts::PI * fc * x).sin() / (2.0 * std::f64::consts::PI * fc * x) };
            2.0 * fc * sinc * bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta)
        }).collect();
        // Unity gain, then +6 dB: SACD's 0 dB reference is half of DSD full scale.
        let gain = 2.0 / h.iter().sum::<f64>();
        h.iter_mut().for_each(|v| *v *= gain);
        let tables = h.chunks_exact(8).map(|taps| {
            let mut t = [0f32; 256];
            for (v, slot) in t.iter_mut().enumerate() {
                *slot = taps.iter().enumerate().map(|(j, c)| if v >> j & 1 == 1 { *c } else { -*c }).sum::<f64>() as f32;
            }
            t
        }).collect();
        Arc::new(Fir { tables })
    }).clone()
}

/// A DSD file seen as a 24-bit PCM WAV at 88.2 kHz (96 kHz for the 48 kHz DSD family).
pub struct DsdPcm {
    f: File,
    info: DsdInfo,
    fir: Arc<Fir>,
    step: usize, // DSD bytes consumed per output frame
    pub out_rate: u32,
    pub frames: u64,
    pub ch: usize, // output channels: stereo, or the front pair of a multichannel file
    pos: u64,
    cache: (u64, Vec<u8>), // (first frame, PCM bytes)
}

const BLOCK: u64 = 4096;
const SILENCE: u8 = 0x69; // the DSD idle pattern: equal ones and zeros

impl DsdPcm {
    pub fn open(path: &str) -> Result<Self> {
        let info = probe(path)?;
        if info.dst {
            bail!("This file is DST-compressed DSD, which Sleeve can't decode yet");
        }
        let decim = ((info.rate / 88_200) as usize / 8 * 8).max(8);
        let step = decim / 8;
        Ok(Self {
            f: File::open(path)?, fir: fir(decim), step,
            out_rate: info.rate / decim as u32,
            frames: info.bytes_per_ch / step as u64,
            ch: info.channels.min(2),
            info, pos: 0, cache: (0, Vec::new()),
        })
    }

    pub fn len(&self) -> u64 {
        44 + self.frames * self.ch as u64 * 3
    }

    fn header(&self) -> [u8; 44] {
        let (data, align) = ((self.len() - 44) as u32, self.ch as u16 * 3);
        let mut h = [0u8; 44];
        h[..4].copy_from_slice(b"RIFF");
        h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
        h[8..16].copy_from_slice(b"WAVEfmt ");
        h[16..20].copy_from_slice(&16u32.to_le_bytes());
        h[20..22].copy_from_slice(&1u16.to_le_bytes());
        h[22..24].copy_from_slice(&(self.ch as u16).to_le_bytes());
        h[24..28].copy_from_slice(&self.out_rate.to_le_bytes());
        h[28..32].copy_from_slice(&(self.out_rate * align as u32).to_le_bytes());
        h[32..34].copy_from_slice(&align.to_le_bytes());
        h[34..36].copy_from_slice(&24u16.to_le_bytes());
        h[36..40].copy_from_slice(b"data");
        h[40..44].copy_from_slice(&data.to_le_bytes());
        h
    }

    /// DSD bytes `start .. start+len` of each output channel, earliest bit in bit 0.
    /// Anything outside the file reads as digital silence.
    fn channels(&mut self, start: i64, len: usize) -> io::Result<Vec<Vec<u8>>> {
        let mut out = vec![vec![SILENCE; len]; self.ch];
        let (a, b) = (start.max(0) as u64, ((start + len as i64).max(0) as u64).min(self.info.bytes_per_ch));
        if a >= b {
            return Ok(out);
        }
        let (nch, base) = (self.info.channels as u64, (a as i64 - start) as usize);
        if self.info.dsf {
            let mut group = vec![0u8; (BLOCK * nch) as usize];
            for blk in a / BLOCK..=(b - 1) / BLOCK {
                self.f.seek(SeekFrom::Start(self.info.data_start + blk * BLOCK * nch))?;
                let mut got = 0;
                while got < group.len() {
                    match self.f.read(&mut group[got..])? { 0 => break, n => got += n }
                }
                let (lo, hi) = ((blk * BLOCK).max(a), ((blk + 1) * BLOCK).min(b));
                for (c, ch) in out.iter_mut().enumerate() {
                    let src = c * BLOCK as usize + (lo - blk * BLOCK) as usize;
                    let n = ((hi - lo) as usize).min(got.saturating_sub(src));
                    ch[base + (lo - a) as usize..][..n].copy_from_slice(&group[src..src + n]);
                }
            }
        } else {
            let mut raw = vec![0u8; ((b - a) * nch) as usize];
            self.f.seek(SeekFrom::Start(self.info.data_start + a * nch))?;
            let mut got = 0;
            while got < raw.len() {
                match self.f.read(&mut raw[got..])? { 0 => break, n => got += n }
            }
            for (i, frame) in raw[..got - got % nch as usize].chunks_exact(nch as usize).enumerate() {
                for (c, ch) in out.iter_mut().enumerate() {
                    ch[base + i] = frame[c];
                }
            }
        }
        if !self.info.lsb_first {
            out.iter_mut().flatten().for_each(|v| *v = v.reverse_bits());
        }
        Ok(out)
    }

    /// `count` output frames starting at frame `first`, taking every `every`-th frame. Interleaved, -1..1.
    pub fn render(&mut self, first: u64, count: usize, every: usize) -> io::Result<Vec<f32>> {
        let (k, hop) = (self.fir.tables.len(), self.step * every);
        let start = (first * self.step as u64) as i64 - (k / 2) as i64;
        let bytes = self.channels(start, count.saturating_sub(1) * hop + k)?;
        let fir = self.fir.clone();
        let mut out = vec![0f32; count * self.ch];
        for i in 0..count {
            for (c, ch) in bytes.iter().enumerate() {
                let window = &ch[i * hop..i * hop + k];
                out[i * self.ch + c] = fir.tables.iter().zip(window).map(|(t, v)| t[*v as usize] as f64).sum::<f64>() as f32;
            }
        }
        Ok(out)
    }
}

impl Read for DsdPcm {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.len() || out.is_empty() {
            return Ok(0);
        }
        if self.pos < 44 {
            let h = self.header();
            let n = out.len().min(44 - self.pos as usize);
            out[..n].copy_from_slice(&h[self.pos as usize..][..n]);
            self.pos += n as u64;
            return Ok(n);
        }
        let width = self.ch as u64 * 3;
        let (off, frame) = (self.pos - 44, (self.pos - 44) / width);
        if frame < self.cache.0 || frame >= self.cache.0 + self.cache.1.len() as u64 / width {
            let count = (self.frames - frame).min(4096) as usize;
            let pcm = self.render(frame, count, 1)?;
            let mut b = Vec::with_capacity(pcm.len() * 3);
            for s in pcm {
                b.extend_from_slice(&((s.clamp(-1.0, 1.0) as f64 * 8_388_607.0) as i32).to_le_bytes()[..3]);
            }
            self.cache = (frame, b);
        }
        let at = (off - self.cache.0 * width) as usize;
        let n = out.len().min(self.cache.1.len() - at);
        out[..n].copy_from_slice(&self.cache.1[at..at + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for DsdPcm {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::Current(d) => self.pos as i64 + d,
            SeekFrom::End(d) => self.len() as i64 + d,
        };
        self.pos = p.max(0) as u64;
        Ok(self.pos)
    }
}

pub fn open_pcm(path: &str) -> Result<DsdPcm> {
    DsdPcm::open(path).map_err(|e| anyhow!("{e}"))
}
