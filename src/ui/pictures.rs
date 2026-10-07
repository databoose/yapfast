//! Pictures: thumbnails of staged files, the picture cache, fitting and
//! animating pictures, blurred spoilers, galleries, and the QR code for linking.

use super::*;

/// The readable part of a Matrix ID: `@alice:example.org` becomes `alice`.
/// A QR code carrying `address`, black on white with the quiet zone around it,
/// one pixel to a module; Slint scales it up without smoothing.
#[cfg(feature = "discord")]
pub(super) fn qr_code(address: &str) -> Option<slint::Image> {
    /// The blank margin scanners need, in modules.
    const QUIET: usize = 4;
    let code = qrcode::QrCode::new(address.as_bytes()).ok()?;
    let width = code.width();
    let modules = code.to_colors();
    let side = width + 2 * QUIET;
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(side as u32, side as u32);
    for (index, pixel) in buffer.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = (index % side, index / side);
        let dark = (QUIET..QUIET + width).contains(&x)
            && (QUIET..QUIET + width).contains(&y)
            && modules[(y - QUIET) * width + (x - QUIET)] == qrcode::Color::Dark;
        let value = if dark { 0 } else { 255 };
        *pixel = slint::Rgba8Pixel::new(value, value, value, 255);
    }
    Some(slint::Image::from_rgba8(buffer))
}

/// The picture at `url` once it has arrived, converted for Slint only once.
/// Reads a picture waiting in the composer off the UI thread, then shows its preview.
pub(super) fn load_thumbnail(
    runtime: &BackgroundRuntime,
    weak: std::rc::Weak<RefCell<Adapter>>,
    path: PathBuf,
) {
    let reading = {
        let path = path.clone();
        runtime.spawn_blocking(move || {
            std::fs::read(&path)
                .ok()
                .and_then(|data| talk_core::chat::decode_picture(&data, 240, 240))
        })
    };
    let task = slint::spawn_local(async move {
        let picture = reading.await.ok().flatten();
        let Some(adapter) = weak.upgrade() else {
            return;
        };
        let window = adapter.borrow().window.upgrade();
        let mut adapter = adapter.borrow_mut();
        // Unless it was taken out of the composer meanwhile.
        let Some(slot) = adapter.thumbnails.get_mut(&path) else {
            return;
        };
        *slot = picture.map(|picture| {
            slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                &picture.rgba,
                picture.width,
                picture.height,
            ))
        });
        if let Some(window) = window {
            adapter.refresh(&window);
        }
    });
    if let Err(error) = task {
        tracing::error!(%error, "could not preview a file to send");
    }
}

pub(super) fn cached_picture(
    cache: &mut HashMap<String, slint::Image>,
    state: &talk_core::application::AppState,
    url: Option<&str>,
) -> slint::Image {
    let Some(url) = url else {
        return slint::Image::default();
    };
    if let Some(image) = cache.get(url) {
        return image.clone();
    }
    let Some(arrived) = state.pictures.get(url) else {
        return slint::Image::default();
    };
    let image = picture(arrived);
    cache.insert(url.to_owned(), image.clone());
    image
}

/// A full image's size once shrunk to the viewer's largest.
pub(super) fn fit(width: u32, height: u32) -> (u32, u32) {
    use talk_core::chat::{VIEW_HEIGHT, VIEW_WIDTH};
    let scale = f64::min(
        1.0,
        f64::min(
            f64::from(VIEW_WIDTH) / f64::from(width.max(1)),
            f64::from(VIEW_HEIGHT) / f64::from(height.max(1)),
        ),
    );
    let side = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    (side(width), side(height))
}

pub(super) fn picture(picture: &Picture) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &picture.rgba,
        picture.width,
        picture.height,
    );
    slint::Image::from_rgba8(buffer)
}

/// An animation's frames for Slint and how long each shows, in milliseconds;
/// both empty for a still picture.
pub(super) fn animation(picture: &Arc<Picture>) -> (ModelRc<slint::Image>, ModelRc<i32>) {
    if picture.frames.len() < 2 {
        return (ModelRc::default(), ModelRc::default());
    }
    let delays = ModelRc::new(VecModel::from(
        picture
            .frames
            .iter()
            .map(|frame| i32::try_from(frame.delay_ms).unwrap_or(i32::MAX))
            .collect::<Vec<_>>(),
    ));
    let frames = FRAMES.with_borrow_mut(|cache| {
        cache.retain(|(source, _)| source.strong_count() > 0);
        if let Some((_, frames)) = cache
            .iter()
            .find(|(source, _)| std::ptr::eq(source.as_ptr(), Arc::as_ptr(picture)))
        {
            return frames.clone();
        }
        let frames = ModelRc::new(VecModel::from(
            picture
                .frames
                .iter()
                .map(|frame| {
                    slint::Image::from_rgba8(
                        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                            &frame.rgba,
                            picture.width,
                            picture.height,
                        ),
                    )
                })
                .collect::<Vec<_>>(),
        ));
        cache.push((Arc::downgrade(picture), frames.clone()));
        frames
    });
    (frames, delays)
}

