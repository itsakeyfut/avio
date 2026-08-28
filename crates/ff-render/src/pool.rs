//! Format-and-usage-keyed pool of reusable GPU textures.
//!
//! Allocating and dropping a `wgpu::Texture` every frame is a per-frame GPU API
//! cost. [`TexturePool`] hands out textures keyed by
//! `(width, height, format, usage)` and takes them back for reuse, so a running
//! pipeline performs zero `create_texture` calls per frame once warmed up.
//!
//! A frame acquires its textures through a [`FrameScope`], which returns them all
//! to the pool when it drops, so an early return still reclaims them.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

/// Pool key: textures are only interchangeable when their size, format, and
/// usage all match.
type TextureKey = (u32, u32, wgpu::TextureFormat, wgpu::TextureUsages);

/// Reuse pool for GPU textures, keyed by size, format, and usage.
pub(crate) struct TexturePool {
    /// Free textures available for reuse, grouped by key.
    free: HashMap<TextureKey, Vec<wgpu::Texture>>,
    /// Number of textures actually created (pool misses). Read by tests to
    /// assert steady-state zero allocation.
    alloc_count: usize,
}

impl TexturePool {
    pub(crate) fn new() -> Self {
        Self {
            free: HashMap::new(),
            alloc_count: 0,
        }
    }

    /// Return a texture matching the key: reuse a freed one, or create it on a
    /// miss.
    pub(crate) fn acquire(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        if let Some(texture) = self
            .free
            .get_mut(&(width, height, format, usage))
            .and_then(Vec::pop)
        {
            return texture;
        }

        self.alloc_count += 1;
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ffrender.pool.texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }

    /// Return a texture to the pool for reuse under its key.
    pub(crate) fn release(&mut self, key: TextureKey, texture: wgpu::Texture) {
        self.free.entry(key).or_default().push(texture);
    }

    /// Number of textures created so far (pool misses).
    #[cfg(test)]
    pub(crate) fn alloc_count(&self) -> usize {
        self.alloc_count
    }
}

/// Lock the pool, recovering the guard if a previous holder panicked.
///
/// The pool never spans an `unwind`-capable operation while locked, so a
/// poisoned lock is a non-event: recover the inner guard rather than panicking.
fn lock(pool: &Mutex<TexturePool>) -> std::sync::MutexGuard<'_, TexturePool> {
    pool.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Owns the textures acquired during one frame and returns them all to the pool
/// on drop.
///
/// `acquire` returns an index rather than a reference so that further acquires
/// (which need `&mut self`) do not conflict with references handed out earlier;
/// resolve an index to a texture with [`get`](Self::get) once all acquires for
/// the frame are done.
pub(crate) struct FrameScope<'a> {
    pool: &'a Mutex<TexturePool>,
    device: &'a wgpu::Device,
    /// Textures acquired this frame, with the key each must be released under.
    items: Vec<(TextureKey, wgpu::Texture)>,
}

impl<'a> FrameScope<'a> {
    pub(crate) fn new(pool: &'a Mutex<TexturePool>, device: &'a wgpu::Device) -> Self {
        Self {
            pool,
            device,
            items: Vec::new(),
        }
    }

    /// Acquire a texture for this frame and return its index within the scope.
    pub(crate) fn acquire(
        &mut self,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> usize {
        let texture = lock(self.pool).acquire(self.device, width, height, format, usage);
        self.items.push(((width, height, format, usage), texture));
        self.items.len() - 1
    }

    /// Resolve an index returned by [`acquire`](Self::acquire) to its texture.
    pub(crate) fn get(&self, index: usize) -> &wgpu::Texture {
        &self.items[index].1
    }
}

impl Drop for FrameScope<'_> {
    fn drop(&mut self) {
        let mut pool = lock(self.pool);
        for (key, texture) in self.items.drain(..) {
            pool.release(key, texture);
        }
    }
}

#[cfg(all(test, feature = "wgpu"))]
mod tests {
    use super::TexturePool;

    /// A headless GPU device, or `None` when no adapter is available (CI).
    fn device() -> Option<wgpu::Device> {
        match futures::executor::block_on(crate::context::RenderContext::init()) {
            Ok(ctx) => Some(ctx.device),
            Err(_) => None,
        }
    }

    #[test]
    fn texture_pool_should_reuse_a_released_texture_for_the_same_key() {
        let Some(device) = device() else {
            return;
        };
        let mut pool = TexturePool::new();
        let key = (
            16,
            16,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        );

        let texture = pool.acquire(&device, key.0, key.1, key.2, key.3);
        assert_eq!(pool.alloc_count(), 1, "the first acquire is a pool miss");

        pool.release(key, texture);
        let _reused = pool.acquire(&device, key.0, key.1, key.2, key.3);
        assert_eq!(
            pool.alloc_count(),
            1,
            "releasing then re-acquiring the same key must reuse, not allocate"
        );
    }

    #[test]
    fn texture_pool_should_key_formats_distinctly() {
        let Some(device) = device() else {
            return;
        };
        let mut pool = TexturePool::new();
        let usage = wgpu::TextureUsages::COPY_DST;

        let rgba8 = pool.acquire(&device, 16, 16, wgpu::TextureFormat::Rgba8Unorm, usage);
        pool.release((16, 16, wgpu::TextureFormat::Rgba8Unorm, usage), rgba8);

        // A different format is a distinct key: it must not reuse the released
        // Rgba8Unorm texture.
        let _rgba16 = pool.acquire(&device, 16, 16, wgpu::TextureFormat::Rgba16Float, usage);
        assert_eq!(
            pool.alloc_count(),
            2,
            "a distinct format must allocate its own texture"
        );
    }
}
