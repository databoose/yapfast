/* Decoding Go Live video someone else streams: VP8 through libvpx and H264
 * through FFmpeg, both inside Talk's libwebrtc archive. Pictures come out
 * as I420 planes that stay valid until the next call on the decoder. */
#include <stdlib.h>
#include <string.h>

#include "libavcodec/avcodec.h"
#include "vpx/vp8dx.h"
#include "vpx/vpx_decoder.h"

enum { TALK_VP8 = 0, TALK_H264 = 1 };

struct talk_decoder {
  int codec;
  vpx_codec_ctx_t vpx;
  AVCodecContext *context;
  AVPacket *packet;
  AVFrame *frame;
  unsigned char *padded;
  size_t padded_size;
};

struct talk_picture {
  int width;
  int height;
  const unsigned char *y;
  const unsigned char *u;
  const unsigned char *v;
  int y_stride;
  int u_stride;
  int v_stride;
};

void talk_decoder_free(struct talk_decoder *decoder) {
  if (!decoder) return;
  if (decoder->codec == TALK_VP8) {
    vpx_codec_destroy(&decoder->vpx);
  } else {
    avcodec_free_context(&decoder->context);
    av_packet_free(&decoder->packet);
    av_frame_free(&decoder->frame);
    free(decoder->padded);
  }
  free(decoder);
}

struct talk_decoder *talk_decoder_new(int codec, unsigned threads) {
  struct talk_decoder *decoder = calloc(1, sizeof *decoder);
  if (!decoder) return NULL;
  decoder->codec = codec;
  if (codec == TALK_VP8) {
    vpx_codec_dec_cfg_t config = {.threads = threads};
    if (vpx_codec_dec_init(&decoder->vpx, vpx_codec_vp8_dx(), &config, 0)) {
      free(decoder);
      return NULL;
    }
    return decoder;
  }
  if (codec != TALK_H264) {
    free(decoder);
    return NULL;
  }
  const AVCodec *h264 = avcodec_find_decoder(AV_CODEC_ID_H264);
  decoder->context = h264 ? avcodec_alloc_context3(h264) : NULL;
  decoder->packet = av_packet_alloc();
  decoder->frame = av_frame_alloc();
  if (!decoder->context || !decoder->packet || !decoder->frame) {
    talk_decoder_free(decoder);
    return NULL;
  }
  decoder->context->thread_count = (int)threads;
  /* Show each frame as soon as it's decoded. */
  decoder->context->flags |= AV_CODEC_FLAG_LOW_DELAY;
  if (avcodec_open2(decoder->context, h264, NULL) < 0) {
    talk_decoder_free(decoder);
    return NULL;
  }
  return decoder;
}

/* Decodes one frame. Returns 1 with a picture, 0 when the decoder has none
 * yet, or a negative number when the frame couldn't be decoded. */
int talk_decoder_decode(struct talk_decoder *decoder, const unsigned char *data,
                        size_t size, struct talk_picture *picture) {
  if (decoder->codec == TALK_VP8) {
    if (vpx_codec_decode(&decoder->vpx, data, (unsigned int)size, NULL, 0)) return -1;
    vpx_codec_iter_t iter = NULL;
    vpx_image_t *image = vpx_codec_get_frame(&decoder->vpx, &iter);
    if (!image) return 0;
    if (image->fmt != VPX_IMG_FMT_I420) return -2;
    picture->width = (int)image->d_w;
    picture->height = (int)image->d_h;
    picture->y = image->planes[VPX_PLANE_Y];
    picture->u = image->planes[VPX_PLANE_U];
    picture->v = image->planes[VPX_PLANE_V];
    picture->y_stride = image->stride[VPX_PLANE_Y];
    picture->u_stride = image->stride[VPX_PLANE_U];
    picture->v_stride = image->stride[VPX_PLANE_V];
    return 1;
  }
  /* FFmpeg's parsers may read a little past the end. */
  size_t needed = size + AV_INPUT_BUFFER_PADDING_SIZE;
  if (decoder->padded_size < needed) {
    unsigned char *grown = realloc(decoder->padded, needed);
    if (!grown) return -3;
    decoder->padded = grown;
    decoder->padded_size = needed;
  }
  memcpy(decoder->padded, data, size);
  memset(decoder->padded + size, 0, AV_INPUT_BUFFER_PADDING_SIZE);
  decoder->packet->data = decoder->padded;
  decoder->packet->size = (int)size;
  if (avcodec_send_packet(decoder->context, decoder->packet) < 0) return -1;
  av_frame_unref(decoder->frame);
  int received = avcodec_receive_frame(decoder->context, decoder->frame);
  if (received == AVERROR(EAGAIN)) return 0;
  if (received < 0) return -1;
  AVFrame *frame = decoder->frame;
  if (frame->format != AV_PIX_FMT_YUV420P && frame->format != AV_PIX_FMT_YUVJ420P) return -2;
  picture->width = frame->width;
  picture->height = frame->height;
  picture->y = frame->data[0];
  picture->u = frame->data[1];
  picture->v = frame->data[2];
  picture->y_stride = frame->linesize[0];
  picture->u_stride = frame->linesize[1];
  picture->v_stride = frame->linesize[2];
  return 1;
}
