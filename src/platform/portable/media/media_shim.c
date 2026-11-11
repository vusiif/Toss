/*
 * The portable media decode shim: on targets with no system media
 * framework, everything FFmpeg does for Toss happens in this file, and what
 * leaves it is plain C data — a session handle, a description, and chunks
 * of bytes Toss can point at.
 *
 * Built by build.rs (`build_ffmpeg_unix`) with the system C compiler
 * against third_party/ffmpeg's configured headers; called from
 * src/platform/portable/media/decoder.rs and from nowhere else. It is the
 * sibling of the Windows HEVC shim, wider because here FFmpeg is the whole
 * stack rather than a fallback: one picture stream and one sound stream
 * per file, each optional.
 *
 * What comes out is fixed, so nothing above this file converts anything:
 *   - pictures: BGRA8, at the size last asked for with
 *     `toss_media_set_size` (the stream's own display size until then);
 *   - sound: interleaved signed 16-bit, TOSS_MEDIA_CHANNELS channels at
 *     TOSS_MEDIA_RATE Hz, whatever the file holds.
 *
 * Ownership: one session = one open file. `toss_media_open` hands back an
 * opaque session the caller passes to `toss_media_close` exactly once. The
 * data pointer in a chunk belongs to the session and stays valid until the
 * next `toss_media_next`, `toss_media_seek` or the close. One thread at a
 * time per session; FFmpeg's own decode threads stay inside.
 *
 * Units: every `*_100ns` number is 100-nanosecond units, counted from the
 * start of the presentation — the same unit the Windows player speaks.
 *
 * Error classes are a stable contract with decoder.rs:
 *   -1  container/format not recognised      -> TossError::UnsupportedFormat
 *   -2  nothing in the file this build plays -> TossError::UnsupportedFormat
 *   -3  bytes recognised but damaged         -> TossError::CorruptInput
 *   -4  internal failure                     -> TossError::other
 */
#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/avutil.h>
#include <libavutil/channel_layout.h>
#include <libavutil/error.h>
#include <libavutil/log.h>
#include <libavutil/mathematics.h>
#include <libavutil/pixfmt.h>
#include <libavutil/samplefmt.h>
#include <libswresample/swresample.h>
#include <libswscale/swscale.h>

#include <limits.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>

#define TOSS_MEDIA_E_RECOGNIZE (-1)
#define TOSS_MEDIA_E_NOTHING (-2)
#define TOSS_MEDIA_E_DATA (-3)
#define TOSS_MEDIA_E_INTERNAL (-4)

#define TOSS_MEDIA_RATE 48000
#define TOSS_MEDIA_CHANNELS 2

#define TOSS_MEDIA_VIDEO 1
#define TOSS_MEDIA_AUDIO 2

/* Damaged packets in a row a session plays through before it calls the
 * file corrupt. A player that stops at the first bad packet is useless on
 * exactly the half-downloaded file somebody needs to check. */
#define TOSS_MEDIA_MAX_BAD_RUN 64

static const AVRational TOSS_100NS_Q = { 1, 10000000 };

typedef struct TossMediaInfo {
    int has_video; /* a picture stream is open and will produce chunks */
    int has_audio; /* a sound stream is open and will produce chunks */
    int skipped_video; /* a picture stream exists but this build cannot decode it */
    int skipped_audio; /* likewise for sound */
    int width; /* display size of the picture, sample aspect applied */
    int height;
    long long duration_100ns; /* -1 when the container does not say */
} TossMediaInfo;

typedef struct TossMediaChunk {
    int kind; /* TOSS_MEDIA_VIDEO or TOSS_MEDIA_AUDIO */
    int bytes; /* length of `data` */
    int width; /* video: picture size and bytes per row */
    int height;
    int stride;
    int frames; /* audio: sample frames in `data` */
    long long pts_100ns;
    long long duration_100ns;
    const unsigned char *data;
} TossMediaChunk;

