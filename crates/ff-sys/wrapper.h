// FFmpeg headers for bindgen
// This file includes the FFmpeg C headers needed for FFI bindings.

// Core utilities
#include <libavutil/avutil.h>
#include <libavutil/imgutils.h>
#include <libavutil/opt.h>
#include <libavutil/channel_layout.h>
#include <libavutil/samplefmt.h>
#include <libavutil/audio_fifo.h>
#include <libavutil/pixfmt.h>
#include <libavutil/rational.h>
#include <libavutil/error.h>
#include <libavutil/frame.h>
#include <libavutil/dict.h>
#include <libavutil/log.h>

// Format I/O
#include <libavformat/avformat.h>
#include <libavformat/avio.h>

// Codec
#include <libavcodec/avcodec.h>
#include <libavcodec/packet.h>
#include <libavcodec/bsf.h>

// Scaling
#include <libswscale/swscale.h>

// Resampling
#include <libswresample/swresample.h>

// Filter graphs
#include <libavfilter/avfilter.h>
#include <libavfilter/buffersink.h>
#include <libavfilter/buffersrc.h>

// Version headers (for FFmpeg 7.x compatibility checks)
#include <libavformat/version.h>
#include <libavcodec/version.h>
#include <libavutil/version.h>
