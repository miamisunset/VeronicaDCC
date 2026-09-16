//! `IOSurface` frame publishing (ADR-0001 slice 2).
//!
//! The context owns a ping-pong pair of fixed-size `IOSurface`s; every
//! [`vrn_tick`] renders the software frame and uploads it into the back
//! buffer under lock, then publishes it as the new front, and
//! [`vrn_frame_surface`] hands Swift the borrowed front handle plus
//! extents. Swift wraps it in an `MTLTexture` with zero copies on the
//! present path.
//!
//! All `unsafe` in this crate lives here and in the `vrn_*` exports:
//! raw CoreFoundation / `IOSurface` handles, each block with a `SAFETY`
//! justification. The surface is refcounted by the framework and released
//! in [`FrameSurface::drop`]; Swift never releases the borrowed handle.
//!
//! [`vrn_tick`]: crate::vrn_tick
//! [`vrn_frame_surface`]: crate::vrn_frame_surface

use std::ffi::{c_long, c_void};
use thiserror::Error;

/// BGRA8 `IOSurface` pixel format, matching `MTLPixelFormat.bgra8Unorm`.
const PIXEL_FORMAT_BGRA: i32 = 0x4247_5241;
/// `kCFNumberSInt32Type` for the `CFNumber` property values.
const CF_NUMBER_SINT32: i32 = 3;
/// `KERN_SUCCESS` from `<mach/kern_return.h>`.
const KERN_SUCCESS: i32 = 0;

type CFTypeRef = *const c_void;
type CFDictionaryRef = *const c_void;
type IOSurfaceRef = *mut c_void;

/// Opaque callback-table type: only its address is ever used.
#[repr(C)]
struct CFCallbackTable {
    _private: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    /// `kCFTypeDictionaryKeyCallBacks` — retained keys, CF-typed values.
    static kCFTypeDictionaryKeyCallBacks: CFCallbackTable;
    /// `kCFTypeDictionaryValueCallBacks` — retained values.
    static kCFTypeDictionaryValueCallBacks: CFCallbackTable;

    fn CFNumberCreate(
        allocator: CFTypeRef,
        number_type: i32,
        value_ptr: *const c_void,
    ) -> CFTypeRef;
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        num_values: c_long,
        key_callbacks: *const CFCallbackTable,
        value_callbacks: *const CFCallbackTable,
    ) -> CFDictionaryRef;
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    /// Property-list keys for [`IOSurfaceCreate`].
    ///
    /// [`IOSurfaceCreate`]: fn.IOSurfaceCreate
    static kIOSurfaceWidth: CFTypeRef;
    /// Property-list keys for [`IOSurfaceCreate`].
    ///
    /// [`IOSurfaceCreate`]: fn.IOSurfaceCreate
    static kIOSurfaceHeight: CFTypeRef;
    /// Property-list keys for [`IOSurfaceCreate`].
    ///
    /// [`IOSurfaceCreate`]: fn.IOSurfaceCreate
    static kIOSurfaceBytesPerElement: CFTypeRef;
    /// Property-list keys for [`IOSurfaceCreate`].
    ///
    /// [`IOSurfaceCreate`]: fn.IOSurfaceCreate
    static kIOSurfacePixelFormat: CFTypeRef;

    fn IOSurfaceCreate(properties: CFDictionaryRef) -> IOSurfaceRef;
    fn IOSurfaceLock(buffer: IOSurfaceRef, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceUnlock(buffer: IOSurfaceRef, options: u32, seed: *mut u32) -> i32;
    fn IOSurfaceGetBaseAddress(buffer: IOSurfaceRef) -> *mut c_void;
    fn IOSurfaceGetBytesPerRow(buffer: IOSurfaceRef) -> usize;
}

/// Errors for `IOSurface` frame publishing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SurfaceError {
    /// The framework refused to create the surface.
    #[error("IOSurface creation failed for {width}x{height}")]
    CreateFailed {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },
    /// The surface would not lock for CPU upload.
    #[error("IOSurface lock failed")]
    LockFailed,
    /// The frame payload does not fill the surface exactly.
    #[error("frame payload has {actual} bytes, expected {expected}")]
    BadPayload {
        /// Bytes the surface holds (`width * height * 4`).
        expected: usize,
        /// Bytes the caller supplied.
        actual: usize,
    },
}

/// One context-owned `IOSurface` plus its fixed extents.
///
/// Created lazily on the first tick and recreated whenever the published
/// frame extents drift (viewport re-target); the handle address changes on
/// recreation and Swift re-adopts it via its address-change path. Swift
/// borrows the handle for the context lifetime and must not release it.
#[derive(Debug)]
pub struct FrameSurface {
    surface: IOSurfaceRef,
    width: u32,
    height: u32,
}