typedef struct TossStream {
    AVCodecContext *ctx; /* NULL when the stream is absent or disabled */
    int index;
    int done; /* the decoder has returned end-of-stream after a flush */
    long long last_pts;
    long long last_dur;
} TossStream;

typedef struct TossMedia {
    AVFormatContext *fmt;
    AVPacket *pkt;
    AVFrame *frame;
    TossStream video;
    TossStream audio;
    int skipped_video;
    int skipped_audio;

    struct SwsContext *sws;
    unsigned char *picture;
    size_t picture_capacity;
    int src_w, src_h, src_fmt; /* what `sws` was built for */
    int dst_w, dst_h; /* what `sws` produces */
    int want_w, want_h; /* asked for by the caller; 0 = display size */

    struct SwrContext *swr;
    unsigned char *sound;
    size_t sound_capacity;
    int swr_rate, swr_fmt, swr_channels; /* what `swr` was built for */

    TossStream *draining; /* the decoder that may still hold output */
    int flushing; /* the file is read to its end; decoders are being emptied */
    int finished;
    long produced;
    int bad_run;
} TossMedia;

/* Error-level av_log messages on the calling thread, counted and never
 * printed: stderr carries Toss's words, not a library's. The count is what
 * tells "this file has nothing playable" apart from "this file is broken".
 * Per-thread because test sessions run in parallel. */
static __thread int tls_error_logs;

static void count_errors(void *ptr, int level, const char *fmt, va_list vl)
{
    (void)ptr;
    (void)fmt;
    (void)vl;
    if (level <= AV_LOG_ERROR) {
        tls_error_logs++;
    }
}

/* Called once from Rust, under a `Once`, before any session exists:
 * `av_log_set_callback` writes a process global. */
void toss_media_init(void)
{
    av_log_set_callback(count_errors);
}

/* ALSA reports problems through a printf-style callback that defaults to
 * stderr. Rust cannot define a variadic function, so the silent handler
 * the audio output installs lives here. */
void toss_media_quiet(const char *file, int line, const char *function, int err,
                      const char *fmt, ...)
{
    (void)file;
    (void)line;
    (void)function;
    (void)err;
    (void)fmt;
}

void toss_media_close(TossMedia *h);

static long long presentation_start(const TossMedia *h)
{
    return h->fmt->start_time == AV_NOPTS_VALUE ? 0 : h->fmt->start_time;
}

/* Open the decoder for stream `index` into `out`. Returns 1 when it is
 * open, 0 when this build has no decoder for it, <0 on an internal error. */
static int open_stream(TossMedia *h, int index, TossStream *out, int threads)
{
    const AVStream *st = h->fmt->streams[index];
    const AVCodec *dec = avcodec_find_decoder(st->codecpar->codec_id);
    if (!dec) {
        return 0;
    }
    AVCodecContext *ctx = avcodec_alloc_context3(dec);
    if (!ctx) {
        return TOSS_MEDIA_E_INTERNAL;
    }
    if (avcodec_parameters_to_context(ctx, st->codecpar) < 0) {
        avcodec_free_context(&ctx);
        return 0;
    }
    ctx->pkt_timebase = st->time_base;
    /* 0 lets FFmpeg pick from the machine: software 4K needs every core. */
    ctx->thread_count = threads;
    if (avcodec_open2(ctx, dec, NULL) < 0) {
        avcodec_free_context(&ctx);
        return 0;
    }
    out->ctx = ctx;
    out->index = index;
    return 1;
}

