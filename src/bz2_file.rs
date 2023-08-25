use bzip2::read::MultiBzDecoder;
use std::{cell::Cell, error::Error, fs::File, io::Read};

/// BZ2File stores the BZDecoder which will decode the original file
pub struct BZ2File {
    /// Decoder involved with doing most of the bz2 decoding
    decoder: Cell<MultiBzDecoder<File>>,
    /// Stores the decoded bytes into this `block` or Vec
    pub decoded_block: Cell<Vec<u8>>,
}

impl BZ2File {
    /// Returns a BZ2File object which can decode the file
    ///
    /// # Arguments
    /// * `f`   -   The bz2 file that would be read after you opened it
    pub fn new(f: File) -> Self {
        Self {
            decoder: Cell::new(MultiBzDecoder::new(f)),
            decoded_block: Cell::new(Vec::<u8>::new()),
        }
    }

    /// Decodes the file, Writes into the `decoded_block` Vec, and Returns a reference to that Vec
    pub fn decode_block(self: &mut Self) -> Result<&mut Vec<u8>, Box<dyn Error>> {
        // Decodes the block of data from the bz2 file
        self.decoder
            .get_mut()
            .read_to_end(self.decoded_block.get_mut())?;

        return Ok(self.decoded_block.get_mut());
        // return self.decoded_block.get_mut();
    }
}
