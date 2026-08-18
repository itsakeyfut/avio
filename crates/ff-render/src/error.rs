use ff_format::{ErrorSeverity, MediaError};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("GPU device creation failed: {message}")]
    DeviceCreation { message: String },

    #[error("shader compile failed: {message}")]
    ShaderCompile { message: String },

    #[error("texture creation failed: width={width} height={height} reason={reason}")]
    TextureCreation {
        width: u32,
        height: u32,
        reason: String,
    },

    #[error("composite failed: {message}")]
    Composite { message: String },

    #[error("lut load failed: path={path} reason={reason}")]
    LutLoad { path: String, reason: String },

    #[error("unsupported pixel format: {format}")]
    UnsupportedFormat { format: String },

    #[error("gpu operation timed out: {operation}")]
    GpuTimeout { operation: String },

    #[error("ffmpeg error: {message} (code={code})")]
    Ffmpeg { code: i32, message: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl MediaError for RenderError {
    fn severity(&self) -> ErrorSeverity {
        match self {
            Self::Ffmpeg { .. } | Self::UnsupportedFormat { .. } => ErrorSeverity::Other,
            Self::DeviceCreation { .. }
            | Self::ShaderCompile { .. }
            | Self::TextureCreation { .. }
            | Self::Composite { .. }
            | Self::LutLoad { .. }
            | Self::GpuTimeout { .. }
            | Self::Io(_) => ErrorSeverity::Fatal,
        }
    }
}
