/*
 * The software HEVC decode shim: everything FFmpeg on this project touches
 * lives in this file, and what leaves it is plain C data (Slice 1, design
 * docs/Phase76_Fallback_Boundary_Design.md §5).
 *
 * Built by build.rs with cl against third_party/ffmpeg's headers; called
 * from src/platform/windows/media/libav.rs and from nowhere else.
 *
 * Ownership: one session = one open file. `toss_hevc_open` hands back an
 * opaque session the caller must pass to `toss_hevc_close` exactly once;
 * the BGRA pointer returned by `toss_hevc_next` belongs to the session and
 * stays valid until the next `toss_hevc_next` or the close, whichever
 * comes first. Single-threaded per session (Slice 2: no worker threads).
 *
 * Slice 2 added the two numbers presentation needs — a per-frame timestamp
 * and duration in 100-ns units inside `TossHevcFrame`, plus
 * `toss_hevc_duration`/`toss_hevc_seek` — still plain C data: no AV type
 * crosses the boundary, and a seek does its own `avcodec_flush_buffers`
 * reset so the caller never sees a stale picture.
 *
 * Units: every `*_100ns` number is 100-nanosecond units — 1 s is
 * 10,000,000, 5 s is 50,000,000 — the unit D2's seek step and MF's clock
 * already speak. The conversion is done by `av_rescale_q` straight into
 * `{1, 10000000}`; nothing multiplies a microsecond count by ten, because
 * that multiply is the one place a long file could overflow signed 64-bit.
 *
 * Error classes are stable contract with libav.rs (see from_shim there):
 *   -1  container/format not recognised   -> TossError::UnsupportedFormat
 *   -2  no decodable video stream         -> TossError::UnsupportedFormat
 *   -3  bytes recognised but damaged      -> TossError::CorruptInput
 *   -4  internal failure                  -> TossError::other
 */

#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libavutil/avutil.h>
#include <libavutil/common.h>
#include <libavutil/error.h>
#include <libavutil/log.h>
#include <libavutil/mathematics.h>
#include <libavutil/pixfmt.h>
#include <libswscale/swscale.h>

#include <limits.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define TOSS_HEVC_E_RECOGNIZE (-1)
#define TOSS_HEVC_E_NO_VIDEO  (-2)
#define TOSS_HEVC_E_DATA      (-3)
#define TOSS_HEVC_E_INTERNAL  (-4)

/* 100-ns units per second, as an AVRational to rescale into. */
static const AVRational TOSS_100NS_Q = { 1, 10000000 };

typedef struct TossHevcFrame {
    int width;
    int height;
    int stride;      /* bytes per row: width * 4 (BGRA8) */
    int bytes;       /* stride * height */
    int src_pix_fmt; /* AVPixelFormat the decoder produced */
    long index;      /* 0-based count of frames handed out before this one */
    /* Presentation time of this picture, and how long it holds, both in
     * 100-ns units (the units D2's seek step and MF's clock already speak).
     * - pts is AV_NOPTS-resolved: a source with no timestamp advances by the
     *   previous picture's length, so the pacing loop always has a number.
     * - duration falls back frame -> frame rate -> previous -> 10 ms; it is
     *   only ever used to know when the *last* picture has had its time. */
    long long pts_100ns;
    long long duration_100ns;
} TossHevcFrame;

typedef struct TossHevc {
    AVFormatContext *fmt;
    AVCodecContext *ctx;
    AVPacket *pkt;
    AVFrame *frame;
    struct SwsContext *sws;
    unsigned char *bgra;
    int width;
    int height;
    int stride;
    int bytes;
    int src_pix_fmt;
    int vidx;        /* index of the video stream the decoder was opened on */
    long produced;   /* frames handed out so far */
    int finished;    /* terminal: flush exhausted or a fatal read/convert error */
    int have_packet; /* a read packet sits in `pkt`, not yet accepted by the decoder */
    int flush_sent;  /* the NULL flush marker has been sent: drain only */
    long long last_pts; /* pts of the previous picture, 100-ns */
    long long last_dur; /* duration of the previous picture, 100-ns */
} TossHevc;

