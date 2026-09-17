//! GPU renderer behind the frame-publish seam (issue #27).
//!
//! [`SceneWorld::new_headless`] builds an explicit GPU plugin set — never
//! `DefaultPlugins` (its `PipelinedRenderingPlugin` breaks synchronous
//! readback) and never `bevy_winit`. Plugin order is load-bearing:
//! [`TaskPoolPlugin`](bevy_app::TaskPoolPlugin) first, then
//! [`FrameCountPlugin`](bevy_diagnostic::FrameCountPlugin) — render extraction
//! panics without the frame counter — then time, transform, assets, a
//! windowless [`WindowPlugin`](bevy_window::WindowPlugin), [`RenderPlugin`](bevy_render::RenderPlugin),
//! and the image/mesh/camera/light/pipeline/PBR stages. After adding plugins
//! the app is finalized once with [`App::finish`] + [`App::cleanup`]
//! (`App::update` never finalizes plugins; `RenderPlugin::finish` must unpack
//! the `RenderDevice`).
//!
//! The viewport camera renders into an offscreen [`Image`](bevy_image::Image)
//! target (`Bgra8UnormSrgb`, matching the BGRA8 seam straight into the
//! `IOSurface`; `COPY_SRC` added for readback). [`readback_frame`] copies that GPU texture into a persistent
//! `MAP_READ | COPY_DST` staging buffer, submits, then maps synchronously
//! (`map_async` + `Device::poll(wait_indefinitely)`) and reads directly from
//! the [`RenderApp`](bevy_render::RenderApp) world — no channels. Row stride
//! follows [`RenderDevice::align_copy_bytes_per_row`]; padding is stripped
//! before the bytes reach [`RenderFrame`](crate::RenderFrame).
//!
//! Metal requirement: Bevy panics inside plugin init when no GPU adapter
//! exists, so there is deliberately no feature gate and no runtime fallback —
//! Veronica is a macOS-only DCC and Metal is always present. (No CI needs
//! protecting.)

use bevy_app::App;
use bevy_asset::{Assets, Handle};
use bevy_ecs::prelude::*;
use bevy_image::Image;
use bevy_render::{
    RenderApp,
    render_asset::RenderAssets,
    render_resource::{
        Buffer, BufferAsyncError, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        Extent3d, MapMode, PollType, TexelCopyBufferInfo, TexelCopyBufferLayout,
        TexelCopyTextureInfo, TextureFormat, TextureUsages,
    },
    renderer::{RenderDevice, RenderQueue},
    texture::GpuImage,
};

use crate::{
    SceneError,
    render::{FRAME_BYTES_PER_PIXEL, FRAME_HEIGHT, FRAME_WIDTH, MAX_VIEWPORT_EDGE, RenderFrame},
};

/// Live extents of the offscreen target, in pixels.
///
/// Initialized to the [`FRAME_WIDTH`] x [`FRAME_HEIGHT`] contract in
/// [`SceneWorld::new_headless`](crate::SceneWorld::new_headless) and
/// re-targeted by
/// [`SceneWorld::set_viewport_size`](crate::SceneWorld::set_viewport_size);
/// [`readback_frame`] reads this (never the constants) so frames follow the
/// pane size.
#[derive(Debug, Clone, Copy, Resource)]
pub(crate) struct GpuViewportSize {
    /// Target width in pixels.
    pub width: u32,
    /// Target height in pixels.
    pub height: u32,
}

/// Handle of the offscreen target the viewport camera renders into.
///
/// Created once in [`SceneWorld::new_headless`](crate::SceneWorld::new_headless);
/// [`SceneWorld::spawn_base_scene`](crate::SceneWorld::spawn_base_scene) points
/// the camera at it. Always present, so a world with no camera reads back the
/// untouched (zeroed) texture deterministically.
#[derive(Debug, Clone, Resource)]
pub(crate) struct GpuFrameTarget {
    /// The `Bgra8UnormSrgb` target texture handle.
    pub handle: bevy_asset::Handle<Image>,
}

