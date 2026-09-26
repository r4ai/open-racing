//! Texture preparation for converters: textures in a package are block-compressed DDS
//! files, which the GPU samples without decoding, normally with a complete mip chain so
//! distant surfaces are filtered instead of shimmering.
//!
//! Levels already present in a block-compressed source are kept as they are and only the
//! missing ones are generated. Other sources are decoded and encoded.

use texpresso::{Format, Params};

const DDS_MAGIC: &[u8] = b"DDS ";
const HEADER_SIZE: usize = 128;
const FOURCC_FLAG: u32 = 0x4;
const RGB_FLAG: u32 = 0x40;
/// Grey levels, in the red mask, possibly with alpha.
const LUMINANCE_FLAG: u32 = 0x2_0000;
const MIPMAP_COUNT_FLAG: u32 = 0x2_0000;

/// A decoded image, 8-bit RGBA, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// Source image: block-compressed levels as stored, or decoded pixels.
enum Source {
    Blocks {
        format: Format,
        width: usize,
        height: usize,
        levels: Vec<Vec<u8>>,
    },
    Pixels(Image),
}

/// Which mip levels a prepared texture has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mips {
    /// A complete chain.
    Complete,
    /// A complete chain for a texture alpha-tested at this cut-off: generated levels keep
    /// the fraction of pixels passing the test, so alpha-tested foliage does not thin out
    /// with distance.
    AlphaTest(f32),
    /// The source's levels only. Averaging erases sub-pixel features of alpha-blended
    /// textures, such as wires, that the source relies on being sampled sharply.
    Source,
}

/// Returns the texture as a DDS with the mip levels `mips` asks for. Formats the
/// preparation does not handle are returned unchanged.
pub fn prepare(encoded: &[u8], mips: Mips) -> Result<Vec<u8>, String> {
    Ok(parse(encoded)?
        .and_then(|source| finish(source, mips))
        .unwrap_or_else(|| encoded.to_vec()))
}

/// Drops the top mip levels of a block-compressed DDS until neither side exceeds
/// `max_size` texels. Textures without those levels stored, and other formats, are
/// returned unchanged.
pub fn limit_size(dds: Vec<u8>, max_size: usize) -> Vec<u8> {
    let Ok(Some(Source::Blocks {
        format,
        width,
        height,
        levels,
    })) = parse_dds(&dds)
    else {
        return dds;
    };
    let mut top = 0;
    while width.max(height) >> top > max_size && top + 1 < levels.len() {
        top += 1;
    }
    if top == 0 {
        return dds;
    }
    let (w, h) = level_size(width, height, top);
    write_dds(format, w, h, &levels[top..])
}

/// Decodes the top level of a texture that `prepare` handles.
pub fn decode(encoded: &[u8]) -> Result<Image, String> {
    match parse(encoded)? {
        Some(Source::Blocks {
            format,
            width,
            height,
            levels,
        }) => Ok(decode_blocks(format, &levels[0], width, height)),
        Some(Source::Pixels(image)) => Ok(image),
        None => Err("unsupported DDS format".into()),
    }
}

/// Block-compresses an image into a DDS with a complete mip chain: BC1 when it is
/// opaque, BC3 otherwise.
pub fn encode(image: Image) -> Vec<u8> {
    finish(Source::Pixels(image), Mips::Complete).expect("pixel sources are always encoded")
}

/// `Ok(None)` for DDS variants that are passed through unchanged.
/// Decodes a JPEG to RGBA.
fn decode_jpeg(data: &[u8]) -> Result<Image, String> {
    use zune_jpeg::zune_core::{
        bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions,
    };
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(data), options);
    let pixels = decoder.decode().map_err(|e| format!("JPEG: {e:?}"))?;
    let info = decoder.info().ok_or("JPEG: no header")?;
    let (width, height) = (info.width as usize, info.height as usize);
    if pixels.len() != width * height * 4 {
        return Err("JPEG: unexpected colour layout".into());
    }
    Ok(Image {
        width,
        height,
        pixels,
    })
}

fn parse(encoded: &[u8]) -> Result<Option<Source>, String> {
    if encoded.starts_with(DDS_MAGIC) {
        parse_dds(encoded)
    } else if encoded.starts_with(b"\x89PNG") {
        Ok(Some(Source::Pixels(decode_png(encoded)?)))
    } else if encoded.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok(Some(Source::Pixels(decode_jpeg(encoded)?)))
    } else {
        Err("unknown image format".into())
    }
}

