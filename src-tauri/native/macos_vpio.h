#ifndef SAAA_MACOS_VPIO_H
#define SAAA_MACOS_VPIO_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    SAAA_VPIO_OK = 0,
    SAAA_VPIO_ERR = 1,
    SAAA_VPIO_UNSUPPORTED = 2
};

typedef struct SaaaVpio SaaaVpio;

typedef struct SaaaVpioConfig {
    uint32_t sample_rate;
    uint32_t ducking_level;
    uint8_t enable_advanced_ducking;
    uint8_t enable_agc;
    uint8_t bypass_voice_processing;
    uint8_t reserved;
} SaaaVpioConfig;

typedef struct SaaaVpioStatus {
    uint32_t sample_rate;
    uint32_t ducking_level;
    uint32_t output_transport;
    uint8_t advanced_ducking;
    uint8_t agc_enabled;
    uint8_t bypass_enabled;
    uint8_t macos_major;
} SaaaVpioStatus;

typedef uint32_t (*SaaaRingWriteF32)(void *ring, const float *samples, uint32_t count);
typedef uint32_t (*SaaaRingReadF32)(void *ring, float *samples, uint32_t count);
typedef void (*SaaaFlagFn)(void *ctx);

SaaaVpio *saaa_vpio_create(const SaaaVpioConfig *config, void *playback_ring, void *capture_ring,
    SaaaRingWriteF32 write_capture, SaaaRingReadF32 read_playback, SaaaFlagFn on_route_change,
    void *route_ctx, char *err, uint32_t err_len);
int saaa_vpio_start(SaaaVpio *session, char *err, uint32_t err_len);
int saaa_vpio_stop(SaaaVpio *session);
int saaa_vpio_readback(SaaaVpio *session, SaaaVpioStatus *status);
int saaa_vpio_rebuild_requested(const SaaaVpio *session);
void saaa_vpio_clear_rebuild(SaaaVpio *session);
void saaa_vpio_destroy(SaaaVpio *session);
int saaa_audio_default_output_transport(uint32_t *transport);
uint8_t saaa_macos_major_version(void);

#ifdef __cplusplus
}
#endif

#endif