TossMedia *toss_media_open(const char *utf8_path, int *err_out)
{
    *err_out = 0;
    tls_error_logs = 0;

    TossMedia *h = (TossMedia *)calloc(1, sizeof(TossMedia));
    if (!h) {
        *err_out = TOSS_MEDIA_E_INTERNAL;
        return NULL;
    }
    h->video.index = -1;
    h->audio.index = -1;

    if (avformat_open_input(&h->fmt, utf8_path, NULL, NULL) < 0) {
        /* Bytes no demuxer claims fail the probe without a word. A demuxer
         * that claimed them and then could not read its own header says so
         * (an mp4 cut before its index: "moov atom not found") — that file
         * is damaged, not foreign. */
        *err_out = tls_error_logs > 0 ? TOSS_MEDIA_E_DATA : TOSS_MEDIA_E_RECOGNIZE;
        goto fail;
    }
    if (avformat_find_stream_info(h->fmt, NULL) < 0) {
        *err_out = tls_error_logs > 0 ? TOSS_MEDIA_E_DATA : TOSS_MEDIA_E_RECOGNIZE;
        goto fail;
    }

    int vi = av_find_best_stream(h->fmt, AVMEDIA_TYPE_VIDEO, -1, -1, NULL, 0);
    /* Cover art in a music file is a still, not a film: it must not turn
     * an mp3 into a one-frame video. */
    if (vi >= 0 && (h->fmt->streams[vi]->disposition & AV_DISPOSITION_ATTACHED_PIC)) {
        vi = -1;
    }
    int ai = av_find_best_stream(h->fmt, AVMEDIA_TYPE_AUDIO, -1, vi, NULL, 0);

    if (vi >= 0) {
        int opened = open_stream(h, vi, &h->video, 0);
        if (opened < 0) {
            *err_out = opened;
            goto fail;
        }
        h->skipped_video = !opened;
    }
    if (ai >= 0) {
        int opened = open_stream(h, ai, &h->audio, 1);
        if (opened < 0) {
            *err_out = opened;
            goto fail;
        }
        h->skipped_audio = !opened;
    }
    if (!h->video.ctx && !h->audio.ctx) {
        /* Recognised but damaged: errors were logged while looking.
         * Otherwise a clean file that simply holds nothing this build
         * decodes. */
        *err_out = (tls_error_logs > 0 && !h->skipped_video && !h->skipped_audio)
                       ? TOSS_MEDIA_E_DATA
                       : TOSS_MEDIA_E_NOTHING;
        goto fail;
    }

    /* Every stream nobody decodes is dropped at the demuxer. */
    for (unsigned i = 0; i < h->fmt->nb_streams; i++) {
        if ((int)i != h->video.index && (int)i != h->audio.index) {
            h->fmt->streams[i]->discard = AVDISCARD_ALL;
        }
    }

    h->pkt = av_packet_alloc();
    h->frame = av_frame_alloc();
    if (!h->pkt || !h->frame) {
        *err_out = TOSS_MEDIA_E_INTERNAL;
        goto fail;
    }
    return h;

fail:
    toss_media_close(h);
    return NULL;
}

/* The picture's size as it should be shown: coded size with the sample
 * aspect ratio applied to the width, so anamorphic video is not squeezed. */
static void display_size(const TossMedia *h, int *width, int *height)
{
    const AVCodecContext *ctx = h->video.ctx;
    const AVStream *st = h->fmt->streams[h->video.index];
    int w = ctx->width;
    int h_ = ctx->height;
    AVRational sar = st->sample_aspect_ratio.num > 0 ? st->sample_aspect_ratio
                                                     : ctx->sample_aspect_ratio;
    if (sar.num > 0 && sar.den > 0 && sar.num != sar.den) {
        long long scaled = av_rescale(w, sar.num, sar.den);
        if (scaled > 0 && scaled <= 65536) {
            w = (int)scaled;
        }
    }
    *width = w;
    *height = h_;
}

void toss_media_info(const TossMedia *h, TossMediaInfo *out)
{
    memset(out, 0, sizeof(*out));
    out->has_video = h->video.ctx != NULL;
    out->has_audio = h->audio.ctx != NULL;
    out->skipped_video = h->skipped_video;
    out->skipped_audio = h->skipped_audio;
    if (h->video.ctx) {
        display_size(h, &out->width, &out->height);
    }
    out->duration_100ns = h->fmt->duration > 0
                              ? av_rescale_q(h->fmt->duration, AV_TIME_BASE_Q, TOSS_100NS_Q)
                              : -1;
}

/* Stop decoding one kind of stream for the rest of the session: a film
 * with no display plays as sound, a machine with no sound device plays
 * silent, and neither should pay for decoding what nobody receives. */
