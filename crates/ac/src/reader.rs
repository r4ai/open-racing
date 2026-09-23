//! Little-endian cursor over a byte buffer.

use crate::Error;

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.remaining() {
            return Err(Error::Format(format!("unexpected end of data at byte {}", self.pos)));
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn skip(&mut self, n: usize) -> Result<(), Error> {
        self.bytes(n).map(|_| ())
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        Ok(self.bytes(N)?.try_into().expect("length checked"))
    }

    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    pub fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    pub fn f32(&mut self) -> Result<f32, Error> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    /// A non-negative count, bounded by what the remaining data could hold.
    pub fn count(&mut self, min_item_size: usize) -> Result<usize, Error> {
        let n = self.i32()?;
        if n < 0 || n as usize * min_item_size.max(1) > self.remaining() {
            return Err(Error::Format(format!("implausible count {n} at byte {}", self.pos - 4)));
        }
        Ok(n as usize)
    }

    /// i32 length followed by bytes, decoded lossily.
    pub fn string(&mut self) -> Result<String, Error> {
        let n = self.count(1)?;
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }

    pub fn f32s<const N: usize>(&mut self) -> Result<[f32; N], Error> {
        let mut out = [0.0; N];
        for v in &mut out {
            *v = self.f32()?;
        }
        Ok(out)
    }
}

#[cfg(test)]
pub mod write {
    //! Builders for synthetic test data.

    #[derive(Default)]
    pub struct Writer(pub Vec<u8>);

    impl Writer {
        pub fn i32(&mut self, v: i32) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        pub fn u8(&mut self, v: u8) -> &mut Self {
            self.0.push(v);
            self
        }
        pub fn u16(&mut self, v: u16) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        pub fn f32(&mut self, v: f32) -> &mut Self {
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        pub fn f32s(&mut self, v: &[f32]) -> &mut Self {
            for &x in v {
                self.f32(x);
            }
            self
        }
        pub fn string(&mut self, s: &str) -> &mut Self {
            self.i32(s.len() as i32);
            self.0.extend_from_slice(s.as_bytes());
            self
        }
        pub fn raw(&mut self, b: &[u8]) -> &mut Self {
            self.0.extend_from_slice(b);
            self
        }
    }
}