impl FrameSurface {
    /// Create a BGRA8 surface of `width` x `height` pixels.
    ///
    /// # Errors
    ///
    /// Returns [`SurfaceError::CreateFailed`] when the framework refuses
    /// the allocation.
    pub fn new(width: u32, height: u32) -> Result<Self, SurfaceError> {
        let surface = create_surface(width, height)?;
        Ok(Self {
            surface,
            width,
            height,
        })
    }

    /// Borrowed `IOSurface` handle. Owned by the context; valid until the
    /// context is destroyed, never released by Swift.
    #[must_use]
    pub fn handle(&self) -> *mut c_void {
        self.surface
    }

    /// Surface width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Surface height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Upload one full BGRA8 frame under lock via row-stride memcpy.
    ///
    /// The payload is already in the surface's native BGRA8 layout (the GPU
    /// renders the offscreen target as `Bgra8UnormSrgb`), so each row copies
    /// verbatim with [`std::ptr::copy_nonoverlapping`]; no channel swizzle.
    ///
    /// # Errors
    ///
    /// Returns [`SurfaceError::BadPayload`] when `bgra` does not hold
    /// exactly `width * height * 4` bytes, [`SurfaceError::LockFailed`]
    /// when the surface will not lock.
    pub fn upload(&self, bgra: &[u8]) -> Result<(), SurfaceError> {
        let expected = self.width as usize * self.height as usize * 4;
        if bgra.len() != expected {
            return Err(SurfaceError::BadPayload {
                expected,
                actual: bgra.len(),
            });
        }
        // SAFETY: handle came from `IOSurfaceCreate`, alive until `drop`.
        let locked = unsafe { IOSurfaceLock(self.surface, 0, std::ptr::null_mut()) };
        if locked != KERN_SUCCESS {
            return Err(SurfaceError::LockFailed);
        }
        // SAFETY: locked above; base address and stride stay valid until
        // the matching unlock below. Rows copy one by one because the
        // framework may pad each row past `width * 4`; source and surface
        // share the BGRA8 layout, so the copy is a plain memcpy.
        unsafe {
            let base = IOSurfaceGetBaseAddress(self.surface).cast::<u8>();
            let stride = IOSurfaceGetBytesPerRow(self.surface);
            let row_bytes = self.width as usize * 4;
            for row in 0..self.height as usize {
                let dst_row = base.add(row * stride);
                let src_row = bgra.as_ptr().add(row * row_bytes);
                std::ptr::copy_nonoverlapping(src_row, dst_row, row_bytes);
            }
            IOSurfaceUnlock(self.surface, 0, std::ptr::null_mut());
        }
        Ok(())
    }
}

impl Drop for FrameSurface {
    fn drop(&mut self) {
        // SAFETY: handle came from `IOSurfaceCreate`; balanced release.
        unsafe {
            CFRelease(self.surface);
        }
    }
}

#[cfg(test)]
impl FrameSurface {
    /// Copy the surface contents as packed rows (test-only readback).
    ///
    /// Locks the surface, copies `width * 4` bytes per row (skipping any
    /// stride padding, whose bytes Rust never wrote), then unlocks. The
    /// ping-pong contract test uses this to prove consecutive fronts carry
    /// different frames. Returns an empty vector when the surface will
    /// not lock.
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        let row_bytes = self.width as usize * 4;
        let mut packed = vec![0u8; row_bytes * self.height as usize];
        // SAFETY: handle came from `IOSurfaceCreate`, alive until `drop`.
        let locked = unsafe { IOSurfaceLock(self.surface, 0, std::ptr::null_mut()) };
        if locked != KERN_SUCCESS {
            return Vec::new();
        }
        // SAFETY: locked above; base address and stride stay valid until
        // the matching unlock below. Only `width * 4` bytes per row are
        // copied, so unwritten stride padding never leaks into the result.
        unsafe {
            let base = IOSurfaceGetBaseAddress(self.surface).cast::<u8>();
            let stride = IOSurfaceGetBytesPerRow(self.surface);
            for (row, dst) in packed.chunks_exact_mut(row_bytes).enumerate() {
                let src = base.add(row * stride);
                std::ptr::copy_nonoverlapping(src, dst.as_mut_ptr(), row_bytes);
            }
            IOSurfaceUnlock(self.surface, 0, std::ptr::null_mut());
        }
        packed
    }
}

