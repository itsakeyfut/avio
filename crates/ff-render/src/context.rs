use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::RenderError;
use crate::pool::TexturePool;

/// Owns the wgpu device and queue used by the render pipeline.
///
/// Share via `Arc<RenderContext>` when multiple components (graph, sink, etc.)
/// need access to the same GPU device.
pub struct RenderContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Shared reuse pool for GPU textures, so the graph (and, later, the
    /// compositor) avoid per-frame texture allocation.
    pub(crate) pool: Mutex<TexturePool>,
    /// Count of GPU-to-CPU readbacks performed (staging-buffer maps). The
    /// zero-copy display path never increments this; tests assert it stays flat.
    pub(crate) readback_count: AtomicU64,
}

impl RenderContext {
    /// Wrap an existing wgpu device (e.g. shared with the window renderer).
    #[must_use]
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            device,
            queue,
            pool: Mutex::new(TexturePool::new()),
            readback_count: AtomicU64::new(0),
        }
    }

    /// Record one GPU-to-CPU readback (called from the staging-buffer path).
    pub(crate) fn note_readback(&self) {
        self.readback_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Number of GPU-to-CPU readbacks performed so far.
    #[cfg(test)]
    pub(crate) fn readback_count(&self) -> u64 {
        self.readback_count.load(Ordering::Relaxed)
    }

    /// Initialise wgpu using the default (best available) backend.
    ///
    /// Backend priority: Metal → Vulkan → DX12 → WebGPU → OpenGL.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::DeviceCreation`] if no suitable adapter is found or
    /// the device request fails.
    pub async fn init() -> Result<Self, RenderError> {
        Self::init_with_backend(wgpu::Backends::all()).await
    }

    /// Blocking wrapper over [`init`](Self::init) for synchronous callers.
    ///
    /// The preview runner and the export path are synchronous, so the block-on
    /// executor is kept here in the GPU crate; callers stay executor-agnostic.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::DeviceCreation`] if no suitable adapter is found or
    /// the device request fails.
    pub fn init_blocking() -> Result<Self, RenderError> {
        futures::executor::block_on(Self::init())
    }

    /// Initialise wgpu with an explicit backend set.
    ///
    /// Useful in CI where only `wgpu::Backends::GL` may be available.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::DeviceCreation`] if no suitable adapter is found or
    /// the device request fails.
    pub async fn init_with_backend(backends: wgpu::Backends) -> Result<Self, RenderError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| RenderError::DeviceCreation {
                message: e.to_string(),
            })?;

        log::info!(
            "render adapter selected backend={:?} name={}",
            adapter.get_info().backend,
            adapter.get_info().name
        );

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("ff-render"),
                ..Default::default()
            })
            .await
            .map_err(|e| RenderError::DeviceCreation {
                message: e.to_string(),
            })?;

        Ok(Self {
            device,
            queue,
            pool: Mutex::new(TexturePool::new()),
            readback_count: AtomicU64::new(0),
        })
    }
}
