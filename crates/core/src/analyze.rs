//! Decodes a whole track once to get what tags can't tell you: the waveform envelope,
//! sample peak, RMS level, clipped samples and a dynamic-range figure.
use anyhow::{anyhow, Result};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub struct Analysis {
    /// Envelope: `buckets` peak values, 0-255, spread evenly over the track.
    pub peaks: Vec<u8>,
    pub peak_db: f64,
    pub rms_db: f64,
    /// Dynamic range after the published TT DR method (3 s blocks, loudest 20 %, second-highest peak).
    pub dr: Option<i64>,
    pub clipped: i64,
}

fn db(x: f64) -> f64 {
    if x > 0.0 { 20.0 * x.log10() } else { -120.0 }
}

/// Running measurements over interleaved samples, whatever decoded them.
struct Meter {
    ch: usize,
    slice: usize, // frames per 50 ms envelope slice
    block: usize, // frames per 3 s dynamic-range block
    env: Vec<f32>,
    slice_pk: f32,
    slice_n: usize,
    block_n: usize,
    peak: f32,
    sq: f64,
    samples: u64,
    clipped: i64,
    ch_sq: Vec<f64>,
    ch_pk: Vec<f32>,
    blocks: Vec<Vec<(f64, f32)>>, // per channel: (block RMS, block peak)
}

impl Meter {
    fn new(rate: usize, ch: usize) -> Self {
        Self { ch, slice: (rate / 20).max(1), block: rate * 3, env: Vec::new(), slice_pk: 0.0, slice_n: 0, block_n: 0, peak: 0.0, sq: 0.0,
               samples: 0, clipped: 0, ch_sq: vec![0.0; ch], ch_pk: vec![0.0; ch], blocks: vec![Vec::new(); ch] }
    }

    fn push(&mut self, interleaved: &[f32]) {
        for frame in interleaved.chunks_exact(self.ch) {
            for (c, &x) in frame.iter().enumerate() {
                let a = x.abs();
                self.slice_pk = self.slice_pk.max(a);
                self.ch_pk[c] = self.ch_pk[c].max(a);
                self.clipped += (a >= 0.9999) as i64;
                let x2 = x as f64 * x as f64;
                self.sq += x2;
                self.ch_sq[c] += x2;
            }
            self.samples += self.ch as u64;
            self.slice_n += 1;
            if self.slice_n == self.slice {
                self.env.push(self.slice_pk);
                self.peak = self.peak.max(self.slice_pk);
                (self.slice_pk, self.slice_n) = (0.0, 0);
            }
            self.block_n += 1;
            if self.block_n == self.block {
                for c in 0..self.ch {
                    self.blocks[c].push(((2.0 * self.ch_sq[c] / self.block as f64).sqrt(), self.ch_pk[c]));
                    (self.ch_sq[c], self.ch_pk[c]) = (0.0, 0.0);
                }
                self.block_n = 0;
            }
        }
    }

    fn finish(mut self, buckets: usize) -> Result<Analysis> {
        if self.slice_n > 0 {
            self.env.push(self.slice_pk);
            self.peak = self.peak.max(self.slice_pk);
        }
        if self.samples == 0 {
            return Err(anyhow!("No audio could be decoded from this file"));
        }
        let env = &self.env;
        let peaks = (0..buckets)
            .map(|b| {
                let (a, z) = (b * env.len() / buckets, ((b + 1) * env.len() / buckets).max(b * env.len() / buckets + 1));
                let v = env[a.min(env.len() - 1)..z.min(env.len())].iter().fold(0f32, |m, v| m.max(*v));
                (v.min(1.0) * 255.0).round() as u8
            })
            .collect();
        let per_channel: Vec<f64> = self.blocks
            .iter_mut()
            .filter(|b| b.len() >= 2)
            .filter_map(|b| {
                let mut pk: Vec<f32> = b.iter().map(|x| x.1).collect();
                pk.sort_by(|a, b| b.total_cmp(a));
                b.sort_by(|a, b| b.0.total_cmp(&a.0));
                let n = (b.len() / 5).max(1);
                let loud = (b[..n].iter().map(|x| x.0 * x.0).sum::<f64>() / n as f64).sqrt();
                (loud > 0.0 && pk[1] > 0.0).then(|| db(pk[1] as f64 / loud))
            })
            .collect();
        let dr = (!per_channel.is_empty()).then(|| (per_channel.iter().sum::<f64>() / per_channel.len() as f64).round() as i64);
        Ok(Analysis { peaks, peak_db: db(self.peak as f64), rms_db: db((self.sq / self.samples as f64).sqrt()), dr, clipped: self.clipped })
    }
}

pub fn analyze(path: &str, buckets: usize) -> Result<Analysis> {
    if crate::dsd::is_dsd(path) {
        // DSD is measured on its PCM conversion. Every 4th frame is enough for levels and keeps this quick.
        let mut pcm = crate::dsd::open_pcm(path)?;
        let mut meter = Meter::new(pcm.out_rate as usize / 4, pcm.ch);
        let mut at = 0u64;
        while at < pcm.frames {
            let count = ((pcm.frames - at) / 4).clamp(1, 4096) as usize;
            meter.push(&pcm.render(at, count, 4)?);
            at += count as u64 * 4;
        }
        return meter.finish(buckets);
    }
    let mss = MediaSourceStream::new(Box::new(std::fs::File::open(path)?), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| anyhow!("No decoder for this file: {e}"))?;
    let mut format = probed.format;
    let track = format.default_track().ok_or_else(|| anyhow!("No audio track in file"))?;
    let (tid, params) = (track.id, track.codec_params.clone());
    let mut dec = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| anyhow!("No decoder for this codec: {e}"))?;
    let rate = params.sample_rate.unwrap_or(44_100) as usize;
    let mut meter: Option<Meter> = None;
    let mut buf: Option<(SampleBuffer<f32>, usize)> = None;
    loop {
        let Ok(packet) = format.next_packet() else { break };
        if packet.track_id() != tid {
            continue;
        }
        let decoded = match dec.decode(&packet) {
            Ok(d) => d,
            Err(Error::DecodeError(_)) => continue, // skip a damaged frame, keep going
            Err(_) => break,
        };
        let ch = decoded.spec().channels.count();
        if buf.as_ref().map_or(true, |b| b.1 < decoded.capacity()) {
            buf = Some((SampleBuffer::new(decoded.capacity() as u64, *decoded.spec()), decoded.capacity()));
        }
        let sb = &mut buf.as_mut().unwrap().0;
        sb.copy_interleaved_ref(decoded);
        if meter.as_ref().map_or(true, |m| m.ch != ch) {
            meter = Some(Meter::new(rate, ch));
        }
        meter.as_mut().unwrap().push(sb.samples());
    }
    meter.ok_or_else(|| anyhow!("No audio could be decoded from this file"))?.finish(buckets)
}