void toss_media_disable(TossMedia *h, int kind)
{
    TossStream *stream = kind == TOSS_MEDIA_VIDEO ? &h->video : &h->audio;
    if (!stream->ctx) {
        return;
    }
    if (h->draining == stream) {
        h->draining = NULL;
    }
    h->fmt->streams[stream->index]->discard = AVDISCARD_ALL;
    avcodec_free_context(&stream->ctx);
    stream->index = -1;
}

/* The size pictures are produced at from the next chunk on; 0x0 returns to
 * the stream's display size. The scaler does the conversion and the resize
 * in one pass, which is what keeps a 4K film watchable in a small window. */
void toss_media_set_size(TossMedia *h, int width, int height)
{
    if (width <= 0 || height <= 0 || width > 16384 || height > 16384) {
        width = 0;
        height = 0;
    }
    h->want_w = width;
    h->want_h = height;
}

static long long stream_pts(TossMedia *h, TossStream *stream)
{
    const AVStream *st = h->fmt->streams[stream->index];
    long long raw = h->frame->best_effort_timestamp != AV_NOPTS_VALUE
                        ? h->frame->best_effort_timestamp
                        : h->frame->pts;
    if (raw == AV_NOPTS_VALUE) {
        /* No timestamp: this one follows the previous one. */
        return av_sat_add64(stream->last_pts, stream->last_dur > 0 ? stream->last_dur : 0);
    }
    long long start = av_rescale_q(presentation_start(h), AV_TIME_BASE_Q, st->time_base);
    long long pts = av_rescale_q(av_sat_sub64(raw, start), st->time_base, TOSS_100NS_Q);
    return pts < 0 ? 0 : pts;
}

static long long video_duration(TossMedia *h)
{
    const AVStream *st = h->fmt->streams[h->video.index];
    if (h->frame->duration > 0) {
        long long duration = av_rescale_q(h->frame->duration, st->time_base, TOSS_100NS_Q);
        if (duration > 0) {
            return duration;
        }
    }
    if (st->avg_frame_rate.num > 0 && st->avg_frame_rate.den > 0) {
        return av_rescale(10000000, st->avg_frame_rate.den, st->avg_frame_rate.num);
    }
    return h->video.last_dur > 0 ? h->video.last_dur : 400000; /* 25 fps */
}

/* Convert the picture in h->frame to BGRA at the wanted size. */
static int publish_video(TossMedia *h, TossMediaChunk *out)
{
    int dst_w = h->want_w;
    int dst_h = h->want_h;
    if (dst_w <= 0 || dst_h <= 0) {
        display_size(h, &dst_w, &dst_h);
    }
    if (h->frame->width <= 0 || h->frame->height <= 0 || dst_w <= 0 || dst_h <= 0 ||
        dst_w > 16384 || dst_h > 16384) {
        return TOSS_MEDIA_E_DATA;
    }

    if (!h->sws || h->src_w != h->frame->width || h->src_h != h->frame->height ||
        h->src_fmt != h->frame->format || h->dst_w != dst_w || h->dst_h != dst_h) {
        size_t bytes = (size_t)dst_w * 4u * (size_t)dst_h;
        if (bytes > (size_t)INT_MAX) {
            return TOSS_MEDIA_E_INTERNAL;
        }
        if (bytes > h->picture_capacity) {
            unsigned char *fresh = (unsigned char *)realloc(h->picture, bytes);
            if (!fresh) {
                return TOSS_MEDIA_E_INTERNAL;
            }
            h->picture = fresh;
            h->picture_capacity = bytes;
        }
        sws_freeContext(h->sws);
        h->sws = sws_getContext(h->frame->width, h->frame->height,
                                (enum AVPixelFormat)h->frame->format, dst_w, dst_h,
                                AV_PIX_FMT_BGRA, SWS_BILINEAR, NULL, NULL, NULL);
        if (!h->sws) {
            return TOSS_MEDIA_E_INTERNAL;
        }
        h->src_w = h->frame->width;
        h->src_h = h->frame->height;
        h->src_fmt = h->frame->format;
        h->dst_w = dst_w;
        h->dst_h = dst_h;
    }

    unsigned char *dst_data[4] = { h->picture, NULL, NULL, NULL };
    int dst_linesize[4] = { dst_w * 4, 0, 0, 0 };
    if (sws_scale(h->sws, (const uint8_t *const *)h->frame->data, h->frame->linesize, 0,
                  h->frame->height, dst_data, dst_linesize) != dst_h) {
        return TOSS_MEDIA_E_INTERNAL;
    }

    out->kind = TOSS_MEDIA_VIDEO;
    out->width = dst_w;
    out->height = dst_h;
    out->stride = dst_w * 4;
    out->bytes = dst_w * 4 * dst_h;
    out->frames = 0;
    out->pts_100ns = stream_pts(h, &h->video);
    out->duration_100ns = video_duration(h);
    out->data = h->picture;
    h->video.last_pts = out->pts_100ns;
    h->video.last_dur = out->duration_100ns;
    return 1;
}