/// Encodes `source` with the mip levels `mips` asks for; `None` when a block-compressed
/// source already has them.
fn finish(source: Source, mips: Mips) -> Option<Vec<u8>> {
    let source = whole_blocks(source);
    let alpha_test = match mips {
        Mips::AlphaTest(c) if c > 0.0 && c < 1.0 => Some(c),
        _ => None,
    };

    // The smallest stored level, which the missing ones are generated from.
    let (format, width, height, mut levels, mut image) = match source {
        Source::Blocks {
            format,
            width,
            height,
            levels,
        } => {
            if mips == Mips::Source || levels.len() == mip_count(width, height) {
                return None;
            }
            let (w, h) = level_size(width, height, levels.len() - 1);
            let last = decode_blocks(format, &levels[levels.len() - 1], w, h);
            (format, width, height, levels, last)
        }
        Source::Pixels(top) => {
            let format = if top.pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255) {
                Format::Bc1
            } else {
                Format::Bc3
            };
            (
                format,
                top.width,
                top.height,
                vec![compress(format, &top)],
                top,
            )
        }
    };

    let wanted = if mips == Mips::Source {
        levels.len()
    } else {
        mip_count(width, height)
    };
    let cutoff = alpha_test.map(|c| (c * 255.0).round() as u8);
    let coverage = cutoff.map(|c| match levels.len() {
        1 => alpha_coverage(&image, c, 1.0),
        _ => alpha_coverage(&decode_blocks(format, &levels[0], width, height), c, 1.0),
    });
    while levels.len() < wanted {
        image = downsample(&image);
        if let (Some(c), Some(target)) = (cutoff, coverage) {
            keep_coverage(&mut image, c, target);
        }
        levels.push(compress(format, &image));
    }
    Some(write_dds(format, width, height, &levels))
}

/// A block-compressed texture's sides must be whole blocks: GPUs size its mip levels from
/// the rounded-up top level. Other sizes are resampled to the next multiple of 4.
fn whole_blocks(source: Source) -> Source {
    let (width, height) = match &source {
        Source::Blocks { width, height, .. } => (*width, *height),
        Source::Pixels(image) => (image.width, image.height),
    };
    let fit = |n: usize| n.div_ceil(4) * 4;
    if (fit(width), fit(height)) == (width, height) {
        return source;
    }
    let top = match source {
        Source::Blocks { format, levels, .. } => decode_blocks(format, &levels[0], width, height),
        Source::Pixels(image) => image,
    };
    Source::Pixels(resample(&top, fit(width), fit(height)))
}