/// A picture shrunk to a few pixels, which Slint smooths into a blur when it's
/// drawn at full size: a spoiler's picture before it's revealed.
pub(super) fn blurred(picture: &Picture) -> slint::Image {
    const SIDE: u32 = 10;
    let scale = (picture.width.max(picture.height) / SIDE).max(1);
    let (width, height) = (
        picture.width.div_ceil(scale).max(1),
        picture.height.div_ceil(scale).max(1),
    );
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0u32; 4];
            let mut count = 0;
            for sy in y * scale..((y + 1) * scale).min(picture.height) {
                for sx in x * scale..((x + 1) * scale).min(picture.width) {
                    let at = ((sy * picture.width + sx) * 4) as usize;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(picture.rgba[at + channel]);
                    }
                    count += 1;
                }
            }
            rgba.extend(sum.map(|total| (total / count.max(1)) as u8));
        }
    }
    let buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, width, height);
    slint::Image::from_rgba8(buffer)
}

/// A picture whose preview is on its way, shown as a skeleton meanwhile.
pub(super) fn loading(attachment: &talk_core::chat::Attachment) -> bool {
    attachment.image && attachment.preview.is_none() && !attachment.preview_failed
}

/// The size a preview will show at, from the picture's own size when known,
/// so its skeleton takes the same room.
pub(super) fn coming_size(attachment: &talk_core::chat::Attachment) -> (i32, i32) {
    use talk_core::chat::{PREVIEW_HEIGHT, PREVIEW_WIDTH};
    let Some((width, height)) = attachment.dimensions else {
        return (PREVIEW_WIDTH as i32, 200);
    };
    let scale = f64::min(
        1.0,
        f64::min(
            f64::from(PREVIEW_WIDTH) / f64::from(width.max(1)),
            f64::from(PREVIEW_HEIGHT) / f64::from(height.max(1)),
        ),
    );
    let side = |side: u32| ((f64::from(side) * scale).round() as i32).max(1);
    (side(width), side(height))
}

/// A message's previews laid out: one at its own size, several as square
/// tiles two to a row, together as wide as one preview.
pub(super) fn gallery(
    previews: &[Preview<'_>],
    covered: &dyn Fn(usize) -> bool,
) -> (Vec<ImageCell>, i32, i32) {
    const TILE: i32 = 178;
    const GAP: i32 = 4;
    let cell =
        |index: usize, attachment: &talk_core::chat::Attachment, preview: Option<&Arc<Picture>>| {
            let Some(preview) = preview else {
                return ImageCell {
                    index: index as i32,
                    name: attachment.name.clone().into(),
                    loading: true,
                    ..Default::default()
                };
            };
            let spoiler = attachment.spoiler && covered(index);
            if spoiler {
                return ImageCell {
                    index: index as i32,
                    name: attachment.name.clone().into(),
                    image: blurred(preview),
                    spoiler,
                    ..Default::default()
                };
            }
            let (frames, delays) = animation(preview);
            ImageCell {
                index: index as i32,
                name: attachment.name.clone().into(),
                image: picture(preview),
                frames,
                delays,
                ..Default::default()
            }
        };
    match previews {
        [] => (vec![], 0, 0),
        [(index, attachment, preview)] => {
            let (width, height) = match preview {
                Some(preview) => (preview.width as i32, preview.height as i32),
                None => coming_size(attachment),
            };
            let image = ImageCell {
                width,
                height,
                ..cell(*index, attachment, *preview)
            };
            (vec![image], width, height)
        }
        _ => {
            let cells = previews
                .iter()
                .enumerate()
                .map(|(place, (index, attachment, preview))| ImageCell {
                    x: (place % 2) as i32 * (TILE + GAP),
                    y: (place / 2) as i32 * (TILE + GAP),
                    width: TILE,
                    height: TILE,
                    ..cell(*index, attachment, *preview)
                })
                .collect();
            let rows = previews.len().div_ceil(2) as i32;
            (cells, 2 * TILE + GAP, rows * TILE + (rows - 1) * GAP)
        }
    }
}