/*
 * Count error-level av_log messages on the *calling* thread and print
 * nothing: Toss owns its diagnostics (§25 — stderr carries Toss's words,
 * not a library's chatter), but the error count is what tells an open that
 * "no video stream" came from damaged bytes rather than a clean file that
 * simply has none. TLS because cargo test runs sessions on parallel
 * threads and a global counter would let one test's corruption bless
 * another test's verdict.
 */
static __declspec(thread) int tls_error_logs;

static void count_errors(void *ptr, int level, const char *fmt, va_list vl)
{
    (void)ptr;
    (void)fmt;
    (void)vl;
    if (level <= AV_LOG_ERROR) {
        tls_error_logs++;
    }
}

/* Installed exactly once, from Rust under a `Once`, before any session
 * exists. `av_log_set_callback` mutates a process-global; reassigning it
 * from concurrent unit-test threads would be a data race, so the callers
 * funnel through here. The counter itself stays per-thread
 * (`tls_error_logs`), so one test's corruption cannot bless another's. */
void toss_hevc_init(void)
{
    av_log_set_callback(count_errors);
}

/* One frame's worth of BGRA storage, (re)allocated when the picture shape
 * changes. Overflow-checked: these are numbers a file chose, and the frame
 * header reports width, height and total bytes as 32-bit ints — a picture
 * the header cannot describe must be refused before the allocation and the
 * cast, never truncated into a buffer shorter than the reported size. */
static int ensure_buffer(TossHevc *h, int width, int height)
{
    if (width <= 0 || height <= 0 || width > 65536 || height > 65536) {
        return TOSS_HEVC_E_INTERNAL;
    }
    size_t stride = (size_t)width * 4u;
    size_t bytes = stride * (size_t)height;
    if (stride > (size_t)INT_MAX || bytes > (size_t)INT_MAX) {
        return TOSS_HEVC_E_INTERNAL;
    }
    unsigned char *fresh = (unsigned char *)realloc(h->bgra, bytes);
    if (!fresh) {
        return TOSS_HEVC_E_INTERNAL;
    }
    h->bgra = fresh;
    h->width = width;
    h->height = height;
    h->stride = (int)stride;
    h->bytes = (int)bytes;
    return 0;
}

/* Used by the fail path in toss_hevc_open below. */
void toss_hevc_close(TossHevc *h);

/* The stream's first presentation time, normalised away so both the
 * per-frame clock and the seek speak the same zero-based timeline. A stream
 * that does not state one starts at zero. */
static long long stream_start(const TossHevc *h)
{
    const AVStream *st = h->fmt->streams[h->vidx];
    return st->start_time == AV_NOPTS_VALUE ? 0 : st->start_time;
}

/* The picture waiting in h->frame, as a presentation time: 100-ns units,
 * counted from the stream's own start. A source with no timestamp advances
 * by the previous picture's length instead of handing the pacing loop an
 * "unknown" (or, worse, the same time twice). */
static long long frame_pts_100ns(TossHevc *h)
{
    const AVStream *st = h->fmt->streams[h->vidx];
    if (h->frame->pts == AV_NOPTS_VALUE) {
        long long step = h->last_dur > 0 ? h->last_dur : 0;
        return av_sat_add64(h->last_pts, step);
    }
    long long rel = av_sat_sub64(h->frame->pts, stream_start(h));
    long long pts = av_rescale_q(rel, st->time_base, TOSS_100NS_Q);
    return pts < 0 ? 0 : pts;
}

/* How long the picture holds, in 100-ns units. The frame's own duration
 * when it has one, the stream's average frame rate when it does not, the
 * previous picture after that, and 10 ms as the last resort — the number
 * only decides when the final picture has had its time, so a wrong guess
 * costs at most one tick of display, never a hang or an early exit. */
