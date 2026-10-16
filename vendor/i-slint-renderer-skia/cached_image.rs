// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

use i_slint_core::graphics::{
    Image, ImageCacheKey, ImageInner, IntSize, OpaqueImage, OpaqueImageVTable, SharedImageBuffer,
    cache as core_cache,
};
use i_slint_core::items::ImageFit;
use i_slint_core::lengths::{LogicalSize, ScaleFactor};

struct SkiaCachedImage {
    image: skia_safe::Image,
    cache_key: ImageCacheKey,
}

i_slint_core::OpaqueImageVTable_static! {
    static SKIA_CACHED_IMAGE_VT for SkiaCachedImage
}

impl OpaqueImage for SkiaCachedImage {
    fn size(&self) -> IntSize {
        IntSize::new(self.image.width() as u32, self.image.height() as u32)
    }

    fn cache_key(&self) -> ImageCacheKey {
        self.cache_key.clone()
    }
}

pub(crate) fn as_skia_image(
    image: Image,
    target_size_fn: &dyn Fn() -> LogicalSize,
    image_fit: ImageFit,
    scale_factor: ScaleFactor,
    canvas: &skia_safe::Canvas,
    surface: Option<&dyn crate::Surface>,
) -> Option<skia_safe::Image> {
    let image_inner: &ImageInner = (&image).into();
    match image_inner {
        ImageInner::None => None,
        // Yapfast: a picture made in code, such as a video frame, which Slint
        // never caches, is read where it is rather than copied each time.
        ImageInner::EmbeddedImage { buffer, cache_key: ImageCacheKey::Invalid } => {
            borrowed_skia_image(buffer).or_else(|| image_buffer_to_skia_image(buffer))
        }
        ImageInner::EmbeddedImage { buffer, cache_key } => {
            let result = image_buffer_to_skia_image(buffer);
            if let Some(img) = result.as_ref() {
                core_cache::replace_cached_image(
                    cache_key.clone(),
                    ImageInner::BackendStorage(vtable::VRc::into_dyn(vtable::VRc::new(
                        SkiaCachedImage { image: img.clone(), cache_key: cache_key.clone() },
                    ))),
                )
            }
            result
        }
        ImageInner::Svg(svg) => {
            // Query target_width/height here again to ensure that changes will invalidate the item rendering cache.
            let target_size = i_slint_core::graphics::scalable_render_size(
                svg.size(),
                image_fit,
                target_size_fn() * scale_factor,
                scale_factor,
                Default::default(),
            )?;
            let pixels = match svg.render(Some(target_size)).ok()? {
                SharedImageBuffer::RGB8(_) => unreachable!(),
                SharedImageBuffer::RGBA8(_) => unreachable!(),
                SharedImageBuffer::RGBA8Premultiplied(pixels) => pixels,
            };

            let image_info = crate::image_info(
                skia_safe::ISize::new(pixels.width() as i32, pixels.height() as i32),
                skia_safe::ColorType::RGBA8888,
                skia_safe::AlphaType::Premul,
            );

            skia_safe::images::raster_from_data(
                &image_info,
                skia_safe::Data::new_copy(pixels.as_bytes()),
                pixels.width() as usize * 4,
            )
        }
        ImageInner::StaticTextures(_) => todo!(),
        ImageInner::BackendStorage(x) => {
            vtable::VRc::borrow(x).downcast::<SkiaCachedImage>().map(|x| x.image.clone())
        }
        ImageInner::BorrowedOpenGLTexture(texture) => {
            surface.and_then(|surface| surface.import_opengl_texture(canvas, texture))
        }
        ImageInner::NineSlice(n) => as_skia_image(
            n.image(),
            target_size_fn,
            ImageFit::Preserve,
            scale_factor,
            canvas,
            surface,
        ),
        #[cfg(any(feature = "unstable-wgpu-29", feature = "unstable-wgpu-30"))]
        ImageInner::WGPUTexture(any_wgpu_texture) => {
            surface.and_then(|surface| surface.import_wgpu_texture(canvas, any_wgpu_texture))
        }
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

std::thread_local! {
    /// Yapfast: each Skia image that reads a Slint buffer in place, with the
    /// buffer, which keeps the pixels alive and unchanged while the image
    /// lives: Slint copies a shared buffer before anyone writes to it.
    static BORROWED: core::cell::RefCell<Vec<(skia_safe::Image, SharedImageBuffer)>> =
        Default::default();
}

/// Yapfast: lets go of the buffers whose images nothing but [`BORROWED`] holds.
pub(crate) fn release_borrowed() {
    // An image whose count is one may go to another thread: skia-safe's
    // public way to ask whether anything else holds it.
    use skia_safe::ConditionallySend;
    BORROWED.with_borrow_mut(|borrowed| borrowed.retain(|(image, _)| !image.can_send()));
}

/// Yapfast: a Skia image reading an RGBA buffer's pixels in place.
fn borrowed_skia_image(buffer: &SharedImageBuffer) -> Option<skia_safe::Image> {
    let (pixels, alpha_type) = match buffer {
        SharedImageBuffer::RGBA8(pixels) => (pixels, skia_safe::AlphaType::Unpremul),
        SharedImageBuffer::RGBA8Premultiplied(pixels) => (pixels, skia_safe::AlphaType::Premul),
        SharedImageBuffer::RGB8(_) => return None,
    };
    let bytes = pixels.as_bytes();
    let alpha_type = if bytes.as_chunks::<4>().0.iter().all(|px| px[3] == 255) {
        skia_safe::AlphaType::Opaque
    } else {
        alpha_type
    };
    let image_info = crate::image_info(
        skia_safe::ISize::new(pixels.width() as i32, pixels.height() as i32),
        skia_safe::ColorType::RGBA8888,
        alpha_type,
    );
    // SAFETY: BORROWED keeps the buffer, and so these bytes, until nothing
    // else holds the image.
    let data = unsafe { skia_safe::Data::new_bytes(bytes) };
    let image =
        skia_safe::images::raster_from_data(&image_info, data, pixels.width() as usize * 4)?;
    release_borrowed();
    BORROWED.with_borrow_mut(|borrowed| borrowed.push((image.clone(), buffer.clone())));
    Some(image)
}

fn image_buffer_to_skia_image(buffer: &SharedImageBuffer) -> Option<skia_safe::Image> {
    // Report fully opaque images as such to Skia. This allows Skia to skip
    // reading destination pixels back when blending, which is particularly
    // important for the software renderer when it renders directly into
    // write-combined (uncached) frame buffer memory, where CPU reads are an
    // order of magnitude slower than writes. The scan happens once per image;
    // the resulting skia_safe::Image is cached.
    fn opaque_or(rgba_bytes: &[u8], alpha_type: skia_safe::AlphaType) -> skia_safe::AlphaType {
        if rgba_bytes.as_chunks::<4>().0.iter().all(|px| px[3] == 255) {
            skia_safe::AlphaType::Opaque
        } else {
            alpha_type
        }
    }

    let (data, bpl, size, color_type, alpha_type) = match buffer {
        SharedImageBuffer::RGB8(pixels) => {
            // RGB888 with one byte per component is not supported by Skia right now. Convert once to RGBA8 :-(
            let rgba = pixels
                .as_bytes()
                .chunks(3)
                .flat_map(|rgb| IntoIterator::into_iter([rgb[0], rgb[1], rgb[2], 255]))
                .collect::<Vec<u8>>();
            (
                skia_safe::Data::new_copy(&rgba),
                pixels.width() as usize * 4,
                pixels.size(),
                skia_safe::ColorType::RGBA8888,
                skia_safe::AlphaType::Opaque,
            )
        }
        SharedImageBuffer::RGBA8(pixels) => (
            skia_safe::Data::new_copy(pixels.as_bytes()),
            pixels.width() as usize * 4,
            pixels.size(),
            skia_safe::ColorType::RGBA8888,
            opaque_or(pixels.as_bytes(), skia_safe::AlphaType::Unpremul),
        ),
        SharedImageBuffer::RGBA8Premultiplied(pixels) => (
            skia_safe::Data::new_copy(pixels.as_bytes()),
            pixels.width() as usize * 4,
            pixels.size(),
            skia_safe::ColorType::RGBA8888,
            opaque_or(pixels.as_bytes(), skia_safe::AlphaType::Premul),
        ),
    };
    let image_info = crate::image_info(
        skia_safe::ISize::new(size.width as i32, size.height as i32),
        color_type,
        alpha_type,
    );
    skia_safe::images::raster_from_data(&image_info, data, bpl)
}