/// Persistent staging buffer for one padded frame.
///
/// Created lazily on the first [`readback_frame`] (a fallible context, so the
/// `u32` stride conversion can use `?` instead of panicking) and reused every
/// tick: map, copy the bytes out, unmap. The recorded extents detect a
/// viewport re-target that bypassed the drop in
/// [`SceneWorld::set_viewport_size`](crate::SceneWorld::set_viewport_size),
/// so the buffer is rebuilt rather than over-/under-read.
#[derive(Debug, Clone, Resource)]
pub(crate) struct GpuFrameStaging {
    /// `MAP_READ | COPY_DST` buffer holding one padded frame.
    pub buffer: Buffer,
    /// Width the buffer was allocated for.
    pub width: u32,
    /// Height the buffer was allocated for.
    pub height: u32,
}

/// Build the offscreen render-target image for the initial frame extents.
///
/// `Bgra8UnormSrgb` matches the BGRA8 seam so the `IOSurface` upload is a
/// plain memcpy with no swizzle; `COPY_SRC` is added to the default target
/// usages for the readback copy.
pub(crate) fn create_frame_target(images: &mut Assets<Image>) -> bevy_asset::Handle<Image> {
    create_frame_target_sized(images, FRAME_WIDTH, FRAME_HEIGHT)
}

/// Build the offscreen render-target image for explicit extents.
///
/// Callers must have validated `width`/`height` (nonzero,
/// `max <= MAX_VIEWPORT_EDGE`); the image constructor takes raw `u32` and
/// cannot fail on them.
pub(crate) fn create_frame_target_sized(
    images: &mut Assets<Image>,
    width: u32,
    height: u32,
) -> bevy_asset::Handle<Image> {
    debug_assert!(width > 0 && height > 0);
    debug_assert!(width.max(height) <= MAX_VIEWPORT_EDGE);
    let mut image = Image::new_target_texture(width, height, TextureFormat::Bgra8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    images.add(image)
}

/// Submit a texture-to-buffer copy, map it synchronously, and return the
/// (still padded) bytes.
///
/// Shared submit-and-map tail for [`readback_frame`] and
/// [`readback_target_bytes`]: explicit encoder (submitted after the frame's
/// own commands, so the GPU executes the copy after rendering), `map_async`
/// plus an indefinite device poll, direct read from the render world — no
/// channels.
///
/// # Errors
///
/// Returns the [`SceneError`] poll and map variants when the copy-back fails.
fn copy_texture_to_mapped_bytes(
    device: &RenderDevice,
    queue: &RenderQueue,
    label: &str,
    source: TexelCopyTextureInfo,
    buffer: &Buffer,
    bytes_per_row: u32,
    extent: Extent3d,
) -> Result<Vec<u8>, SceneError> {
    let mut encoder =
        device.create_command_encoder(&CommandEncoderDescriptor { label: Some(label) });
    encoder.copy_texture_to_buffer(
        source,
        TexelCopyBufferInfo {
            buffer,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(extent.height),
            },
        },
        extent,
    );
    let _submission = queue.submit([encoder.finish()]);

    let (sender, receiver) = std::sync::mpsc::channel::<Result<(), BufferAsyncError>>();
    buffer.slice(..).map_async(MapMode::Read, move |result| {
        drop(sender.send(result));
    });
    device
        .wgpu_device()
        .poll(PollType::wait_indefinitely())
        .map_err(|_| SceneError::DevicePollFailed)?;
    receiver
        .recv()
        .map_err(|_| SceneError::MapCallbackLost)?
        .map_err(|_| SceneError::MapFailed)?;

    let bytes = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    Ok(bytes)
}

