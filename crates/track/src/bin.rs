//! Binary encoding of the package's data files: a 4-byte magic, a u32 version, then
//! values in little-endian order. Arrays are a u32 element count followed by the
//! elements, so bulk vertex data is read with plain copies.

use std::ops::RangeInclusive;

use crate::Error;

pub struct Writer(Vec<u8>);

impl Writer {
    pub fn new(magic: &[u8; 4], version: u32) -> Self {
        let mut w = Self(magic.to_vec());
        w.u32(version);
        w
    }

    pub fn finish(self) -> Vec<u8> {
        self.0
    }

    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }

    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn len(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("array longer than u32::MAX"));
    }

    pub fn bytes(&mut self, v: &[u8]) {
        self.len(v.len());
        self.0.extend_from_slice(v);
    }

    pub fn u32s(&mut self, v: &[u32]) {
        self.len(v.len());
        self.0.extend(v.iter().flat_map(|x| x.to_le_bytes()));
    }

    pub fn vecs<const N: usize>(&mut self, v: &[[f32; N]]) {
        self.len(v.len());
        self.0
            .extend(v.iter().flatten().flat_map(|x| x.to_le_bytes()));
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    /// The file's format version.
    pub version: u32,
}

impl<'a> Reader<'a> {
    /// Starts reading a file of one of the `versions`, checking the magic; `what` names
    /// the file in errors.
    pub fn new(
        buf: &'a [u8],
        magic: &[u8; 4],
        versions: RangeInclusive<u32>,
        what: &str,
    ) -> Result<Self, Error> {
        if !buf.starts_with(magic) {
            return Err(Error::Format(format!(
                "{what}: not an open-racing package file"
            )));
        }
        let mut r = Self {
            buf,
            pos: magic.len(),
            version: 0,
        };
        let found = r.u32()?;
        r.version = found;
        if !versions.contains(&found) {
            return Err(Error::Format(format!(
                "{what}: format version {found}, expected {}; convert it again",
                versions.end()
            )));
        }
        Ok(r)
    }

    /// Fails when data is left over, which means the file is not what it claims to be.
    pub fn finish(self) -> Result<(), Error> {
        match self.buf.len() - self.pos {
            0 => Ok(()),
            n => Err(Error::Format(format!("{n} unexpected bytes at the end"))),
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.buf.len() - self.pos {
            return Err(Error::Format(format!(
                "unexpected end of data at byte {}",
                self.pos
            )));
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn word(&mut self) -> Result<[u8; 4], Error> {
        Ok(self.take(4)?.try_into().expect("length checked"))
    }

    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    pub fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.word()?))
    }

    pub fn f32(&mut self) -> Result<f32, Error> {
        Ok(f32::from_le_bytes(self.word()?))
    }

    pub fn bytes(&mut self) -> Result<&'a [u8], Error> {
        let n = self.u32()? as usize;
        self.take(n)
    }

    pub fn u32s(&mut self) -> Result<Vec<u32>, Error> {
        let n = self.u32()? as usize;
        let raw = self.take(
            n.checked_mul(4)
                .ok_or_else(|| Error::Format("array too long".into()))?,
        )?;
        Ok(raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect())
    }

    pub fn vecs<const N: usize>(&mut self) -> Result<Vec<[f32; N]>, Error> {
        let n = self.u32()? as usize;
        let raw = self.take(
            n.checked_mul(4 * N)
                .ok_or_else(|| Error::Format("array too long".into()))?,
        )?;
        Ok(raw
            .as_chunks::<4>()
            .0
            .as_chunks::<N>()
            .0
            .iter()
            .map(|v| v.map(f32::from_le_bytes))
            .collect())
    }
}