static long long frame_duration_100ns(TossHevc *h)
{
    const AVStream *st = h->fmt->streams[h->vidx];
    if (h->frame->duration > 0) {
        long long duration =
            av_rescale_q(h->frame->duration, st->time_base, TOSS_100NS_Q);
        if (duration > 0) {
            return duration;
        }
    }
    if (st->avg_frame_rate.num > 0 && st->avg_frame_rate.den > 0) {
        return (long long)(10000000.0 * st->avg_frame_rate.den /
                           st->avg_frame_rate.num + 0.5);
    }
    if (h->last_dur > 0) {
        return h->last_dur;
    }
    return 100000; /* 10 ms */
}

/* Hand the converted picture over. One place, because two fill sites that
 * drift apart would give the same frame two different timestamps. */
static void publish(TossHevc *h, TossHevcFrame *out, const unsigned char **bgra)
{
    out->width = h->width;
    out->height = h->height;
    out->stride = h->stride;
    out->bytes = h->bytes;
    out->src_pix_fmt = h->src_pix_fmt;
    out->index = h->produced;
    out->pts_100ns = frame_pts_100ns(h);
    out->duration_100ns = frame_duration_100ns(h);
    *bgra = h->bgra;
    h->last_pts = out->pts_100ns;
    h->last_dur = out->duration_100ns;
    h->produced++;
}

TossHevc *toss_hevc_open(const char *utf8_path, int *err_out)
{
    *err_out = 0;
    /* The log callback is installed once by toss_hevc_init; only this
     * thread's counter is per-session. */
    tls_error_logs = 0;

    TossHevc *h = (TossHevc *)calloc(1, sizeof(TossHevc));
    if (!h) {
        *err_out = TOSS_HEVC_E_INTERNAL;
        return NULL;
    }

    int ret = avformat_open_input(&h->fmt, utf8_path, NULL, NULL);
    if (ret < 0) {
        /* The bytes did not open as any format this build knows: not a
         * recognised container (design §9: "libav 也说不认识" -> 3). */
        *err_out = TOSS_HEVC_E_RECOGNIZE;
        goto fail;
    }

    ret = avformat_find_stream_info(h->fmt, NULL);
    if (ret < 0) {
        *err_out = tls_error_logs > 0 ? TOSS_HEVC_E_DATA : TOSS_HEVC_E_RECOGNIZE;
        goto fail;
    }

    int vi = av_find_best_stream(h->fmt, AVMEDIA_TYPE_VIDEO, -1, -1, NULL, 0);
    if (vi < 0) {
        /* Recognised but damaged: errors were logged while looking.
         * Clean file with no video: recognised, just nothing to decode. */
        *err_out = tls_error_logs > 0 ? TOSS_HEVC_E_DATA : TOSS_HEVC_E_NO_VIDEO;
        goto fail;
    }
    h->vidx = vi;

    const AVCodec *dec = avcodec_find_decoder(h->fmt->streams[vi]->codecpar->codec_id);
    if (!dec) {
        /* The container parsed, the stream exists, this build simply has
         * no decoder for it: "there is no such thing here" -> 3. */
        *err_out = TOSS_HEVC_E_NO_VIDEO;
        goto fail;
    }

    h->ctx = avcodec_alloc_context3(dec);
    if (!h->ctx) {
        *err_out = TOSS_HEVC_E_INTERNAL;
        goto fail;
    }
    if (avcodec_parameters_to_context(h->ctx, h->fmt->streams[vi]->codecpar) < 0 ||
        avcodec_open2(h->ctx, dec, NULL) < 0) {
        *err_out = TOSS_HEVC_E_RECOGNIZE;
        goto fail;
    }

    h->pkt = av_packet_alloc();
    h->frame = av_frame_alloc();
    if (!h->pkt || !h->frame) {
        *err_out = TOSS_HEVC_E_INTERNAL;
        goto fail;
    }

    return h;

fail:
    toss_hevc_close(h);
    return NULL;
}

