use std::time::Duration;

use ff_preview::FrameSink;

use crate::graph::RenderGraph;

// TextureHandle

/// A GPU texture together with its default view and dimensions.
///
/// Window systems (winit, egui) can blit this directly to the display
/// surface without a CPU round-trip download.
#[cfg(feature = "wgpu")]
pub struct TextureHandle {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

// GpuFrameSink

/// A [`FrameSink`] that processes each frame through a [`RenderGraph`] before
/// forwarding to a downstream sink.
///
/// When the `wgpu` feature is enabled and the graph was created with a GPU
/// context, the GPU pipeline runs. On GPU error the unprocessed frame is
/// forwarded as a fallback.
///
/// When the `wgpu` feature is **not** enabled (or the graph is CPU-only), the
/// CPU fallback pipeline runs transparently.
///
/// # Example
///
/// ```ignore
/// let ctx = Arc::new(RenderContext::init().await?);
/// let graph = RenderGraph::new(ctx)
///     .push(ColorGradeNode { brightness: 0.2, ..Default::default() });
/// let sink = GpuFrameSink::new(graph, Box::new(RgbaSink::new()));
/// runner.set_sink(Box::new(sink));
/// ```
pub struct GpuFrameSink {
    graph: RenderGraph,
    downstream: Box<dyn FrameSink>,
}

impl GpuFrameSink {
    /// Construct a sink that applies `graph` to every incoming frame and
    /// forwards the result to `downstream`.
    #[must_use]
    pub fn new(graph: RenderGraph, downstream: Box<dyn FrameSink>) -> Self {
        Self { graph, downstream }
    }
}

impl FrameSink for GpuFrameSink {
    fn push_frame(&mut self, rgba: &[u8], width: u32, height: u32, pts: Duration) {
        #[cfg(feature = "wgpu")]
        {
            match self.graph.process_gpu(rgba, width, height) {
                Ok(processed) => {
                    self.downstream.push_frame(&processed, width, height, pts);
                    return;
                }
                Err(e) => {
                    log::warn!("GpuFrameSink GPU processing failed, using CPU fallback error={e}");
                }
            }
        }
        // CPU fallback (also used when wgpu feature is disabled).
        let processed = self.graph.process_cpu(rgba, width, height);
        self.downstream.push_frame(&processed, width, height, pts);
    }

    fn flush(&mut self) {
        self.downstream.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::nodes::ColorGradeNode;

    struct CollectSink(Arc<Mutex<Vec<Vec<u8>>>>);

    impl FrameSink for CollectSink {
        fn push_frame(&mut self, rgba: &[u8], _w: u32, _h: u32, _pts: Duration) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(rgba.to_vec());
        }
    }

    #[test]
    fn gpu_frame_sink_cpu_path_should_forward_processed_frame() {
        // Use a CPU-only graph so no GPU device is required.
        let graph = RenderGraph::new_cpu().push_cpu(ColorGradeNode::new(0.5, 1.0, 1.0, 0.0, 0.0));

        let collected = Arc::new(Mutex::new(Vec::new()));
        let downstream = Box::new(CollectSink(Arc::clone(&collected)));
        let mut sink = GpuFrameSink::new(graph, downstream);

        let pts = Duration::from_millis(0);
        // When wgpu feature is enabled, process_gpu will fail (no ctx) and
        // fall back to process_cpu — which is what we want for this test.
        sink.push_frame(&[128u8, 128, 128, 255], 1, 1, pts);

        let guard = collected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(guard.len(), 1, "exactly one frame must be forwarded");
        assert!(
            guard[0][0] > 128,
            "brightness +0.5 must increase R channel; got {}",
            guard[0][0]
        );
    }

    #[test]
    fn gpu_frame_sink_flush_should_propagate_to_downstream() {
        struct FlushTracker(Arc<Mutex<bool>>);
        impl FrameSink for FlushTracker {
            fn push_frame(&mut self, _: &[u8], _: u32, _: u32, _: Duration) {}
            fn flush(&mut self) {
                *self
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            }
        }

        let flushed = Arc::new(Mutex::new(false));
        let mut sink = GpuFrameSink::new(
            RenderGraph::new_cpu(),
            Box::new(FlushTracker(Arc::clone(&flushed))),
        );
        sink.flush();
        assert!(
            *flushed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            "flush must propagate to downstream"
        );
    }

    #[test]
    fn gpu_frame_sink_should_be_send() {
        fn assert_send<T: Send>() {}
        assert_send::<GpuFrameSink>();
    }
}