/* Convert the samples in h->frame to interleaved S16 at the fixed rate and
 * layout. Returns 1 with a chunk, 0 when the resampler kept everything for
 * later, <0 on failure. */
static int sound_room(TossMedia *h, int frames);
static void sound_chunk(TossMediaChunk *out, unsigned char *data, int produced, long long pts);

static int publish_audio(TossMedia *h, TossMediaChunk *out)
{
    const AVFrame *frame = h->frame;
    int channels = frame->ch_layout.nb_channels;
    if (frame->nb_samples <= 0 || frame->sample_rate <= 0 || channels <= 0) {
        return TOSS_MEDIA_E_DATA;
    }

    if (!h->swr || h->swr_rate != frame->sample_rate || h->swr_fmt != frame->format ||
        h->swr_channels != channels) {
        AVChannelLayout in_layout;
        AVChannelLayout out_layout;
        /* A file that states only a channel count gets the usual layout for
         * that count; one that states a real layout keeps it. */
        if (frame->ch_layout.order == AV_CHANNEL_ORDER_UNSPEC) {
            av_channel_layout_default(&in_layout, channels);
        } else if (av_channel_layout_copy(&in_layout, &frame->ch_layout) < 0) {
            return TOSS_MEDIA_E_INTERNAL;
        }
        av_channel_layout_default(&out_layout, TOSS_MEDIA_CHANNELS);
        swr_free(&h->swr);
        int ret = swr_alloc_set_opts2(&h->swr, &out_layout, AV_SAMPLE_FMT_S16, TOSS_MEDIA_RATE,
                                      &in_layout, (enum AVSampleFormat)frame->format,
                                      frame->sample_rate, 0, NULL);
        av_channel_layout_uninit(&in_layout);
        av_channel_layout_uninit(&out_layout);
        if (ret < 0 || swr_init(h->swr) < 0) {
            swr_free(&h->swr);
            return TOSS_MEDIA_E_INTERNAL;
        }
        h->swr_rate = frame->sample_rate;
        h->swr_fmt = frame->format;
        h->swr_channels = channels;
    }

    int capacity = swr_get_out_samples(h->swr, frame->nb_samples);
    if (sound_room(h, capacity) < 0) {
        return TOSS_MEDIA_E_INTERNAL;
    }

    /* The time of the first sample that comes *out*: the frame's own time
     * minus whatever the resampler was still holding from earlier frames. */
    long long held = swr_get_delay(h->swr, 10000000);
    uint8_t *planes[1] = { h->sound };
    int produced = swr_convert(h->swr, planes, capacity, (const uint8_t **)frame->extended_data,
                               frame->nb_samples);
    if (produced < 0) {
        return TOSS_MEDIA_E_INTERNAL;
    }
    long long pts = stream_pts(h, &h->audio);
    long long in_duration = av_rescale(frame->nb_samples, 10000000, frame->sample_rate);
    h->audio.last_pts = pts;
    h->audio.last_dur = in_duration;
    if (produced == 0) {
        return 0;
    }
    sound_chunk(out, h->sound, produced, pts > held ? pts - held : 0);
    return 1;
}