/* Convert the picture sitting in h->frame into the session's BGRA buffer.
 * The caller has already established that frame->data[0] is present. */
static int publish_frame(TossHevc *h, TossHevcFrame *out, const unsigned char **bgra)
{
    if (!h->sws || h->width != h->frame->width || h->height != h->frame->height ||
        h->src_pix_fmt != (int)h->frame->format) {
        if (ensure_buffer(h, h->frame->width, h->frame->height) != 0) {
            return TOSS_HEVC_E_INTERNAL;
        }
        sws_freeContext(h->sws);
        h->sws = sws_getContext(h->width, h->height,
                                (enum AVPixelFormat)h->frame->format,
                                h->width, h->height, AV_PIX_FMT_BGRA,
                                SWS_BILINEAR, NULL, NULL, NULL);
        if (!h->sws) {
            return TOSS_HEVC_E_INTERNAL;
        }
        h->src_pix_fmt = (int)h->frame->format;
    }

    unsigned char *dst_data[4] = { h->bgra, NULL, NULL, NULL };
    int dst_linesize[4] = { h->stride, 0, 0, 0 };
    if (sws_scale(h->sws, h->frame->data, h->frame->linesize, 0, h->height,
                  dst_data, dst_linesize) != h->height) {
        return TOSS_HEVC_E_INTERNAL;
    }

    publish(h, out, bgra);
    return 0;
}

/* The verdict for a hard mid-stream failure (any error that is not
 * AVERROR_EOF, EAGAIN or a clean flush). A truncated-tail file ends through
 * AVERROR_EOF and never reaches here; anything else that stops a stream
 * after a successful open is damaged bytes, not an early end — so it is the
 * `-3` the boundary maps to CorruptInput, never a silent end-of-file. */
static int midstream_error(TossHevc *h)
{
    h->finished = 1;
    if (h->produced == 0) {
        return tls_error_logs > 0 ? TOSS_HEVC_E_DATA : TOSS_HEVC_E_INTERNAL;
    }
    return TOSS_HEVC_E_DATA;
}

int toss_hevc_next(TossHevc *h, TossHevcFrame *out, const unsigned char **bgra)
{
    if (h->finished) {
        return 0;
    }

    for (;;) {
        if (!h->flush_sent) {
            if (!h->have_packet) {
                int ret = av_read_frame(h->fmt, h->pkt);
                if (ret == AVERROR_EOF) {
                    /* Enter draining: the NULL marker flushes the decoder's
                     * own buffered pictures, and the receive below collects
                     * them one per call. The marker is only recorded as sent
                     * when the decoder actually took it — EAGAIN means it
                     * wants buffered output read first, so the marker is
                     * retried next call and no frame is lost or mislabelled
                     * as end-of-file. */
                    int sret = avcodec_send_packet(h->ctx, NULL);
                    if (sret == AVERROR(EAGAIN)) {
                        /* Not accepted yet: drain below, retry next call. */
                    } else if (sret < 0 && sret != AVERROR_EOF) {
                        return midstream_error(h);
                    } else {
                        h->flush_sent = 1;
                    }
                } else if (ret < 0) {
                    return midstream_error(h);
                } else if (h->pkt->stream_index != h->vidx) {
                    /* Only the stream this decoder was opened on. Any other
                     * video stream in the container is somebody else's. */
                    av_packet_unref(h->pkt);
                    continue;
                } else {
                    h->have_packet = 1;
                }
            }

            if (h->have_packet) {
                int ret = avcodec_send_packet(h->ctx, h->pkt);
                if (ret != AVERROR(EAGAIN)) {
                    av_packet_unref(h->pkt);
                    h->have_packet = 0;
                    if (ret == AVERROR_EOF) {
                        h->finished = 1;
                        return 0;
                    }
                    if (ret < 0) {
                        return midstream_error(h);
                    }
                }
                /* EAGAIN: the decoder would not take the packet yet. It is
                 * left in h->pkt and retried after the receive below — never
                 * unreffed, so nothing the file carried is discarded. */
            }
        }

        int ret = avcodec_receive_frame(h->ctx, h->frame);
        if (ret == AVERROR(EAGAIN)) {
            if (h->flush_sent) {
                /* In drain mode a decoder has no more input to wait for:
                 * EAGAIN here means the flush found nothing left. */
                h->finished = 1;
                return 0;
            }
            continue;
        }
        if (ret == AVERROR_EOF) {
            h->finished = 1;
            return 0;
        }
        if (ret < 0) {
            return midstream_error(h);
        }
        if (!h->frame->data[0]) {
            av_frame_unref(h->frame);
            h->finished = 1;
            return TOSS_HEVC_E_INTERNAL;
        }

        int code = publish_frame(h, out, bgra);
        av_frame_unref(h->frame);
        if (code != 0) {
            h->finished = 1;
            return code;
        }
        return 1;
    }
}

