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

    /// Upload one full RGBA8 frame under lock, swizzling to the surface's
    /// BGRA8 layout during the copy.
    ///
    /// # Errors
    ///
    /// Returns [`SurfaceError::BadPayload`] when `rgba` does not hold
    /// exactly `width * height * 4` bytes, [`SurfaceError::LockFailed`]
    /// when the surface will not lock.
    pub fn upload(&self, rgba: &[u8]) -> Result<(), SurfaceError> {
        let expected = self.width as usize * self.height as usize * 4;
        if rgba.len() != expected {
            return Err(SurfaceError::BadPayload {
                expected,
                actual: rgba.len(),
            });
        }
        // SAFETY: handle came from `IOSurfaceCreate`, alive until `drop`.
        let locked = unsafe { IOSurfaceLock(self.surface, 0, std::ptr::null_mut()) };
        if locked != KERN_SUCCESS {
            return Err(SurfaceError::LockFailed);
        }
        // SAFETY: locked above; base address and stride stay valid until
        // the matching unlock below. Rows copy one by one because the
        // framework may pad each row past `width * 4`; channels swizzle
        // RGBA source order into the surface's BGRA layout per pixel.
        unsafe {
            let base = IOSurfaceGetBaseAddress(self.surface).cast::<u8>();
            let stride = IOSurfaceGetBytesPerRow(self.surface);
            let row_bytes = self.width as usize * 4;
            for row in 0..self.height as usize {
                let dst_row = base.add(row * stride);
                let src_row = rgba.as_ptr().add(row * row_bytes);
                for pixel in 0..self.width as usize {
                    let src = src_row.add(pixel * 4);
                    let dst = dst_row.add(pixel * 4);
                    *dst = *src.add(2);
                    *dst.add(1) = *src.add(1);
                    *dst.add(2) = *src;
                    *dst.add(3) = *src.add(3);
                }
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
    fn upload_round_trips_through_the_mapping() {
        let surface = FrameSurface::new(8, 4).unwrap();
        let mut payload = vec![0u8; 8 * 4 * 4];
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte = u8::try_from(index % 251).unwrap_or(u8::MAX);
        }
        surface.upload(&payload).unwrap();
        // SAFETY: just uploaded; re-lock and compare the first row, then
        // unlock. Test-only read-back of our own surface. Bytes come back
        // swizzled: surface order is B, G, R, A against the RGBA payload.
        unsafe {
            assert_eq!(IOSurfaceLock(surface.handle(), 0, std::ptr::null_mut()), 0);
            let base = IOSurfaceGetBaseAddress(surface.handle()).cast::<u8>();
            let first_row = std::slice::from_raw_parts(base.cast_const(), 8 * 4).to_vec();
            IOSurfaceUnlock(surface.handle(), 0, std::ptr::null_mut());
            for pixel in 0..8 {
                assert_eq!(first_row[pixel * 4], payload[pixel * 4 + 2]);
                assert_eq!(first_row[pixel * 4 + 1], payload[pixel * 4 + 1]);
                assert_eq!(first_row[pixel * 4 + 2], payload[pixel * 4]);
                assert_eq!(first_row[pixel * 4 + 3], payload[pixel * 4 + 3]);
            }
        }
    }

    #[test]
    fn upload_swizzles_pure_red_to_bgra() {
        let surface = FrameSurface::new(2, 1).unwrap();
        let payload = [255u8, 0, 0, 255, 0, 255, 0, 255];
        surface.upload(&payload).unwrap();
        // SAFETY: just uploaded; re-lock, read both pixels, unlock.
        unsafe {
            assert_eq!(IOSurfaceLock(surface.handle(), 0, std::ptr::null_mut()), 0);
            let base = IOSurfaceGetBaseAddress(surface.handle()).cast::<u8>();
            let row = std::slice::from_raw_parts(base.cast_const(), 8).to_vec();
            IOSurfaceUnlock(surface.handle(), 0, std::ptr::null_mut());
            assert_eq!(&row[..4], &[0, 0, 255, 255]);
            assert_eq!(&row[4..], &[0, 255, 0, 255]);
        }
    }
}