/* At the very end, the few samples the resampler's filter still holds.
 * 1 = a last chunk, 0 = nothing left. The resampler is freed either way,
 * so this hands out its tail exactly once. */
static int publish_tail(TossMedia *h, TossMediaChunk *out)
{
    if (!h->swr || !h->audio.ctx) {
        /* No sound left to finish, or nobody wants it any more. */
        swr_free(&h->swr);
        return 0;
    }
    long long held = swr_get_delay(h->swr, 10000000);
    int capacity = swr_get_out_samples(h->swr, 0);
    int produced = 0;
    if (capacity > 0 && sound_room(h, capacity) == 0) {
        uint8_t *planes[1] = { h->sound };
        produced = swr_convert(h->swr, planes, capacity, NULL, 0);
    }
    swr_free(&h->swr);
    if (produced <= 0) {
        return 0;
    }
    long long end = h->audio.last_pts + h->audio.last_dur;
    sound_chunk(out, h->sound, produced, end > held ? end - held : 0);
    return 1;
}

/* Make the sound buffer hold `frames` output frames. */
static int sound_room(TossMedia *h, int frames)
{
    if (frames <= 0 || frames > INT_MAX / (TOSS_MEDIA_CHANNELS * 2)) {
        return -1;
    }
    size_t bytes = (size_t)frames * TOSS_MEDIA_CHANNELS * 2u;
    if (bytes > h->sound_capacity) {
        unsigned char *fresh = (unsigned char *)realloc(h->sound, bytes);
        if (!fresh) {
            return -1;
        }
        h->sound = fresh;
        h->sound_capacity = bytes;
    }
    return 0;
}

static void sound_chunk(TossMediaChunk *out, unsigned char *data, int produced, long long pts)
{
    out->kind = TOSS_MEDIA_AUDIO;
    out->width = 0;
    out->height = 0;
    out->stride = 0;
    out->frames = produced;
    out->bytes = produced * TOSS_MEDIA_CHANNELS * 2;
    out->pts_100ns = pts;
    out->duration_100ns = av_rescale(produced, 10000000, TOSS_MEDIA_RATE);
    out->data = data;
}

/* One more damaged packet or frame. Playback carries on through a short
 * run of them; a long run means the file, not the moment, is broken. */
static int bad_data(TossMedia *h)
{
    if (++h->bad_run > TOSS_MEDIA_MAX_BAD_RUN) {
        h->finished = 1;
        return TOSS_MEDIA_E_DATA;
    }
    return 0;
}

/* The next decoder still holding output during the final flush. */
static TossStream *next_to_flush(TossMedia *h)
{
    if (h->video.ctx && !h->video.done) {
        return &h->video;
    }
    if (h->audio.ctx && !h->audio.done) {
        return &h->audio;
    }
    return NULL;
}

/* 1 = a chunk was produced, 0 = the presentation is over, <0 = an error
 * class from the table at the top of this file. */