/* How long the presentation is, in 100-ns units; -1 when the container
 * does not say (the caller then clamps nothing, which is MF's u64::MAX
 * answer in the units this side speaks). */
long long toss_hevc_duration(const TossHevc *h)
{
    if (!h || !h->fmt || h->fmt->duration <= 0) {
        return -1;
    }
    long long duration = av_rescale_q(h->fmt->duration, AV_TIME_BASE_Q, TOSS_100NS_Q);
    return duration > 0 ? duration : -1;
}

/* Jump to an absolute position and reset everything that remembered the
 * old one. Returns 0 whether the position landed on a picture or on the
 * end — asking for the end is a quiet EOF (P7-C's boundary: "asking for
 * the last frame" must not become an error), and -3 (bytes) only when the
 * file cannot serve a position inside its own declared length. */
int toss_hevc_seek(TossHevc *h, long long pos_100ns)
{
    if (!h || !h->fmt || !h->ctx) {
        return TOSS_HEVC_E_INTERNAL;
    }
    if (pos_100ns < 0) {
        pos_100ns = 0;
    }

    long long duration = toss_hevc_duration(h);
    if (duration > 0 && pos_100ns >= duration) {
        /* At or past the end: land on the end and let the next read answer
         * Ok(None). No error — this is the boundary P7-C recorded on the
         * MF side and this path must not repeat. */
        h->finished = 1;
        h->have_packet = 0;
        return 0;
    }

    /* Seek in the selected stream's own time base, offset by its start so
     * the target is on the same zero-based timeline the frame clock uses.
     * AVSEEK_FLAG_BACKWARD lands on the keyframe at or before the target —
     * the frame the caller then drops as preroll. */
    long long rel = av_rescale_q(pos_100ns, TOSS_100NS_Q,
                                 h->fmt->streams[h->vidx]->time_base);
    long long target = av_sat_add64(stream_start(h), rel);

    if (av_seek_frame(h->fmt, h->vidx, target, AVSEEK_FLAG_BACKWARD) < 0) {
        return TOSS_HEVC_E_DATA;
    }

    /* Clear every buffer that could still hold a picture from before the
     * jump, and forget the timestamp history so a missing-PTS frame after
     * the seek does not continue from a time before it. */
    avcodec_flush_buffers(h->ctx);
    av_packet_unref(h->pkt);
    av_frame_unref(h->frame);
    h->finished = 0;
    h->have_packet = 0;
    h->flush_sent = 0;
    h->last_pts = pos_100ns;
    h->last_dur = 0;
    return 0;
}

void toss_hevc_close(TossHevc *h)
{
    if (!h) {
        return;
    }
    free(h->bgra);
    sws_freeContext(h->sws);
    av_frame_free(&h->frame);
    av_packet_free(&h->pkt);
    avcodec_free_context(&h->ctx);
    avformat_close_input(&h->fmt);
    free(h);
}
