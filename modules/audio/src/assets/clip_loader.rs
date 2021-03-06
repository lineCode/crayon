use std::io::Cursor;
use std::sync::Arc;

use lewton::inside_ogg::OggStreamReader;

use super::super::Result;
use super::clip::*;

pub const MAGIC: [u8; 8] = [b'C', b'S', b'F', b'X', b' ', 0, 0, 1];

#[derive(Clone)]
pub struct AudioClipLoader {}

impl AudioClipLoader {
    pub(crate) fn new() -> Self {
        AudioClipLoader {}
    }
}

impl ::crayon::res::registry::Register for AudioClipLoader {
    type Handle = AudioClipHandle;
    type Intermediate = AudioClip;
    type Value = Arc<AudioClip>;

    fn load(&self, handle: Self::Handle, bytes: &[u8]) -> Result<Self::Intermediate> {
        if &bytes[0..8] != MAGIC {
            bail!("[AudioClipLoader] MAGIC number not match.");
        }

        let cursor = Cursor::new(&bytes[8..]);
        let mut stream_reader = OggStreamReader::new(cursor).unwrap();

        let mut clip = AudioClip {
            channels: stream_reader.ident_hdr.audio_channels,
            sample_rate: stream_reader.ident_hdr.audio_sample_rate,
            pcm: Vec::new(),
        };

        while let Some(v) = stream_reader.read_dec_packet_itl()? {
            clip.pcm.extend(&v);
        }

        info!(
            "[AudioClipLoader] loads clip {:?} (channels {:?} sample_rate {:?} pcm: {:?}).",
            handle,
            clip.channels,
            clip.sample_rate,
            clip.pcm.len()
        );

        Ok(clip)
    }

    fn attach(&self, _: Self::Handle, item: Self::Intermediate) -> Result<Self::Value> {
        Ok(Arc::new(item))
    }

    fn detach(&self, _: Self::Handle, _: Self::Value) {}
}
