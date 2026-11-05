//! Shared media construction only; numerical oracles stay independent.
use std::{fs, io::Write, path::Path};

pub fn write_wav(path: &Path, rate: u32, channels: u16, samples: &[i16]) {
    assert_eq!(samples.len() % usize::from(channels), 0);
    let data_bytes = u32::try_from(samples.len() * 2).unwrap();
    let mut file = fs::File::create(path).unwrap();
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + data_bytes).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16_u32.to_le_bytes()).unwrap();
    file.write_all(&1_u16.to_le_bytes()).unwrap();
    file.write_all(&channels.to_le_bytes()).unwrap();
    file.write_all(&rate.to_le_bytes()).unwrap();
    file.write_all(&(rate * u32::from(channels) * 2).to_le_bytes())
        .unwrap();
    file.write_all(&(channels * 2).to_le_bytes()).unwrap();
    file.write_all(&16_u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&data_bytes.to_le_bytes()).unwrap();
    for sample in samples {
        file.write_all(&sample.to_le_bytes()).unwrap();
    }
}