int toss_media_next(TossMedia *h, TossMediaChunk *out)
{
    if (h->finished) {
        return 0;
    }
    for (;;) {
        if (h->draining) {
            TossStream *stream = h->draining;
            int ret = avcodec_receive_frame(stream->ctx, h->frame);
            if (ret == AVERROR(EAGAIN)) {
                h->draining = NULL;
                if (h->flushing) {
                    /* Nothing more to feed it: an empty-handed decoder in
                     * flush mode is finished. */
                    stream->done = 1;
                }
                continue;
            }
            if (ret == AVERROR_EOF) {
                stream->done = 1;
                h->draining = NULL;
                continue;
            }
            if (ret < 0) {
                int verdict = bad_data(h);
                if (verdict < 0) {
                    return verdict;
                }
                h->draining = NULL;
                continue;
            }
            int code = stream == &h->video ? publish_video(h, out) : publish_audio(h, out);
            av_frame_unref(h->frame);
            if (code == TOSS_MEDIA_E_DATA) {
                int verdict = bad_data(h);
                if (verdict < 0) {
                    return verdict;
                }
                continue;
            }
            if (code < 0) {
                h->finished = 1;
                return code;
            }
            if (code == 0) {
                continue;
            }
            h->bad_run = 0;
            h->produced++;
            return 1;
        }

        if (h->flushing) {
            TossStream *stream = next_to_flush(h);
            if (!stream) {
                if (publish_tail(h, out)) {
                    h->produced++;
                    return 1;
                }
                h->finished = 1;
                /* A file that opened and then yielded nothing at all did
                 * not play: that is damage, not a short film. */
                return h->produced == 0 ? TOSS_MEDIA_E_DATA : 0;
            }
            h->draining = stream;
            continue;
        }

        int ret = av_read_frame(h->fmt, h->pkt);
        if (ret == AVERROR_EOF) {
            h->flushing = 1;
            if (h->video.ctx) {
                avcodec_send_packet(h->video.ctx, NULL);
            }
            if (h->audio.ctx) {
                avcodec_send_packet(h->audio.ctx, NULL);
            }
            continue;
        }
        if (ret < 0) {
            /* A hard read error after a successful open: the bytes stop
             * making sense. A merely cut-off file ends through EOF above. */
            h->finished = 1;
            return TOSS_MEDIA_E_DATA;
        }

        TossStream *stream = NULL;
        if (h->video.ctx && h->pkt->stream_index == h->video.index) {
            stream = &h->video;
        } else if (h->audio.ctx && h->pkt->stream_index == h->audio.index) {
            stream = &h->audio;
        }
        if (!stream) {
            av_packet_unref(h->pkt);
            continue;
        }
        /* Each decoder is emptied before it is fed again, so a send is
         * never refused for being full. */
        ret = avcodec_send_packet(stream->ctx, h->pkt);
        av_packet_unref(h->pkt);
        if (ret < 0 && ret != AVERROR(EAGAIN)) {
            int verdict = bad_data(h);
            if (verdict < 0) {
                return verdict;
            }
            continue;
        }
        h->draining = stream;
    }
}

/* Move to `pos_100ns` from the start of the presentation, landing on the
 * key frame at or before it. Decoders and the resampler forget what they
 * held, so nothing from before the jump is ever handed out after it; the
 * caller discards the chunks between the key frame and the position. */
int toss_media_seek(TossMedia *h, long long pos_100ns)
{
    if (pos_100ns < 0) {
        pos_100ns = 0;
    }
    long long target = av_sat_add64(av_rescale_q(pos_100ns, TOSS_100NS_Q, AV_TIME_BASE_Q),
                                    presentation_start(h));
    if (av_seek_frame(h->fmt, -1, target, AVSEEK_FLAG_BACKWARD) < 0) {
        return TOSS_MEDIA_E_INTERNAL;
    }
    if (h->video.ctx) {
        avcodec_flush_buffers(h->video.ctx);
    }
    if (h->audio.ctx) {
        avcodec_flush_buffers(h->audio.ctx);
    }
    swr_free(&h->swr);
    av_packet_unref(h->pkt);
    av_frame_unref(h->frame);
    h->video.done = 0;
    h->audio.done = 0;
    h->video.last_pts = pos_100ns;
    h->audio.last_pts = pos_100ns;
    h->video.last_dur = 0;
    h->audio.last_dur = 0;
    h->draining = NULL;
    h->flushing = 0;
    h->finished = 0;
    h->bad_run = 0;
    return 0;
}

void toss_media_close(TossMedia *h)
{
    if (!h) {
        return;
    }
    sws_freeContext(h->sws);
    swr_free(&h->swr);
    free(h->picture);
    free(h->sound);
    av_frame_free(&h->frame);
    av_packet_free(&h->pkt);
    avcodec_free_context(&h->video.ctx);
    avcodec_free_context(&h->audio.ctx);
    avformat_close_input(&h->fmt);
    free(h);
}
