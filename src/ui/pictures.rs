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
/// A staged file's picture, and the blur it shows as a spoiler.
pub(super) struct Thumbnail {
    pub(super) picture: slint::Image,
    pub(super) covered: slint::Image,
}

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
        *slot = picture.map(|picture| Thumbnail {
            picture: self::picture(&picture),
            covered: blurred(&picture),
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

/// The UI asks for avatars only as virtualized components use them. Weak
/// sources do not pin decoded core pictures; conversions share the bounded
/// preview cache rather than living in every off-screen row.
pub(super) fn refresh_avatar_sources(state: &AppState) {
    AVATAR_SOURCES.with_borrow_mut(|sources| {
        *sources = state
            .pictures
            .iter()
            .map(|(url, picture)| (url.clone(), Arc::downgrade(picture)))
            .collect();
    });
}

pub(super) fn avatar_picture(url: &str) -> slint::Image {
    let source =
        AVATAR_SOURCES.with_borrow(|sources| sources.get(url).and_then(std::sync::Weak::upgrade));
    source.map_or_else(slint::Image::default, |source| {
        PREVIEWS.with_borrow_mut(|cache| cache.image(&source, false))
    })
}

thread_local! {
    static AVATAR_SOURCES: RefCell<HashMap<String, std::sync::Weak<Picture>>> = RefCell::default();
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

/// A picture's pixels in Slint's own buffer, so showing it shares them.
struct SlintPixels(slint::SharedPixelBuffer<slint::Rgba8Pixel>);

impl talk_core::chat::PixelBuffer for SlintPixels {
    fn bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// Has pictures decoded into Slint's buffers from now on, so showing one
/// shares its pixels rather than copying them; otherwise each picture on
/// screen would be in memory twice.
pub(super) fn decode_into_slint_buffers() {
    talk_core::chat::set_pixel_buffers(|width, height, rgba| {
        Box::new(SlintPixels(slint::SharedPixelBuffer::clone_from_slice(
            rgba, width, height,
        )))
    });
}

/// A moving picture's frame in Slint's own buffer, which it shows as it is.
struct SlintFrame(slint::SharedPixelBuffer<slint::Rgba8Pixel>);

impl talk_core::picture::FrameBuffer for SlintFrame {
    fn size(&self) -> (u32, u32) {
        (self.0.width(), self.0.height())
    }

    fn pixels_mut(&mut self) -> &mut [u8] {
        // The feed writes only buffers Slint no longer shows, so this never
        // copies.
        self.0.make_mut_bytes()
    }
}

/// Has moving pictures made in Slint's buffers from now on.
pub(super) fn make_frames_in_slint_buffers() {
    talk_core::picture::set_frame_buffers(|width, height| {
        Box::new(SlintFrame(slint::SharedPixelBuffer::new(width, height)))
    });
}

/// A moving picture's frame, shown without a copy.
pub(crate) fn frame(buffer: &dyn talk_core::picture::FrameBuffer) -> slint::Image {
    match (buffer as &dyn std::any::Any).downcast_ref::<SlintFrame>() {
        Some(frame) => slint::Image::from_rgba8(frame.0.clone()),
        None => slint::Image::default(),
    }
}

pub(crate) fn picture(picture: &Picture) -> slint::Image {
    let buffer = match picture.rgba.buffer::<SlintPixels>() {
        Some(pixels) => pixels.0.clone(),
        None => {
            slint::SharedPixelBuffer::clone_from_slice(&picture.rgba, picture.width, picture.height)
        }
    };
    slint::Image::from_rgba8(buffer)
}

/// Gallery metadata is cheap to keep for the whole timeline. Copy pixels
/// into Slint only when a visible row asks for its image.
struct ImageCells(Vec<(ImageCell, Option<Arc<Picture>>)>);

pub(super) fn image_cells(cells: Vec<(ImageCell, Option<Arc<Picture>>)>) -> ModelRc<ImageCell> {
    ModelRc::new(ImageCells(cells))
}

impl Model for ImageCells {
    type Data = ImageCell;

    fn row_count(&self) -> usize {
        self.0.len()
    }

    fn row_data(&self, row: usize) -> Option<ImageCell> {
        let (cell, source) = self.0.get(row)?;
        let mut cell = cell.clone();
        if let Some(source) = source {
            cell.image = PREVIEWS.with_borrow_mut(|cache| cache.image(source, cell.spoiler));
        }
        Some(cell)
    }

    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &()
    }
}

/// Converted previews shared across row rebuilds, with an eight MiB limit.
/// Visible Slint components keep their own handles after cache eviction.
#[derive(Default)]
struct Previews {
    images: std::collections::VecDeque<CachedPreview>,
    bytes: usize,
}

struct CachedPreview {
    source: std::sync::Weak<Picture>,
    spoiler: bool,
    image: slint::Image,
    bytes: usize,
}

const PREVIEW_CACHE_BYTES: usize = 8 * 1024 * 1024;

impl Previews {
    fn image(&mut self, source: &Arc<Picture>, spoiler: bool) -> slint::Image {
        self.images
            .retain(|cached| cached.source.strong_count() > 0);
        self.bytes = self.images.iter().map(|cached| cached.bytes).sum();
        let weak = Arc::downgrade(source);
        if let Some(index) = self
            .images
            .iter()
            .position(|cached| cached.source.ptr_eq(&weak) && cached.spoiler == spoiler)
        {
            let cached = self.images.remove(index).unwrap();
            let image = cached.image.clone();
            self.images.push_back(cached);
            return image;
        }
        let image = if spoiler {
            blurred(source)
        } else {
            picture(source)
        };
        let size = image.size();
        let bytes = size.width as usize * size.height as usize * 4;
        if bytes <= PREVIEW_CACHE_BYTES {
            while self.bytes + bytes > PREVIEW_CACHE_BYTES {
                self.bytes -= self.images.pop_front().unwrap().bytes;
            }
            self.bytes += bytes;
            self.images.push_back(CachedPreview {
                source: weak,
                spoiler,
                image: image.clone(),
                bytes,
            });
        }
        image
    }
}

thread_local! {
    static PREVIEWS: RefCell<Previews> = RefCell::default();
}

/// An animation's frames for Slint and how long each shows, in milliseconds;
/// both empty for a still picture. The frames decode as they show.
pub(super) fn animation(picture: &Arc<Picture>) -> (ModelRc<slint::Image>, ModelRc<i32>) {
    let Some(animation) = &picture.animation else {
        return (ModelRc::default(), ModelRc::default());
    };
    let delays = ModelRc::new(VecModel::from(
        animation
            .delays
            .iter()
            .map(|delay| i32::try_from(*delay).unwrap_or(i32::MAX))
            .collect::<Vec<_>>(),
    ));
    let frames = FRAMES.with_borrow_mut(|cache| {
        cache.retain(|frames| frames.strong_count() > 0);
        let shared = cache
            .iter()
            .filter_map(std::rc::Weak::upgrade)
            .find(|frames| Arc::ptr_eq(&frames.picture, picture));
        let frames = shared.unwrap_or_else(|| {
            let frames = Rc::new(Frames {
                picture: picture.clone(),
                decoding: RefCell::default(),
            });
            cache.push(Rc::downgrade(&frames));
            frames
        });
        ModelRc::from(frames)
    });
    (frames, delays)
}

thread_local! {
    /// Animations' frames handed to Slint, so a row built again shares its
    /// picture's.
    static FRAMES: RefCell<Vec<std::rc::Weak<Frames>>> = RefCell::default();
    /// Runs while decoders are in use, including after the last row scrolls away.
    static FRAME_SWEEP: slint::Timer = slint::Timer::default();
}

/// How long an animation keeps its decoder after its last frame showed, as
/// when its row scrolled away.
const IDLE_FRAMES: Duration = Duration::from_secs(1);

/// An animation's frames, decoded in order as Slint asks for them: only the
/// one showing is kept, and only while it shows.
struct Frames {
    picture: Arc<Picture>,
    decoding: RefCell<Decoding>,
}

#[derive(Default)]
struct Decoding {
    /// The frames still to come.
    frames: Option<Box<dyn Iterator<Item = Vec<u8>>>>,
    /// The index of the next.
    next: usize,
    shown: Option<(usize, slint::Image)>,
    used: Option<Duration>,
}

impl Model for Frames {
    type Data = slint::Image;

    fn row_count(&self) -> usize {
        self.picture
            .animation
            .as_ref()
            .map_or(0, |animation| animation.delays.len())
    }

    fn row_data(&self, row: usize) -> Option<slint::Image> {
        let now = Duration::from_millis(i_slint_core::animations::current_tick().0);
        let image = {
            let mut decoding = self.decoding.borrow_mut();
            decoding.used = Some(now);
            match &decoding.shown {
                Some((shown, image)) if *shown == row => Some(image.clone()),
                _ => self.decode(&mut decoding, row),
            }
        };
        FRAME_SWEEP.with(|timer| {
            if !timer.running() {
                timer.start(slint::TimerMode::Repeated, IDLE_FRAMES, || {
                    if !sweep(Duration::from_millis(
                        i_slint_core::animations::current_tick().0,
                    )) {
                        FRAME_SWEEP.with(slint::Timer::stop);
                    }
                });
            }
        });
        image
    }

    fn model_tracker(&self) -> &dyn slint::ModelTracker {
        &()
    }
}

impl Frames {
    /// Frame `row`, decoding on from the last or again from the first. A
    /// frame that fails leaves the last one showing.
    fn decode(&self, decoding: &mut Decoding, row: usize) -> Option<slint::Image> {
        if decoding.frames.is_none() || decoding.next > row {
            decoding.frames = Some(self.picture.frames());
            decoding.next = 0;
        }
        let frames = decoding.frames.as_mut()?;
        while decoding.next < row {
            frames.next();
            decoding.next += 1;
        }
        let Some(rgba) = frames.next() else {
            return Some(
                decoding
                    .shown
                    .as_ref()
                    .map_or_else(|| picture(&self.picture), |(_, image)| image.clone()),
            );
        };
        decoding.next += 1;
        let image = slint::Image::from_rgba8(
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &rgba,
                self.picture.width,
                self.picture.height,
            ),
        );
        decoding.shown = Some((row, image.clone()));
        Some(image)
    }
}

/// Lets animations that stopped showing give up their decoders and frames.
fn sweep(now: Duration) -> bool {
    let mut active = false;
    FRAMES.with_borrow(|cache| {
        for frames in cache.iter().filter_map(std::rc::Weak::upgrade) {
            // The one decoding now is in use.
            let Ok(mut decoding) = frames.decoding.try_borrow_mut() else {
                active = true;
                continue;
            };
            if decoding
                .used
                .is_some_and(|used| now.saturating_sub(used) >= IDLE_FRAMES)
            {
                *decoding = Decoding::default();
            }
            active |= decoding.used.is_some();
        }
    });
    active
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
    (attachment.image || attachment.video)
        && attachment.preview.is_none()
        && !attachment.preview_failed
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
) -> (ModelRc<ImageCell>, i32, i32) {
    const TILE: i32 = 178;
    const GAP: i32 = 4;
    let cell =
        |index: usize, attachment: &talk_core::chat::Attachment, preview: Option<&Arc<Picture>>| {
            // A video whose poster didn't come plays all the same.
            let Some(preview) = preview else {
                return ImageCell {
                    index: index as i32,
                    name: attachment.name.clone().into(),
                    loading: loading(attachment),
                    video: attachment.video,
                    ..Default::default()
                };
            };
            let spoiler = attachment.spoiler && covered(index);
            if spoiler {
                return ImageCell {
                    index: index as i32,
                    name: attachment.name.clone().into(),
                    spoiler,
                    video: attachment.video,
                    ..Default::default()
                };
            }
            let (frames, delays) = animation(preview);
            ImageCell {
                index: index as i32,
                name: attachment.name.clone().into(),
                frames,
                delays,
                video: attachment.video,
                gif: attachment
                    .gif
                    .as_ref()
                    .map(|gif| gif.url.as_str().into())
                    .unwrap_or_default(),
                ..Default::default()
            }
        };
    match previews {
        [] => (ModelRc::default(), 0, 0),
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
            (image_cells(vec![(image, preview.cloned())]), width, height)
        }
        _ => {
            let cells = previews
                .iter()
                .enumerate()
                .map(|(place, (index, attachment, preview))| {
                    (
                        ImageCell {
                            x: (place % 2) as i32 * (TILE + GAP),
                            y: (place / 2) as i32 * (TILE + GAP),
                            width: TILE,
                            height: TILE,
                            ..cell(*index, attachment, *preview)
                        },
                        preview.cloned(),
                    )
                })
                .collect();
            let rows = previews.len().div_ceil(2) as i32;
            (
                image_cells(cells),
                2 * TILE + GAP,
                rows * TILE + (rows - 1) * GAP,
            )
        }
    }
}

#[cfg(all(test, feature = "discord"))]
mod tests {
    use super::*;

    #[test]
    fn galleries_copy_only_requested_previews_and_bound_their_cache() {
        PREVIEWS.set(Previews::default());
        let attachments: Vec<_> = (0..40)
            .map(|index| talk_core::chat::Attachment {
                name: format!("{index}.png"),
                size: None,
                image: true,
                dimensions: Some((360, 240)),
                preview: Some(Arc::new(Picture {
                    width: 360,
                    height: 240,
                    rgba: talk_core::chat::Pixels::new(360, 240, vec![index; 360 * 240 * 4]),
                    ..Default::default()
                })),
                preview_failed: false,
                placement: Placement::Message,
                gif: None,
                spoiler: false,
                video: false,
            })
            .collect();
        let row = message_row(
            &ChatMessage {
                attachments,
                ..Default::default()
            },
            &HashSet::new(),
        );
        assert_eq!(row.images.row_count(), 40);
        PREVIEWS.with_borrow(|cache| assert_eq!(cache.bytes, 0));

        let first = row.images.row_data(0).unwrap();
        assert_eq!(
            (first.image.size().width, first.image.size().height),
            (360, 240)
        );
        assert_eq!(first.image, row.images.row_data(0).unwrap().image);
        PREVIEWS.with_borrow(|cache| assert_eq!(cache.bytes, 360 * 240 * 4));
        for index in 1..row.images.row_count() {
            row.images.row_data(index).unwrap();
        }
        PREVIEWS.with_borrow(|cache| {
            assert!(cache.bytes <= PREVIEW_CACHE_BYTES);
            assert!(cache.images.len() < row.images.row_count());
        });
        // Eviction cannot invalidate a handle held by a visible component.
        assert_eq!(
            (first.image.size().width, first.image.size().height),
            (360, 240)
        );
        assert_eq!(
            row.images.row_data(0).unwrap().image.size(),
            first.image.size()
        );
        PREVIEWS.set(Previews::default());
    }

    #[test]
    fn avatar_sources_are_weak_and_pixels_are_converted_only_when_requested() {
        PREVIEWS.set(Previews::default());
        let mut state = AppState::default();
        let source = Arc::new(Picture {
            width: 1,
            height: 1,
            rgba: talk_core::chat::Pixels::new(1, 1, vec![255; 4]),
            ..Default::default()
        });
        let weak = Arc::downgrade(&source);
        state.pictures.insert("avatar".into(), source);
        refresh_avatar_sources(&state);
        PREVIEWS.with_borrow(|cache| assert_eq!(cache.bytes, 0));
        let shown = avatar_picture("avatar");
        assert_eq!(shown, avatar_picture("avatar"));
        PREVIEWS.with_borrow(|cache| assert_eq!(cache.bytes, 4));
        drop(state);
        assert!(
            weak.upgrade().is_none(),
            "neither registry nor converted cache pins core pixels"
        );
        assert_eq!(avatar_picture("avatar").size().width, 0);
        assert_eq!(
            shown.size().width,
            1,
            "a component's image handle stays valid"
        );
        AVATAR_SOURCES.with_borrow_mut(HashMap::clear);
        PREVIEWS.set(Previews::default());
    }

    #[test]
    fn showing_a_decoded_picture_shares_its_pixels() {
        decode_into_slint_buffers();
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let decoded = talk_core::chat::decode_picture(&png, 240, 240).unwrap();
        let shown = picture(&decoded).to_rgba8().unwrap();
        assert_eq!(shown.as_bytes(), &*decoded.rgba);
        assert_eq!(
            shown.as_bytes().as_ptr(),
            decoded.rgba.as_ptr(),
            "not a copy"
        );
    }

    #[test]
    fn idle_animation_releases_its_decoder_without_another_frame_request() {
        i_slint_backend_testing::init_no_event_loop();
        let file = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testing/fixtures/gif-h264.mp4"),
        )
        .unwrap();
        let picture = Arc::new(talk_core::chat::decode_picture(&file, 32, 32).unwrap());
        let (frames, _) = animation(&picture);
        let shown = frames.row_data(0).unwrap();
        let model = FRAMES.with_borrow(|cache| cache.last().unwrap().upgrade().unwrap());
        assert!(model.decoding.borrow().frames.is_some());
        i_slint_backend_testing::mock_elapsed_time(IDLE_FRAMES * 2);
        slint::platform::update_timers_and_animations();
        assert!(model.decoding.borrow().frames.is_none());
        assert!(model.decoding.borrow().shown.is_none());
        FRAME_SWEEP.with(|timer| assert!(!timer.running()));
        assert_eq!(shown.size(), frames.row_data(0).unwrap().size());
    }

    #[test]
    fn animations_decode_the_frame_showing_and_let_it_go_when_idle() {
        i_slint_backend_testing::init_no_event_loop();
        let file = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testing/fixtures/gif-h264.mp4"),
        )
        .unwrap();
        let picture = Arc::new(talk_core::chat::decode_picture(&file, 32, 32).unwrap());
        let decoded: Vec<_> = picture.frames().collect();
        let (frames, delays) = animation(&picture);
        assert_eq!((frames.row_count(), delays.row_count()), (10, 10));
        let pixels = |row: usize| {
            let image = frames.row_data(row).unwrap();
            image.to_rgba8().unwrap().as_bytes().to_vec()
        };
        // In turn, out of turn, and around again from the first.
        for row in [0, 1, 1, 2, 7, 3, 0, 9] {
            assert_eq!(pixels(row), decoded[row], "frame {row}");
        }
        // A row built again shares them.
        assert!(frames == animation(&picture).0);
        // Idle, it gives up its decoder, and decodes again when it shows.
        let model = FRAMES.with_borrow(|cache| cache[0].upgrade().unwrap());
        i_slint_backend_testing::mock_elapsed_time(IDLE_FRAMES * 2);
        slint::platform::update_timers_and_animations();
        assert!(
            model.decoding.borrow().frames.is_none() && model.decoding.borrow().shown.is_none()
        );
        assert_eq!(pixels(4), decoded[4]);
    }
}
