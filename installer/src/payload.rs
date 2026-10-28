//! Archivos de la app comprimidos y anexados al final del ejecutable.
//!
//! Formato: `[stub .exe][entradas][offset u64][MAGIC]`. Cada entrada es
//! `[len nombre u16][nombre][tamaño original u64][tamaño comprimido u64][deflate]`.
//! Windows ignora los bytes que siguen a la imagen PE, así que el stub sigue
//! siendo un ejecutable válido, y sin el payload sirve como desinstalador.

use std::{
    fs::File,
    io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use flate2::{Compression, read::DeflateDecoder, write::DeflateEncoder};

const MAGIC: &[u8; 8] = b"NFSETUP1";
const FOOTER_LEN: u64 = 16;

pub struct Entry {
    pub name: String,
    pub size: u64,
    offset: u64,
    compressed: u64,
}

pub struct Payload {
    exe: PathBuf,
    pub entries: Vec<Entry>,
}

impl Payload {
    /// Lee el índice del payload del ejecutable actual, si lo tiene.
    pub fn open_current() -> io::Result<Option<Self>> {
        let exe = std::env::current_exe()?;
        let mut file = File::open(&exe)?;
        let Some(start) = payload_start(&mut file)? else {
            return Ok(None);
        };

        let end = file.seek(SeekFrom::End(0))? - FOOTER_LEN;
        file.seek(SeekFrom::Start(start))?;
        let mut reader = BufReader::new(file);
        let mut position = start;
        let mut entries = Vec::new();
        while position < end {
            let name_len = u64::from(read_u16(&mut reader)?);
            let mut name = vec![0; name_len as usize];
            reader.read_exact(&mut name)?;
            let size = read_u64(&mut reader)?;
            let compressed = read_u64(&mut reader)?;
            let offset = position + 2 + name_len + 16;
            reader.seek_relative(compressed as i64)?;
            position = offset + compressed;
            entries.push(Entry {
                name: String::from_utf8(name).map_err(io::Error::other)?,
                size,
                offset,
                compressed,
            });
        }
        Ok(Some(Self { exe, entries }))
    }

    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|entry| entry.size).sum()
    }

    /// Descomprime una entrada en `dest`, informando los bytes escritos.
    pub fn extract(
        &self,
        entry: &Entry,
        dest: &Path,
        mut on_progress: impl FnMut(u64),
    ) -> io::Result<()> {
        let mut file = File::open(&self.exe)?;
        file.seek(SeekFrom::Start(entry.offset))?;
        let mut decoder = DeflateDecoder::new(BufReader::new(file).take(entry.compressed));
        let mut out = BufWriter::new(File::create(dest)?);
        let mut buffer = vec![0; 1 << 20];
        loop {
            let read = decoder.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            out.write_all(&buffer[..read])?;
            on_progress(read as u64);
        }
        out.flush()
    }
}

/// Bytes del ejecutable actual sin el payload: el desinstalador.
pub fn stub_bytes() -> io::Result<Vec<u8>> {
    let mut file = File::open(std::env::current_exe()?)?;
    let len = payload_start(&mut file)?.unwrap_or(file.seek(SeekFrom::End(0))?);
    let mut bytes = vec![0; len as usize];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

/// Genera el instalador final: stub + archivos comprimidos.
pub fn pack(output: &Path, files: &[PathBuf]) -> io::Result<()> {
    let stub = stub_bytes()?;
    let mut out = BufWriter::new(File::create(output)?);
    out.write_all(&stub)?;
    for path in files {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| io::Error::other("nombre de archivo inválido"))?;
        let raw = std::fs::read(path)?;
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&raw)?;
        let compressed = encoder.finish()?;
        out.write_all(&(name.len() as u16).to_le_bytes())?;
        out.write_all(name.as_bytes())?;
        out.write_all(&(raw.len() as u64).to_le_bytes())?;
        out.write_all(&(compressed.len() as u64).to_le_bytes())?;
        out.write_all(&compressed)?;
    }
    out.write_all(&(stub.len() as u64).to_le_bytes())?;
    out.write_all(MAGIC)?;
    out.flush()
}

fn payload_start(file: &mut File) -> io::Result<Option<u64>> {
    let len = file.seek(SeekFrom::End(0))?;
    if len < FOOTER_LEN {
        return Ok(None);
    }
    file.seek(SeekFrom::End(-(FOOTER_LEN as i64)))?;
    let mut footer = [0; FOOTER_LEN as usize];
    file.read_exact(&mut footer)?;
    if &footer[8..] != MAGIC {
        return Ok(None);
    }
    let start = u64::from_le_bytes(footer[..8].try_into().expect("8 bytes"));
    Ok((start < len).then_some(start))
}

fn read_u16(reader: &mut impl Read) -> io::Result<u16> {
    let mut bytes = [0; 2];
    reader.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> io::Result<u64> {
    let mut bytes = [0; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}