/// Build the property dictionary and create the surface.
fn create_surface(width: u32, height: u32) -> Result<IOSurfaceRef, SurfaceError> {
    let width_i32 = i32::try_from(width).unwrap_or(i32::MAX);
    let height_i32 = i32::try_from(height).unwrap_or(i32::MAX);
    // (width, height, bytes-per-element, pixel-format) value quadruple.
    let values = [width_i32, height_i32, 4, PIXEL_FORMAT_BGRA];
    // SAFETY: foreign statics are valid for the process lifetime; only
    // their addresses are read.
    let keys: [CFTypeRef; 4] = unsafe {
        [
            kIOSurfaceWidth,
            kIOSurfaceHeight,
            kIOSurfaceBytesPerElement,
            kIOSurfacePixelFormat,
        ]
    };
    let mut numbers: [CFTypeRef; 4] = [std::ptr::null(); 4];
    for (slot, value) in numbers.iter_mut().zip(values.iter()) {
        // SAFETY: `value` outlives the call; `CFNumberCreate` copies it.
        let number = unsafe {
            CFNumberCreate(
                std::ptr::null(),
                CF_NUMBER_SINT32,
                std::ptr::from_ref(value).cast::<c_void>(),
            )
        };
        if number.is_null() {
            release_all(&numbers);
            return Err(SurfaceError::CreateFailed { width, height });
        }
        *slot = number;
    }
    // SAFETY: keys are live statics, numbers were just created, callbacks
    // are process-lifetime statics; the dictionary retains what it needs.
    let properties = unsafe {
        CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            numbers.as_ptr(),
            c_long::try_from(numbers.len()).unwrap_or(c_long::MAX),
            std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks),
            std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks),
        )
    };
    release_all(&numbers);
    if properties.is_null() {
        return Err(SurfaceError::CreateFailed { width, height });
    }
    // SAFETY: properties is a live dictionary; `IOSurfaceCreate` retains it.
    let surface = unsafe { IOSurfaceCreate(properties) };
    // SAFETY: created above, still alive.
    unsafe {
        CFRelease(properties);
    }
    if surface.is_null() {
        return Err(SurfaceError::CreateFailed { width, height });
    }
    Ok(surface)
}

/// Release every non-null entry (partial cleanup on the failure paths).
fn release_all(objects: &[CFTypeRef]) {
    for object in objects {
        if !object.is_null() {
            // SAFETY: non-null entries came from `CFNumberCreate` above.
            unsafe {
                CFRelease(*object);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_creates_with_requested_extents() {
        let surface = FrameSurface::new(64, 48).unwrap();
        assert!(!surface.handle().is_null());
        assert_eq!(surface.width(), 64);
        assert_eq!(surface.height(), 48);
    }

    #[test]
    fn short_payload_is_an_error() {
        let surface = FrameSurface::new(64, 48).unwrap();
        assert_eq!(
            surface.upload(&[0u8; 10]),
            Err(SurfaceError::BadPayload {
                expected: 64 * 48 * 4,
                actual: 10
            })
        );
    }

    #[test]
    fn oversized_payload_is_an_error() {
        // Arrange.
        let surface = FrameSurface::new(4, 4).unwrap();
        let payload = vec![0u8; 4 * 4 * 4 + 1];
        // Act + assert.
        assert_eq!(
            surface.upload(&payload),
            Err(SurfaceError::BadPayload {
                expected: 4 * 4 * 4,
                actual: 4 * 4 * 4 + 1
            })
        );
    }

    #[test]
    fn upload_copies_bgra_payload_verbatim() {
        // Arrange: BGRA payload with every byte distinct per pixel, over
        // multiple rows so stride handling is exercised.
        let surface = FrameSurface::new(8, 4).unwrap();
        let mut payload = vec![0u8; 8 * 4 * 4];
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte = u8::try_from(index % 251).unwrap_or(u8::MAX);
        }
        // Act.
        surface.upload(&payload).unwrap();
        // Assert: packed readback skips stride padding, so identity proves
        // the per-row memcpy wrote every payload byte untouched.
        assert_eq!(surface.snapshot_bytes(), payload);
    }

    #[test]
    fn upload_preserves_channel_order_on_primary_colors() {
        // Arrange: BGRA-order primaries — red is (0, 0, 255, 255) in BGRA.
        // A stale RGBA→BGRA swizzle would turn this payload blue.
        let surface = FrameSurface::new(2, 1).unwrap();
        let payload = [0u8, 0, 255, 255, 0, 255, 0, 255];
        // Act.
        surface.upload(&payload).unwrap();
        // Assert.
        assert_eq!(surface.snapshot_bytes(), payload);
    }

    #[test]
    fn upload_round_trips_odd_width_past_stride_padding() {
        // Arrange: odd width stresses row-stride padding (framework rows
        // may be wider than `width * 4`); distinct channels per pixel so a
        // swizzle or a row-offset bug cannot hide.
        let surface = FrameSurface::new(7, 3).unwrap();
        let payload: Vec<u8> = (0..7 * 3)
            .flat_map(|pixel| {
                let base = u8::try_from(pixel % 251).unwrap_or(u8::MAX);
                [base, base.wrapping_add(1), base.wrapping_add(2), 255]
            })
            .collect();
        // Act.
        surface.upload(&payload).unwrap();
        // Assert.
        assert_eq!(surface.snapshot_bytes(), payload);
    }
}