/// Bilinear resampling to `width` × `height`, texel centres aligned.
fn resample(src: &Image, width: usize, height: usize) -> Image {
    let mut pixels = vec![0; width * height * 4];
    let coord = |i: usize, dst: usize, src: usize| {
        let f = ((i as f32 + 0.5) * src as f32 / dst as f32 - 0.5).clamp(0.0, (src - 1) as f32);
        let i0 = f.floor() as usize;
        (i0, (i0 + 1).min(src - 1), f - i0 as f32)
    };
    for y in 0..height {
        let (y0, y1, fy) = coord(y, height, src.height);
        for x in 0..width {
            let (x0, x1, fx) = coord(x, width, src.width);
            let at =
                |x: usize, y: usize, c: usize| f32::from(src.pixels[(y * src.width + x) * 4 + c]);
            for c in 0..4 {
                let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
                let bottom = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
                pixels[(y * width + x) * 4 + c] = (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
        }
    }
    Image {
        width,
        height,
        pixels,
    }
}

fn mip_count(width: usize, height: usize) -> usize {
    (usize::BITS - width.max(height).max(1).leading_zeros()) as usize
}

fn level_size(width: usize, height: usize, level: usize) -> (usize, usize) {
    ((width >> level).max(1), (height >> level).max(1))
}

fn u32_at(b: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(b[offset..offset + 4].try_into().expect("in bounds"))
}

/// `Ok(None)` for DDS variants that are passed through unchanged.
fn parse_dds(b: &[u8]) -> Result<Option<Source>, String> {
    if b.len() < HEADER_SIZE {
        return Err("truncated DDS header".into());
    }
    let (height, width) = (u32_at(b, 12) as usize, u32_at(b, 16) as usize);
    let flags = u32_at(b, 8);
    let stored = if flags & MIPMAP_COUNT_FLAG != 0 {
        (u32_at(b, 28) as usize).max(1)
    } else {
        1
    };
    let (pf_flags, fourcc, bits) = (u32_at(b, 80), &b[84..88], u32_at(b, 88));
    let data = &b[HEADER_SIZE..];
    if width == 0 || height == 0 {
        return Err("empty DDS".into());
    }

    if pf_flags & FOURCC_FLAG != 0 {
        let format = match fourcc {
            b"DXT1" => Format::Bc1,
            b"DXT3" => Format::Bc2,
            b"DXT5" => Format::Bc3,
            _ => return Ok(None),
        };
        let mut levels = Vec::new();
        let mut offset = 0;
        for level in 0..stored.min(mip_count(width, height)) {
            let (w, h) = level_size(width, height, level);
            let size = format.compressed_size(w, h);
            let Some(bytes) = data.get(offset..offset + size) else {
                break;
            };
            levels.push(bytes.to_vec());
            offset += size;
        }
        if levels.is_empty() {
            return Err("truncated DDS data".into());
        }
        return Ok(Some(Source::Blocks {
            format,
            width,
            height,
            levels,
        }));
    }

    let luminance = pf_flags & LUMINANCE_FLAG != 0;
    if (pf_flags & RGB_FLAG != 0 || luminance) && matches!(bits, 8 | 16 | 24 | 32) {
        let mut masks = [u32_at(b, 92), u32_at(b, 96), u32_at(b, 100), u32_at(b, 104)];
        if luminance {
            masks[1] = masks[0];
            masks[2] = masks[0];
        }
        let size = bits as usize / 8;
        let texels = data
            .get(..width * height * size)
            .ok_or("truncated DDS data")?;
        let pixels = texels
            .chunks_exact(size)
            .flat_map(|p| {
                let px = p
                    .iter()
                    .rev()
                    .fold(0, |px, &byte| px << 8 | u32::from(byte));
                masks.map(|m| channel(px, m))
            })
            .collect();
        return Ok(Some(Source::Pixels(Image {
            width,
            height,
            pixels,
        })));
    }
    Ok(None)
}

/// The bits of `px` under `mask`, scaled to 0–255; 255 for an absent channel.
fn channel(px: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 255;
    }
    let max = mask >> mask.trailing_zeros();
    (u64::from((px & mask) >> mask.trailing_zeros()) * 255 / u64::from(max)) as u8
}

fn decode_png(b: &[u8]) -> Result<Image, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(b));
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA,
    );
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("PNG too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (width, height) = (info.width as usize, info.height as usize);
    let pixels = match info.color_type {
        png::ColorType::Rgba => buf[..width * height * 4].to_vec(),
        png::ColorType::GrayscaleAlpha => buf[..width * height * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[l, a]| [l, l, l, a])
            .collect(),
        other => return Err(format!("unexpected PNG colour type {other:?}")),
    };
    Ok(Image {
        width,
        height,
        pixels,
    })
}

fn decode_blocks(format: Format, blocks: &[u8], width: usize, height: usize) -> Image {
    let mut pixels = vec![0; width * height * 4];
    format.decompress(blocks, width, height, &mut pixels);
    Image {
        width,
        height,
        pixels,
    }
}

fn compress(format: Format, image: &Image) -> Vec<u8> {
    let mut out = vec![0; format.compressed_size(image.width, image.height)];
    format.compress(
        &image.pixels,
        image.width,
        image.height,
        Params::default(),
        &mut out,
    );
    out
}

