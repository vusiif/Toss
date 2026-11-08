//! Strict, bounded-buffer transcoding. Never substitutes replacement characters.
use encoding_rs::{Decoder, DecoderResult, Encoding};
use std::io::{self, Read};
pub struct Decoded<R> {
    source: R,
    decoder: Decoder,
    label: &'static str,
    input: [u8; 8192],
    start: usize,
    end: usize,
    eof: bool,
    done: bool,
    output: [u8; 8192],
    output_start: usize,
    output_end: usize,
}
impl<R: Read> Decoded<R> {
    pub fn new(source: R, encoding: &'static Encoding) -> Self {
        Self {
            source,
            decoder: encoding.new_decoder_without_bom_handling(),
            label: encoding.name(),
            input: [0; 8192],
            start: 0,
            end: 0,
            eof: false,
            done: false,
            output: [0; 8192],
            output_start: 0,
            output_end: 0,
        }
    }
    fn fill(&mut self) -> io::Result<()> {
        self.output_start = 0;
        self.output_end = 0;
        while !self.done {
            if self.start == self.end && !self.eof {
                self.end = self.source.read(&mut self.input)?;
                self.start = 0;
                self.eof = self.end == 0;
            }
            let (result, read, written) = self.decoder.decode_to_utf8_without_replacement(
                &self.input[self.start..self.end],
                &mut self.output,
                self.eof,
            );
            self.start += read;
            match result {
                DecoderResult::Malformed(..) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid {} byte sequence", self.label),
                    ));
                }
                DecoderResult::InputEmpty if self.eof => self.done = true,
                _ => {}
            }
            self.output_end = written;
            if written > 0 {
                break;
            }
        }
        Ok(())
    }
}
impl<R: Read> Read for Decoded<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.output_start == self.output_end {
            self.fill()?;
        }
        let count = out.len().min(self.output_end - self.output_start);
        out[..count].copy_from_slice(&self.output[self.output_start..self.output_start + count]);
        self.output_start += count;
        Ok(count)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Tiny<R>(R);
    impl<R: Read> Read for Tiny<R> {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            let n = b.len().min(1);
            self.0.read(&mut b[..n])
        }
    }
    #[test]
    fn split_utf16_and_gb18030_scalars_survive_one_byte_reads() {
        let text = "中文,😀\r\n\"a,b\",001";
        for encoding in [
            encoding_rs::UTF_16LE,
            encoding_rs::UTF_16BE,
            encoding_rs::GB18030,
        ] {
            let bytes: Vec<u8> = if encoding == encoding_rs::UTF_16LE {
                text.encode_utf16().flat_map(u16::to_le_bytes).collect()
            } else if encoding == encoding_rs::UTF_16BE {
                text.encode_utf16().flat_map(u16::to_be_bytes).collect()
            } else {
                let (bytes, _, errors) = encoding.encode(text);
                assert!(!errors);
                bytes.into_owned()
            };
            let mut decoder = Decoded::new(Tiny(bytes.as_slice()), encoding);
            let mut actual = Vec::new();
            let mut byte = [0];
            while decoder.read(&mut byte).unwrap() > 0 {
                actual.push(byte[0]);
            }
            assert_eq!(String::from_utf8(actual).unwrap(), text);
        }
    }
    #[test]
    fn truncated_utf16_and_gb_sequences_are_errors() {
        for (encoding, bytes) in [
            (encoding_rs::UTF_16LE, vec![0x00, 0xd8]),
            (encoding_rs::UTF_16BE, vec![0x00]),
            (encoding_rs::GB18030, vec![0x81, 0x30, 0x81]),
        ] {
            assert!(
                Decoded::new(bytes.as_slice(), encoding)
                    .read_to_end(&mut Vec::new())
                    .is_err()
            );
        }
    }
}
