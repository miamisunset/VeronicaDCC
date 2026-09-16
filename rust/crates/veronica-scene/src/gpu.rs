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
//! The demo camera renders into an offscreen [`Image`](bevy_image::Image)
//! target (`Rgba8UnormSrgb`, matching the RGBA8 seam; `COPY_SRC` added for
//! readback). [`readback_frame`] copies that GPU texture into a persistent
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
use bevy_asset::Assets;
use bevy_ecs::prelude::*;
use bevy_image::Image;
use bevy_render::{
    RenderApp,
    render_asset::RenderAssets,
    render_resource::{
        Buffer, BufferAsyncError, BufferDescriptor, BufferUsages, CommandEncoderDescriptor,
        Extent3d, MapMode, PollType, TexelCopyBufferInfo, TexelCopyBufferLayout, TextureFormat,
        TextureUsages,
    },
    renderer::{RenderDevice, RenderQueue},
    texture::GpuImage,
};

use crate::{
    SceneError,
    render::{FRAME_BYTES_PER_PIXEL, FRAME_HEIGHT, FRAME_WIDTH, RenderFrame},
};

/// Handle of the offscreen target the demo camera renders into.
///
/// Created once in [`SceneWorld::new_headless`](crate::SceneWorld::new_headless);
/// [`SceneWorld::spawn_demo_scene`](crate::SceneWorld::spawn_demo_scene) points
/// the camera at it. Always present, so a world with no camera reads back the
/// untouched (zeroed) texture deterministically.
#[derive(Debug, Clone, Resource)]
pub(crate) struct GpuFrameTarget {
    /// The `Rgba8UnormSrgb` target texture handle.
    pub handle: bevy_asset::Handle<Image>,
}

/// Persistent staging buffer for one padded frame.
///
/// Created lazily on the first [`readback_frame`] (a fallible context, so the
/// `u32` stride conversion can use `?` instead of panicking) and reused every
/// tick: map, copy the bytes out, unmap. The stride is recomputed from the
/// fixed frame constants on every readback, so the buffer needs no layout
/// metadata of its own.
#[derive(Debug, Clone, Resource)]
pub(crate) struct GpuFrameStaging {
    /// `MAP_READ | COPY_DST` buffer holding one padded frame.
    pub buffer: Buffer,
}

/// Build the offscreen render-target image for the fixed frame extents.
///
/// `Rgba8UnormSrgb` matches the RGBA8 seam so Swift's swizzle stays untouched;
/// `COPY_SRC` is added to the default target usages for the readback copy.
pub(crate) fn create_frame_target(images: &mut Assets<Image>) -> bevy_asset::Handle<Image> {
    let mut image = Image::new_target_texture(
        FRAME_WIDTH,
        FRAME_HEIGHT,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    images.add(image)
}

/// Copy the last rendered frame off the GPU into a [`RenderFrame`].
///
/// Reads the [`GpuImage`] for the [`GpuFrameTarget`] out of the
/// [`RenderApp`](bevy_render::RenderApp) world, copies it to the staging
/// buffer with an explicit encoder (submitted after the frame's own commands,
/// so the GPU executes the copy after rendering), then maps synchronously.
/// Padding rows are stripped, so the returned bytes are tight RGBA8.
///
/// # Errors
///
/// Returns [`SceneError::NoGpuImage`] when the target has no GPU image
/// yet (read before the first tick uploaded it), [`SceneError`] staging, poll,
/// and map variants when the copy-back fails.
pub(crate) fn readback_frame(app: &mut App) -> Result<RenderFrame, SceneError> {
    let target = app.world().resource::<GpuFrameTarget>().handle.clone();
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

    let unpadded_bytes_per_row = FRAME_WIDTH as usize * FRAME_BYTES_PER_PIXEL;
    let padded_bytes_per_row = RenderDevice::align_copy_bytes_per_row(unpadded_bytes_per_row);
    let padded_stride =
        u32::try_from(padded_bytes_per_row).map_err(|_| SceneError::StagingStrideOverflow)?;
    let buffer_size = u64::from(padded_stride) * u64::from(FRAME_HEIGHT);

    if app.world().get_resource::<GpuFrameStaging>().is_none() {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("veronica-frame-staging"),
            size: buffer_size,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        app.world_mut().insert_resource(GpuFrameStaging { buffer });
    }
    let staging = app.world().resource::<GpuFrameStaging>().clone();

    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("veronica-frame-readback"),
    });
    encoder.copy_texture_to_buffer(
        gpu_image.texture.as_image_copy(),
        TexelCopyBufferInfo {
            buffer: &*staging.buffer,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_stride),
                rows_per_image: Some(FRAME_HEIGHT),
            },
        },
        Extent3d {
            width: FRAME_WIDTH,
            height: FRAME_HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    let _submission = queue.submit([encoder.finish()]);

    let (sender, receiver) = std::sync::mpsc::channel::<Result<(), BufferAsyncError>>();
    staging
        .buffer
        .slice(..)
        .map_async(MapMode::Read, move |result| {
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

    let bytes = {
        let view = staging.buffer.slice(..).get_mapped_range();
        view.to_vec()
    };
    staging.buffer.unmap();

    let mut pixels = Vec::with_capacity(unpadded_bytes_per_row * FRAME_HEIGHT as usize);
    for row in bytes.chunks_exact(padded_bytes_per_row) {
        pixels.extend_from_slice(&row[..unpadded_bytes_per_row]);
    }
    RenderFrame::from_raw_parts(FRAME_WIDTH, FRAME_HEIGHT, pixels)
}