/// Copy one full target frame off the GPU as tight row-major bytes.
///
/// Same synchronous map discipline as [`readback_frame`], but for an
/// explicit target at explicit extents (the pick pass's transient ID
/// target), with a fresh staging buffer per call — one frame-sized
/// allocation per tap is noise next to the frames the pick renders.
/// Channel order is the TARGET's native order (RGBA for the pick pass's
/// `Rgba8Unorm` target): the caller interprets the bytes, so no channel
/// contract is attached here. Padding rows are stripped, so the returned
/// bytes are tight.
///
/// # Errors
///
/// Returns [`SceneError::NoGpuImage`] when the target has no GPU image
/// yet, [`SceneError`] staging, poll, and map variants when the copy-back
/// fails.
pub(crate) fn readback_target_bytes(
    app: &mut App,
    target: &Handle<Image>,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, SceneError> {
    let (device, queue, gpu_image) = {
        let render_world = app.sub_app(RenderApp).world();
        (
            render_world.resource::<RenderDevice>().clone(),
            render_world.resource::<RenderQueue>().clone(),
            render_world
                .resource::<RenderAssets<GpuImage>>()
                .get(target)
                .cloned(),
        )
    };
    let Some(gpu_image) = gpu_image else {
        return Err(SceneError::NoGpuImage);
    };

    // `FRAME_BYTES_PER_PIXEL` doubles as the ID target's stride: both the
    // beauty (`Bgra8UnormSrgb`) and pick (`Rgba8Unorm`) targets are four
    // bytes per texel.
    let unpadded_bytes_per_row = width as usize * FRAME_BYTES_PER_PIXEL;
    let padded_bytes_per_row = RenderDevice::align_copy_bytes_per_row(unpadded_bytes_per_row);
    let padded_stride =
        u32::try_from(padded_bytes_per_row).map_err(|_| SceneError::StagingStrideOverflow)?;
    let staging = device.create_buffer(&BufferDescriptor {
        label: Some("veronica-target-staging"),
        size: padded_bytes_per_row as u64 * u64::from(height),
        usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let padded = copy_texture_to_mapped_bytes(
        &device,
        &queue,
        "veronica-target-readback",
        gpu_image.texture.as_image_copy(),
        &staging,
        padded_stride,
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    )?;
    let mut pixels = Vec::with_capacity(unpadded_bytes_per_row * height as usize);
    for row in padded.chunks_exact(padded_bytes_per_row) {
        pixels.extend_from_slice(&row[..unpadded_bytes_per_row]);
    }
    Ok(pixels)
}

/// Copy the last rendered frame off the GPU into a [`RenderFrame`].
///
/// Reads the [`GpuImage`] for the [`GpuFrameTarget`] out of the
/// [`RenderApp`](bevy_render::RenderApp) world, copies it to the staging
/// buffer with an explicit encoder (submitted after the frame's own commands,
/// so the GPU executes the copy after rendering), then maps synchronously.
/// Padding rows are stripped, so the returned bytes are tight BGRA8.
///
/// # Errors
///
/// Returns [`SceneError::NoGpuImage`] when the target has no GPU image
/// yet (read before the first tick uploaded it), [`SceneError`] staging, poll,
/// and map variants when the copy-back fails.
pub(crate) fn readback_frame(app: &mut App) -> Result<RenderFrame, SceneError> {
    let target = app.world().resource::<GpuFrameTarget>().handle.clone();
    let size = *app.world().resource::<GpuViewportSize>();
    let (device, queue, gpu_image) = {
        let render_world = app.sub_app(RenderApp).world();
        (
            render_world.resource::<RenderDevice>().clone(),
            render_world.resource::<RenderQueue>().clone(),
            render_world
                .resource::<RenderAssets<GpuImage>>()
                .get(&target)
                .cloned(),
        )
    };
    let Some(gpu_image) = gpu_image else {
        return Err(SceneError::NoGpuImage);
    };

    let unpadded_bytes_per_row = size.width as usize * FRAME_BYTES_PER_PIXEL;
    let padded_bytes_per_row = RenderDevice::align_copy_bytes_per_row(unpadded_bytes_per_row);
    let padded_stride =
        u32::try_from(padded_bytes_per_row).map_err(|_| SceneError::StagingStrideOverflow)?;
    let buffer_size = u64::from(padded_stride) * u64::from(size.height);

    let staging_matches = app
        .world()
        .get_resource::<GpuFrameStaging>()
        .is_some_and(|staging| staging.width == size.width && staging.height == size.height);
    if !staging_matches {
        if app.world().get_resource::<GpuFrameStaging>().is_some() {
            app.world_mut().remove_resource::<GpuFrameStaging>();
        }
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("veronica-frame-staging"),
            size: buffer_size,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        app.world_mut().insert_resource(GpuFrameStaging {
            buffer,
            width: size.width,
            height: size.height,
        });
    }
    let staging = app.world().resource::<GpuFrameStaging>().clone();

    let bytes = copy_texture_to_mapped_bytes(
        &device,
        &queue,
        "veronica-frame-readback",
        gpu_image.texture.as_image_copy(),
        &staging.buffer,
        padded_stride,
        Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
    )?;

    let mut pixels = Vec::with_capacity(unpadded_bytes_per_row * size.height as usize);
    for row in bytes.chunks_exact(padded_bytes_per_row) {
        pixels.extend_from_slice(&row[..unpadded_bytes_per_row]);
    }
    RenderFrame::from_raw_parts(size.width, size.height, pixels)
}
