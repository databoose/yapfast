/* H264 encoding for Discord Go Live, through the OpenH264 inside Yapfast's
 * libwebrtc archive. The same interface as vp8.c, so Rust drives both alike.
 * Frames come out in Annex B form, each NAL unit after a 4-byte start code. */
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>

#include "wels/codec_api.h"

struct yapfast_h264 {
  ISVCEncoder *encoder;
  int width;
  int height;
  int kbps;
};

/* A realtime encoder for screen content: constrained baseline, which every
 * Discord client decodes, with the parameter sets repeated before each IDR. */
struct yapfast_h264 *yapfast_h264_new(unsigned width, unsigned height, unsigned fps,
                                unsigned kbps, unsigned threads) {
  ISVCEncoder *encoder = NULL;
  if (WelsCreateSVCEncoder(&encoder) || !encoder) return NULL;
  SEncParamExt param;
  memset(&param, 0, sizeof param);
  (*encoder)->GetDefaultParams(encoder, &param);
  param.iUsageType = SCREEN_CONTENT_REAL_TIME;
  param.iPicWidth = (int)width;
  param.iPicHeight = (int)height;
  param.iTargetBitrate = (int)kbps * 1000;
  param.iMaxBitrate = (int)kbps * 1500;
  param.iRCMode = RC_BITRATE_MODE;
  param.fMaxFrameRate = (float)fps;
  param.iTemporalLayerNum = 1;
  param.iSpatialLayerNum = 1;
  /* A new viewer waits at most this long for a picture. */
  param.uiIntraPeriod = fps * 3;
  param.iNumRefFrame = 1;
  param.eSpsPpsIdStrategy = CONSTANT_ID;
  param.bPrefixNalAddingCtrl = false;
  param.iEntropyCodingModeFlag = 0;
  param.bEnableFrameSkip = true;
  param.iMultipleThreadIdc = (unsigned short)threads;
  param.bEnableDenoise = false;
  param.bEnableBackgroundDetection = false;
  param.bEnableAdaptiveQuant = false;
  param.bEnableSceneChangeDetect = true;
  param.bEnableLongTermReference = false;

  SSpatialLayerConfig *layer = &param.sSpatialLayers[0];
  layer->iVideoWidth = (int)width;
  layer->iVideoHeight = (int)height;
  layer->fFrameRate = (float)fps;
  layer->iSpatialBitrate = param.iTargetBitrate;
  layer->iMaxSpatialBitrate = param.iMaxBitrate;
  layer->uiProfileIdc = PRO_BASELINE;
  /* One slice for each thread; each slice is its own NAL unit. */
  layer->sSliceArgument.uiSliceMode =
      threads > 1 ? SM_FIXEDSLCNUM_SLICE : SM_SINGLE_SLICE;
  layer->sSliceArgument.uiSliceNum = threads > 1 ? threads : 1;

  if ((*encoder)->InitializeExt(encoder, &param)) {
    WelsDestroySVCEncoder(encoder);
    return NULL;
  }
  int format = videoFormatI420;
  (*encoder)->SetOption(encoder, ENCODER_OPTION_DATAFORMAT, &format);

  struct yapfast_h264 *h264 = calloc(1, sizeof *h264);
  if (!h264) {
    (*encoder)->Uninitialize(encoder);
    WelsDestroySVCEncoder(encoder);
    return NULL;
  }
  h264->encoder = encoder;
  h264->width = (int)width;
  h264->height = (int)height;
  h264->kbps = (int)kbps;
  return h264;
}

/* Encodes one I420 frame into `out`, with `pts` in 90 kHz units. Returns the
 * encoded size, 0 when the encoder skipped the frame, or a negative number
 * on failure. `duration` is unused: OpenH264 times frames itself. */
long yapfast_h264_encode(struct yapfast_h264 *h264, const unsigned char *y,
                      int y_stride, const unsigned char *u, int u_stride,
                      const unsigned char *v, int v_stride, long long pts,
                      unsigned long duration, int keyframe, unsigned char *out,
                      size_t capacity, int *is_key) {
  (void)duration;
  ISVCEncoder *encoder = h264->encoder;
  if (keyframe) (*encoder)->ForceIntraFrame(encoder, true);
  SSourcePicture picture;
  memset(&picture, 0, sizeof picture);
  picture.iColorFormat = videoFormatI420;
  picture.iPicWidth = h264->width;
  picture.iPicHeight = h264->height;
  picture.iStride[0] = y_stride;
  picture.iStride[1] = u_stride;
  picture.iStride[2] = v_stride;
  picture.pData[0] = (unsigned char *)y;
  picture.pData[1] = (unsigned char *)u;
  picture.pData[2] = (unsigned char *)v;
  picture.uiTimeStamp = pts / 90;
  SFrameBSInfo info;
  memset(&info, 0, sizeof info);
  if ((*encoder)->EncodeFrame(encoder, &picture, &info) != cmResultSuccess)
    return -1;
  *is_key = info.eFrameType == videoFrameTypeIDR;
  if (info.eFrameType == videoFrameTypeSkip ||
      info.eFrameType == videoFrameTypeInvalid)
    return 0;
  size_t written = 0;
  for (int index = 0; index < info.iLayerNum; index++) {
    const SLayerBSInfo *layer = &info.sLayerInfo[index];
    size_t size = 0;
    for (int nal = 0; nal < layer->iNalCount; nal++)
      size += (size_t)layer->pNalLengthInByte[nal];
    if (written + size > capacity) return -2;
    memcpy(out + written, layer->pBsBuf, size);
    written += size;
  }
  return (long)written;
}

/* Changes the bitrate the encoder aims for, as the network allows, keeping
 * its maximum half as much again. Returns 0, or nonzero on failure. */
int yapfast_h264_set_bitrate(struct yapfast_h264 *h264, unsigned kbps) {
  ISVCEncoder *encoder = h264->encoder;
  SBitrateInfo target = {SPATIAL_LAYER_ALL, (int)kbps * 1000};
  SBitrateInfo max = {SPATIAL_LAYER_ALL, (int)kbps * 1500};
  /* The target may never pass the maximum, so the one growing goes first. */
  int failed;
  if ((int)kbps > h264->kbps)
    failed = (*encoder)->SetOption(encoder, ENCODER_OPTION_MAX_BITRATE, &max) ||
             (*encoder)->SetOption(encoder, ENCODER_OPTION_BITRATE, &target);
  else
    failed = (*encoder)->SetOption(encoder, ENCODER_OPTION_BITRATE, &target) ||
             (*encoder)->SetOption(encoder, ENCODER_OPTION_MAX_BITRATE, &max);
  if (!failed) h264->kbps = (int)kbps;
  return failed;
}

void yapfast_h264_free(struct yapfast_h264 *h264) {
  (*h264->encoder)->Uninitialize(h264->encoder);
  WelsDestroySVCEncoder(h264->encoder);
  free(h264);
}
