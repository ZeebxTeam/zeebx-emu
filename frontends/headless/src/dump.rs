//! The frame going out without a window.
//!
//! It is the mode for whoever already has somewhere to paint: a frontend that simulates the
//! console draws its own screen and wants only the pixels. Here there is no window, no keyboard —
//! the player uses a controller, which the system delivers without needing focus — and 3D, when
//! requested on the GPU, runs in the core's offscreen context.
//!
//! The format is fixed and headerless: one frame after another, always the same size. The reader
//! already knows the size — the console has only one — and a per-frame header would be one more
//! thing to agree on between two programs that already agreed on everything in `config.ini`.

use std::io::Write;
use std::path::Path;

use zeebx::video::display::Framebuffer;

use crate::config::{Dump, Format};

/// Where the frames go.
pub struct Output {
    format: Format,
    dest: Destination,
    /// How many have gone out already. It is what numbers the `.png` files and what the limit
    /// counts.
    counted: u64,
    limit: u64,
}

enum Destination {
    /// Standard output. It is what a `zeebx-headless game.mod | my-frontend` uses.
    Stdout,
    /// A file, which may be a FIFO — and then it does not touch the disk.
    File(std::fs::File),
    /// A folder with one `.png` per frame.
    Dir(std::path::PathBuf),
}

impl Output {
    pub fn open(dump: &Dump) -> Result<Self, String> {
        let dest = match (&dump.dest, dump.format) {
            (Some(path), Format::Png) => {
                std::fs::create_dir_all(path)
                    .map_err(|error| format!("could not create {}: {error}", path.display()))?;
                Destination::Dir(path.clone())
            }
            // PNG on standard output would be one file after another in the same stream, which no
            // PNG reader unwraps. Saying so is better than writing something useless.
            (None, Format::Png) => {
                return Err("png dumping needs a folder in `destination`".to_string());
            }
            (Some(path), _) => {
                let file = open_for_writing(path)
                    .map_err(|error| format!("could not open {}: {error}", path.display()))?;
                Destination::File(file)
            }
            (None, _) => Destination::Stdout,
        };
        Ok(Self {
            format: dump.format,
            dest,
            counted: 0,
            limit: dump.limit,
        })
    }

    /// Whether too many frames have gone out. It is what ends the run in windowless mode.
    pub fn full(&self) -> bool {
        self.limit > 0 && self.counted >= self.limit
    }

    pub fn counted(&self) -> u64 {
        self.counted
    }

    /// Writes a frame.
    ///
    /// An error here **ends the run**, and is not ignored: the other end closed the pipe, and
    /// carrying on emulating to write into nothing would only waste the machine of whoever ran it.
    pub fn write(&mut self, frame: &Framebuffer) -> Result<(), String> {
        let (width, height) = (frame.width(), frame.height());
        match self.format {
            Format::Rgb565 => self.bytes(&frame.to_rgb565_bytes())?,
            Format::Rgba => {
                let rgba: Vec<u8> = frame
                    .to_argb()
                    .into_iter()
                    .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, 255])
                    .collect();
                self.bytes(&rgba)?;
            }
            Format::Png => {
                let Destination::Dir(dir) = &self.dest else {
                    return Err("png dumping without a folder".to_string());
                };
                let path = dir.join(format!("quadro-{:06}.png", self.counted));
                let rgba: Vec<u8> = frame
                    .to_argb()
                    .into_iter()
                    .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, 255])
                    .collect();
                write_png(&path, &rgba, width, height)
                    .map_err(|error| format!("could not write {}: {error}", path.display()))?;
            }
        }
        self.counted += 1;
        Ok(())
    }

    fn bytes(&mut self, data: &[u8]) -> Result<(), String> {
        let written = match &mut self.dest {
            Destination::Stdout => {
                let output = std::io::stdout();
                let mut locked = output.lock();
                // Each frame goes out whole or not at all: the reader counts bytes, and half a
                // frame would misalign everything that comes after.
                locked.write_all(data).and_then(|()| locked.flush())
            }
            Destination::File(file) => file.write_all(data).and_then(|()| file.flush()),
            Destination::Dir(_) => Ok(()),
        };
        written.map_err(|error| format!("the frame did not go out: {error}"))
    }
}

/// Opens for writing without truncating what is not a regular file.
///
/// A FIFO is the case that matters: `File::create` on one blocks until someone opens the other
/// end, which is exactly what is wanted — but truncating makes no sense, and on some systems it
/// errors. That is why the options are spelled out by hand.
fn open_for_writing(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(path.is_file())
        .open(path)
}

fn write_png(path: &Path, rgba: &[u8], width: u32, height: u32) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    writer.write_image_data(rgba).map_err(std::io::Error::other)
}
