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
 * comes first. Single-threaded per session (Slice 1: no worker threads).
 *
 * Error classes are stable contract with libav.rs (see from_shim there):
 *   -1  container/format not recognised   -> TossError::UnsupportedFormat
 *   -2  no decodable video stream         -> TossError::UnsupportedFormat
 *   -3  bytes recognised but damaged      -> TossError::CorruptInput
 *   -4  internal failure                  -> TossError::other
 */

#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libavutil/error.h>
#include <libavutil/log.h>
#include <libavutil/pixfmt.h>
#include <libswscale/swscale.h>

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define TOSS_HEVC_E_RECOGNIZE (-1)
#define TOSS_HEVC_E_NO_VIDEO  (-2)
#define TOSS_HEVC_E_DATA      (-3)
#define TOSS_HEVC_E_INTERNAL  (-4)

typedef struct TossHevcFrame {
    int width;
    int height;
    int stride;      /* bytes per row: width * 4 (BGRA8) */
    int bytes;       /* stride * height */
    int src_pix_fmt; /* AVPixelFormat the decoder produced */
    long index;      /* 0-based count of frames handed out before this one */
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
    long produced; /* frames handed out so far */
    int finished;  /* terminal: flush exhausted or a fatal read/convert error */
    int saw_error_log; /* an AV_LOG_ERROR was seen on this thread */
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

static void silence_ffmpeg_logs(void)
{
    av_log_set_callback(count_errors);
}

/* One frame's worth of BGRA storage, (re)allocated when the picture shape
 * changes. Overflow-checked: these are numbers a file chose. */
static int ensure_buffer(TossHevc *h, int width, int height)
{
    if (width <= 0 || height <= 0 || width > 65536 || height > 65536) {
        return TOSS_HEVC_E_INTERNAL;
    }
    size_t bytes = (size_t)width * 4u * (size_t)height;
    unsigned char *fresh = (unsigned char *)realloc(h->bgra, bytes);
    if (!fresh) {
        return TOSS_HEVC_E_INTERNAL;
    }
    h->bgra = fresh;
    h->width = width;
    h->height = height;
    h->stride = width * 4;
    h->bytes = (int)bytes;
    return 0;
}

/* Used by the fail path in toss_hevc_open below. */
void toss_hevc_close(TossHevc *h);

TossHevc *toss_hevc_open(const char *utf8_path, int *err_out)
{
    *err_out = 0;
    silence_ffmpeg_logs();
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

int toss_hevc_next(TossHevc *h, TossHevcFrame *out, const unsigned char **bgra)
{
    if (h->finished) {
        return 0;
    }

    for (;;) {
        int ret = av_read_frame(h->fmt, h->pkt);
        if (ret == AVERROR_EOF) {
            /* Not terminal yet: the decoder's flush below still owns the
             * buffered frames, and the next call must be able to collect
             * them (the lesson the 9-of-10 test taught: marking `finished`
             * here dropped every frame the flush had not handed over). */
            break;
        }
        if (ret < 0) {
            /* A read error mid-file. With frames already delivered the
             * honest answer is "that was the end of what could be read"
             * (the truncated-tail case); with none delivered and damage
             * logged, the file never yielded anything usable. */
            h->finished = 1;
            if (h->produced == 0) {
                return tls_error_logs > 0 ? TOSS_HEVC_E_DATA
                                          : TOSS_HEVC_E_INTERNAL;
            }
            return 0;
        }
        if (h->pkt->stream_index >= 0 &&
            h->fmt->streams[h->pkt->stream_index]->codecpar->codec_type !=
                AVMEDIA_TYPE_VIDEO) {
            av_packet_unref(h->pkt);
            continue;
        }

        ret = avcodec_send_packet(h->ctx, h->pkt);
        av_packet_unref(h->pkt);
        if (ret < 0 && ret != AVERROR(EAGAIN)) {
            h->finished = 1;
            if (h->produced == 0) {
                return tls_error_logs > 0 ? TOSS_HEVC_E_DATA
                                          : TOSS_HEVC_E_INTERNAL;
            }
            return 0;
        }

        ret = avcodec_receive_frame(h->ctx, h->frame);
        if (ret == AVERROR(EAGAIN) || ret == AVERROR_EOF) {
            continue;
        }
        if (ret < 0) {
            h->finished = 1;
            if (h->produced == 0) {
                return tls_error_logs > 0 ? TOSS_HEVC_E_DATA
                                          : TOSS_HEVC_E_INTERNAL;
            }
            return 0;
        }
        if (!h->frame->data[0]) {
            av_frame_unref(h->frame);
            h->finished = 1;
            return TOSS_HEVC_E_INTERNAL;
        }

        /* First frame, or a shape change: (re)build the converter and the
         * destination for exactly this picture (§8.1 — swscale is the one
         * conversion this build owns). */
        if (!h->sws || h->width != h->frame->width || h->height != h->frame->height ||
            h->src_pix_fmt != (int)h->frame->format) {
            if (ensure_buffer(h, h->frame->width, h->frame->height) != 0) {
                av_frame_unref(h->frame);
                h->finished = 1;
                return TOSS_HEVC_E_INTERNAL;
            }
            sws_freeContext(h->sws);
            h->sws = sws_getContext(h->width, h->height,
                                    (enum AVPixelFormat)h->frame->format,
                                    h->width, h->height, AV_PIX_FMT_BGRA,
                                    SWS_BILINEAR, NULL, NULL, NULL);
            if (!h->sws) {
                av_frame_unref(h->frame);
                h->finished = 1;
                return TOSS_HEVC_E_INTERNAL;
            }
            h->src_pix_fmt = (int)h->frame->format;
        }

        unsigned char *dst_data[4] = { h->bgra, NULL, NULL, NULL };
        int dst_linesize[4] = { h->stride, 0, 0, 0 };
        int scaled = sws_scale(h->sws, h->frame->data, h->frame->linesize, 0,
                               h->height, dst_data, dst_linesize);
        if (scaled != h->height) {
            av_frame_unref(h->frame);
            h->finished = 1;
            return TOSS_HEVC_E_INTERNAL;
        }

        out->width = h->width;
        out->height = h->height;
        out->stride = h->stride;
        out->bytes = h->bytes;
        out->src_pix_fmt = h->src_pix_fmt;
        out->index = h->produced;
        *bgra = h->bgra;
        h->produced++;
        av_frame_unref(h->frame);
        return 1;
    }

    /* Drained: send the flush marker and take whatever is still buffered.
     * For the one-second corpus files this is where the last frames live. */
    avcodec_send_packet(h->ctx, NULL);
    for (;;) {
        int ret = avcodec_receive_frame(h->ctx, h->frame);
        if (ret < 0 || !h->frame->data[0]) {
            break;
        }
        if (!h->sws || h->width != h->frame->width || h->height != h->frame->height ||
            h->src_pix_fmt != (int)h->frame->format) {
            if (ensure_buffer(h, h->frame->width, h->frame->height) != 0) {
                av_frame_unref(h->frame);
                h->finished = 1;
                return TOSS_HEVC_E_INTERNAL;
            }
            sws_freeContext(h->sws);
            h->sws = sws_getContext(h->width, h->height,
                                    (enum AVPixelFormat)h->frame->format,
                                    h->width, h->height, AV_PIX_FMT_BGRA,
                                    SWS_BILINEAR, NULL, NULL, NULL);
            if (!h->sws) {
                av_frame_unref(h->frame);
                h->finished = 1;
                return TOSS_HEVC_E_INTERNAL;
            }
            h->src_pix_fmt = (int)h->frame->format;
        }
        unsigned char *dst_data[4] = { h->bgra, NULL, NULL, NULL };
        int dst_linesize[4] = { h->stride, 0, 0, 0 };
        if (sws_scale(h->sws, h->frame->data, h->frame->linesize, 0, h->height,
                      dst_data, dst_linesize) != h->height) {
            av_frame_unref(h->frame);
            h->finished = 1;
            return TOSS_HEVC_E_INTERNAL;
        }
        out->width = h->width;
        out->height = h->height;
        out->stride = h->stride;
        out->bytes = h->bytes;
        out->src_pix_fmt = h->src_pix_fmt;
        out->index = h->produced;
        *bgra = h->bgra;
        h->produced++;
        av_frame_unref(h->frame);
        return 1;
    }

    /* The flush had nothing left: the presentation is over for good. */
    h->finished = 1;
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