/// Halves each dimension with a 2×2 box filter.
fn downsample(src: &Image) -> Image {
    let (width, height) = ((src.width / 2).max(1), (src.height / 2).max(1));
    let mut pixels = vec![0; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            for c in 0..4 {
                let mut sum = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (2 * x + dx).min(src.width - 1);
                    let sy = (2 * y + dy).min(src.height - 1);
                    sum += u32::from(src.pixels[(sy * src.width + sx) * 4 + c]);
                }
                pixels[(y * width + x) * 4 + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    Image {
        width,
        height,
        pixels,
    }
}

/// Fraction of pixels whose alpha, scaled by `scale`, passes the alpha test.
fn alpha_coverage(image: &Image, cutoff: u8, scale: f32) -> f32 {
    let passing = image
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| f32::from(p[3]) * scale >= f32::from(cutoff))
        .count();
    passing as f32 / (image.width * image.height) as f32
}

/// Scales alpha up so that the level passes the alpha test over at least the `target`
/// fraction of its area, as the full-resolution image does. Alpha is never scaled down:
/// in atlases a region that averages above the cut-off (a fence) would otherwise make
/// thin features elsewhere (wires) fail it.
fn keep_coverage(image: &mut Image, cutoff: u8, target: f32) {
    let (mut lo, mut hi) = (1.0f32, 8.0f32);
    if alpha_coverage(image, cutoff, lo) >= target {
        return;
    }
    for _ in 0..20 {
        let mid = (lo + hi) * 0.5;
        if alpha_coverage(image, cutoff, mid) < target {
            lo = mid
        } else {
            hi = mid
        }
    }
    for p in image.pixels.as_chunks_mut::<4>().0 {
        p[3] = (f32::from(p[3]) * hi).min(255.0) as u8;
    }
}

fn write_dds(format: Format, width: usize, height: usize, levels: &[Vec<u8>]) -> Vec<u8> {
    let fourcc = match format {
        Format::Bc1 => b"DXT1",
        Format::Bc2 => b"DXT3",
        _ => b"DXT5",
    };
    let mut h = [0u32; 31];
    h[0] = 124;
    // Caps, height, width, pixel format, mipmap count, linear size.
    h[1] = 0x1 | 0x2 | 0x4 | 0x1000 | MIPMAP_COUNT_FLAG | 0x8_0000;
    h[2] = height as u32;
    h[3] = width as u32;
    h[4] = levels[0].len() as u32;
    h[6] = levels.len() as u32;
    h[18] = 32;
    h[19] = FOURCC_FLAG;
    h[20] = u32::from_le_bytes(*fourcc);
    // Complex, mipmap, texture.
    h[26] = 0x8 | 0x40_0000 | 0x1000;
    let mut out = DDS_MAGIC.to_vec();
    out.extend(h.iter().flat_map(|v| v.to_le_bytes()));
    for level in levels {
        out.extend_from_slice(level);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpegs_decode_and_prepare() {
        let jpeg = include_bytes!("testdata/red.jpg");
        let image = decode(jpeg).unwrap();
        assert_eq!((image.width, image.height), (8, 4));
        let [r, g, b, a] = image.pixels[..4] else {
            unreachable!()
        };
        assert!(r > 180 && g < 60 && b < 70 && a == 255, "{r} {g} {b} {a}");
        assert!(
            prepare(jpeg, Mips::Complete)
                .unwrap()
                .starts_with(DDS_MAGIC)
        );
    }

    fn checker(size: usize, alpha: bool) -> Image {
        let pixels = (0..size * size)
            .flat_map(|i| {
                let on = (i % size + i / size).is_multiple_of(2);
                let v = if on { 255 } else { 0 };
                [v, 128, 255 - v, if alpha && !on { 0 } else { 255 }]
            })
            .collect();
        Image {
            width: size,
            height: size,
            pixels,
        }
    }

    fn levels_of(dds: &[u8]) -> (Format, usize, usize, usize) {
        match parse_dds(dds).unwrap().unwrap() {
            Source::Blocks {
                format,
                width,
                height,
                levels,
            } => (format, width, height, levels.len()),
            Source::Pixels(_) => panic!("expected blocks"),
        }
    }

    #[test]
    fn uncompressed_grey_and_packed_colour_expand_to_rgba() {
        let dds = |flags: u32, bits: u32, masks: [u32; 4], texel: &[u8]| {
            let mut b = vec![0; HEADER_SIZE];
            b[..4].copy_from_slice(DDS_MAGIC);
            for (offset, v) in [(12, 1), (16, 1), (80, flags), (88, bits)]
                .into_iter()
                .chain((0..4).map(|i| (92 + 4 * i, masks[i])))
            {
                b[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(texel);
            match parse_dds(&b).unwrap().unwrap() {
                Source::Pixels(img) => img.pixels,
                Source::Blocks { .. } => panic!("expected pixels"),
            }
        };
        // L8A8: grey 200 with alpha 100.
        assert_eq!(
            dds(LUMINANCE_FLAG | 1, 16, [0xff, 0, 0, 0xff00], &[200, 100]),
            [200, 200, 200, 100]
        );
        // L8, opaque.
        assert_eq!(
            dds(LUMINANCE_FLAG, 8, [0xff, 0, 0, 0], &[7]),
            [7, 7, 7, 255]
        );
        // R5G6B5 white.
        assert_eq!(
            dds(RGB_FLAG, 16, [0xf800, 0x07e0, 0x001f, 0], &[0xff, 0xff]),
            [255; 4]
        );
    }

    #[test]
    fn sides_become_whole_blocks() {
        let odd = Image {
            width: 6,
            height: 3,
            pixels: [10, 20, 30, 255].repeat(18),
        };
        let out = finish(Source::Pixels(odd.clone()), Mips::Complete).unwrap();
        assert_eq!(levels_of(&out), (Format::Bc1, 8, 4, 4));
        // Resampling keeps a flat colour.
        let top = decode(&out).unwrap();
        assert!(
            top.pixels
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[0].abs_diff(10) <= 4)
        );
        // Block sources are resampled too, so a source-only chain still fits.
        let dds = write_dds(Format::Bc1, 6, 3, &[compress(Format::Bc1, &odd)]);
        assert_eq!(
            levels_of(&prepare(&dds, Mips::Source).unwrap()),
            (Format::Bc1, 8, 4, 1)
        );
    }

    #[test]
    fn large_textures_lose_their_top_levels() {
        let dds = encode(checker(64, false));
        assert_eq!(
            levels_of(&limit_size(dds.clone(), 16)),
            (Format::Bc1, 16, 16, 5)
        );
        assert_eq!(limit_size(dds.clone(), 64), dds);
        // A single level cannot shrink.
        let one = write_dds(
            Format::Bc1,
            64,
            64,
            &[compress(Format::Bc1, &checker(64, false))],
        );
        assert_eq!(limit_size(one.clone(), 16), one);
    }

    #[test]
    fn completes_a_truncated_mip_chain() {
        let top = checker(64, false);
        let dds = write_dds(
            Format::Bc1,
            64,
            64,
            &[
                compress(Format::Bc1, &top),
                compress(Format::Bc1, &downsample(&top)),
            ],
        );
        assert_eq!(prepare(&dds, Mips::Source).unwrap(), dds);
        let out = prepare(&dds, Mips::Complete).unwrap();
        assert_eq!(levels_of(&out), (Format::Bc1, 64, 64, 7));
        // The stored levels are kept byte for byte.
        assert_eq!(
            &out[HEADER_SIZE..HEADER_SIZE + dds.len() - HEADER_SIZE],
            &dds[HEADER_SIZE..]
        );
        // A complete chain is returned unchanged.
        assert_eq!(prepare(&out, Mips::Complete).unwrap(), out);
    }

    #[test]
    fn non_square_chain_ends_at_one_pixel() {
        let img = Image {
            width: 16,
            height: 4,
            pixels: vec![200; 16 * 4 * 4],
        };
        let dds = write_dds(Format::Bc1, 16, 4, &[compress(Format::Bc1, &img)]);
        assert_eq!(levels_of(&prepare(&dds, Mips::Complete).unwrap()).3, 5);
    }

    #[test]
    fn alpha_tested_texels_survive_downsampling() {
        // Half the texels are opaque. A box filter averages them to alpha 128 everywhere,
        // which fails a 0.8 cut-off: the texture would vanish at a distance.
        let top = checker(32, true);
        let mut level = downsample(&top);
        assert_eq!(alpha_coverage(&level, 204, 1.0), 0.0);
        keep_coverage(&mut level, 204, alpha_coverage(&top, 204, 1.0));
        assert!(alpha_coverage(&level, 204, 1.0) >= 0.5);
    }

    #[test]
    fn rgba_sources_are_compressed() {
        let img = checker(8, true);
        let mut dds = write_dds(Format::Bc1, 8, 8, &[vec![]]);
        dds.truncate(HEADER_SIZE);
        // Uncompressed 32-bit RGBA header.
        dds[80..84].copy_from_slice(&RGB_FLAG.to_le_bytes());
        dds[84..88].copy_from_slice(&[0; 4]);
        dds[88..92].copy_from_slice(&32u32.to_le_bytes());
        for (i, m) in [0xffu32, 0xff00, 0xff_0000, 0xff00_0000].iter().enumerate() {
            dds[92 + 4 * i..96 + 4 * i].copy_from_slice(&m.to_le_bytes());
        }
        dds.extend_from_slice(&img.pixels);
        assert_eq!(
            levels_of(&prepare(&dds, Mips::Complete).unwrap()),
            (Format::Bc3, 8, 8, 4)
        );
        assert_eq!(
            levels_of(&prepare(&dds, Mips::Source).unwrap()),
            (Format::Bc3, 8, 8, 1)
        );

        // Uncompressed 24-bit BGR.
        dds.truncate(HEADER_SIZE);
        dds[88..92].copy_from_slice(&24u32.to_le_bytes());
        for (i, m) in [0xff_0000u32, 0xff00, 0xff, 0].iter().enumerate() {
            dds[92 + 4 * i..96 + 4 * i].copy_from_slice(&m.to_le_bytes());
        }
        dds.extend(
            img.pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0]]),
        );
        let rgb: Vec<u8> = img
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
        assert_eq!(decode(&dds).unwrap().pixels, rgb);
        assert_eq!(
            levels_of(&prepare(&dds, Mips::Complete).unwrap()),
            (Format::Bc1, 8, 8, 4)
        );
    }
}
