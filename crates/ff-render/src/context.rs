use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
    /// Compiled transition pipelines, keyed by shader label.
    ///
    /// A transition node carries the incoming clip's pixels, so it is rebuilt for every
    /// frame of a transition -- the graph feeds `input[1..]` the *source* frame, not a
    /// caller texture, so there is nowhere else for those pixels to live. Compiling the
    /// shader per instance would therefore mean compiling it per frame. The pipeline
    /// depends only on the shader and the layout, never on the node's data, so it is
    /// cached here on the device that owns it instead (#1726).
    ///
    /// **The uniform buffer lives inside each cached entry, so nodes of one kind now
    /// share it.** That is safe as things stand: a `SceneRunner` owns its own compositor
    /// and therefore its own context, and the export drains on one thread. Two graphs
    /// driving the same kind on *one* context concurrently would interleave
    /// `write_buffer` and `submit` and draw each other's uniforms — if that ever becomes
    /// possible, the buffer has to come back out of the cache.
    ///
    /// The logic lives in `nodes::transition::cached_pipeline`; this is only the store.
    pub(crate) transition_pipelines: Mutex<
        HashMap<
            crate::nodes::transition::TransitionPipelineKey,
            Arc<crate::nodes::transition::TransitionPipeline>,
        >,
    >,
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
            transition_pipelines: Mutex::new(HashMap::new()),
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

    /// Number of distinct transition pipelines compiled on this device so far.
    ///
    /// The cache is what keeps a per-frame node from recompiling its shader, and a
    /// cache is only load-bearing if something checks it: this lets a test assert the
    /// count stays at one across a multi-frame transition rather than trust the
    /// `entry().or_insert_with` by construction (#1726).
    #[cfg(test)]
    pub(crate) fn transition_pipeline_count(&self) -> usize {
        match self.transition_pipelines.lock() {
            Ok(guard) => guard.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
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
            transition_pipelines: Mutex::new(HashMap::new()),
        })
    }
}
