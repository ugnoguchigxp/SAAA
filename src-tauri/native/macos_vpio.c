#include "macos_vpio.h"

#include <AudioToolbox/AudioToolbox.h>
#include <CoreAudio/CoreAudio.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/sysctl.h>

enum { SAAA_MAX_FRAMES = 4096 };

struct SaaaVpio {
    AudioUnit unit;
    void *playback_ring;
    void *capture_ring;
    SaaaRingWriteF32 write_capture;
    SaaaRingReadF32 read_playback;
    SaaaFlagFn on_route_change;
    void *route_ctx;
    float input_samples[SAAA_MAX_FRAMES];
    AudioBufferList input_list;
    _Atomic uint32_t rebuild;
    uint32_t sample_rate;
    uint32_t ducking_level;
    uint8_t advanced_ducking;
    uint8_t enable_agc;
    uint8_t bypass;
    uint8_t started;
};

static void set_error(char *err, uint32_t err_len, const char *message) {
    if (err == NULL || err_len == 0) {
        return;
    }
    snprintf(err, err_len, "%s", message);
}

static AudioStreamBasicDescription mono_float(uint32_t rate) {
    AudioStreamBasicDescription format;
    memset(&format, 0, sizeof(format));
    format.mSampleRate = rate;
    format.mFormatID = kAudioFormatLinearPCM;
    format.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagsNativeEndian;
    format.mBytesPerPacket = 4;
    format.mFramesPerPacket = 1;
    format.mBytesPerFrame = 4;
    format.mChannelsPerFrame = 1;
    format.mBitsPerChannel = 32;
    return format;
}

static OSStatus set_uint32(AudioUnit unit, AudioUnitPropertyID property, AudioUnitScope scope,
    AudioUnitElement bus, UInt32 value) {
    return AudioUnitSetProperty(unit, property, scope, bus, &value, sizeof(value));
}

static OSStatus render_cb(void *inRefCon, AudioUnitRenderActionFlags *ioActionFlags,
    const AudioTimeStamp *inTimeStamp, UInt32 inBusNumber, UInt32 inNumberFrames,
    AudioBufferList *ioData) {
    (void)ioActionFlags;
    (void)inTimeStamp;
    (void)inBusNumber;
    SaaaVpio *session = (SaaaVpio *)inRefCon;
    if (ioData == NULL || ioData->mNumberBuffers == 0 || inNumberFrames == 0) {
        return noErr;
    }
    float *dst = (float *)ioData->mBuffers[0].mData;
    uint32_t n = inNumberFrames;
    if (n > SAAA_MAX_FRAMES) {
        n = SAAA_MAX_FRAMES;
    }
    uint32_t got = 0;
    if (session->read_playback != NULL && dst != NULL) {
        got = session->read_playback(session->playback_ring, dst, n);
    }
    if (dst != NULL) {
        if (got < n) {
            memset(dst + got, 0, (size_t)(n - got) * sizeof(float));
        }
        if (inNumberFrames > n) {
            memset(dst + n, 0, (size_t)(inNumberFrames - n) * sizeof(float));
        }
    }
    for (UInt32 i = 1; i < ioData->mNumberBuffers; i++) {
        if (ioData->mBuffers[i].mData != NULL) {
            memset(ioData->mBuffers[i].mData, 0, ioData->mBuffers[i].mDataByteSize);
        }
    }
    return noErr;
}

static OSStatus input_cb(void *inRefCon, AudioUnitRenderActionFlags *ioActionFlags,
    const AudioTimeStamp *inTimeStamp, UInt32 inBusNumber, UInt32 inNumberFrames,
    AudioBufferList *ioData) {
    (void)inBusNumber;
    (void)ioData;
    SaaaVpio *session = (SaaaVpio *)inRefCon;
    if (inNumberFrames == 0 || inNumberFrames > SAAA_MAX_FRAMES) {
        return noErr;
    }
    session->input_list.mNumberBuffers = 1;
    session->input_list.mBuffers[0].mNumberChannels = 1;
    session->input_list.mBuffers[0].mData = session->input_samples;
    session->input_list.mBuffers[0].mDataByteSize = inNumberFrames * (UInt32)sizeof(float);
    OSStatus err = AudioUnitRender(session->unit, ioActionFlags, inTimeStamp, 1, inNumberFrames,
        &session->input_list);
    if (err != noErr) {
        return noErr;
    }
    if (session->write_capture != NULL) {
        session->write_capture(session->capture_ring, session->input_samples, inNumberFrames);
    }
    return noErr;
}

static OSStatus route_listener(AudioObjectID object, UInt32 count,
    const AudioObjectPropertyAddress *addresses, void *ctx) {
    (void)object;
    (void)count;
    (void)addresses;
    SaaaVpio *session = (SaaaVpio *)ctx;
    atomic_store(&session->rebuild, 1);
    if (session->on_route_change != NULL) {
        session->on_route_change(session->route_ctx);
    }
    return noErr;
}

static void add_device_listeners(SaaaVpio *session) {
    AudioObjectPropertyAddress input = {kAudioHardwarePropertyDefaultInputDevice,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    AudioObjectPropertyAddress output = {kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    AudioObjectAddPropertyListener(kAudioObjectSystemObject, &input, route_listener, session);
    AudioObjectAddPropertyListener(kAudioObjectSystemObject, &output, route_listener, session);
}

static void remove_device_listeners(SaaaVpio *session) {
    AudioObjectPropertyAddress input = {kAudioHardwarePropertyDefaultInputDevice,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    AudioObjectPropertyAddress output = {kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    AudioObjectRemovePropertyListener(kAudioObjectSystemObject, &input, route_listener, session);
    AudioObjectRemovePropertyListener(kAudioObjectSystemObject, &output, route_listener, session);
}

static int apply_voice_properties(SaaaVpio *session, char *err, uint32_t err_len) {
    UInt32 bypass = session->bypass ? 1 : 0;
    UInt32 agc = session->enable_agc ? 1 : 0;
    if (set_uint32(session->unit, kAUVoiceIOProperty_BypassVoiceProcessing, kAudioUnitScope_Global,
            0, bypass)
        != noErr) {
        set_error(err, err_len, "Could not set VoiceProcessing bypass");
        return SAAA_VPIO_ERR;
    }
    if (set_uint32(session->unit, kAUVoiceIOProperty_VoiceProcessingEnableAGC,
            kAudioUnitScope_Global, 0, agc)
        != noErr) {
        set_error(err, err_len, "Could not set VoiceProcessing AGC");
        return SAAA_VPIO_ERR;
    }
    if (__builtin_available(macOS 14.0, *)) {
        AUVoiceIOOtherAudioDuckingConfiguration ducking;
        memset(&ducking, 0, sizeof(ducking));
        ducking.mEnableAdvancedDucking = session->advanced_ducking ? 1 : 0;
        ducking.mDuckingLevel = session->ducking_level;
        OSStatus duck_err = AudioUnitSetProperty(session->unit,
            kAUVoiceIOProperty_OtherAudioDuckingConfiguration, kAudioUnitScope_Global, 0, &ducking,
            sizeof(ducking));
        if (duck_err != noErr) {
            set_error(err, err_len, "Could not set other-audio ducking");
            return SAAA_VPIO_ERR;
        }
    }
    return SAAA_VPIO_OK;
}

uint8_t saaa_macos_major_version(void) {
    char release[64];
    size_t size = sizeof(release);
    if (sysctlbyname("kern.osrelease", release, &size, NULL, 0) != 0) {
        return 0;
    }
    unsigned major = 0;
    if (sscanf(release, "%u.", &major) != 1) {
        return 0;
    }
    if (major >= 23) {
        return (uint8_t)(major - 9);
    }
    return (uint8_t)major;
}

int saaa_audio_default_output_transport(uint32_t *transport) {
    if (transport == NULL) {
        return SAAA_VPIO_ERR;
    }
    AudioDeviceID device = 0;
    UInt32 size = sizeof(device);
    AudioObjectPropertyAddress address = {kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size, &device)
        != noErr) {
        return SAAA_VPIO_ERR;
    }
    UInt32 value = 0;
    size = sizeof(value);
    AudioObjectPropertyAddress transport_address = {kAudioDevicePropertyTransportType,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    if (AudioObjectGetPropertyData(device, &transport_address, 0, NULL, &size, &value) != noErr) {
        return SAAA_VPIO_ERR;
    }
    *transport = value;
    return SAAA_VPIO_OK;
}

SaaaVpio *saaa_vpio_create(const SaaaVpioConfig *config, void *playback_ring, void *capture_ring,
    SaaaRingWriteF32 write_capture, SaaaRingReadF32 read_playback, SaaaFlagFn on_route_change,
    void *route_ctx, char *err, uint32_t err_len) {
    if (config == NULL || config->sample_rate == 0) {
        set_error(err, err_len, "VoiceProcessing sample rate is invalid");
        return NULL;
    }
    if (saaa_macos_major_version() < 14) {
        set_error(err, err_len, "VoiceProcessing ducking requires macOS 14 or later");
        return NULL;
    }
    AudioComponentDescription description;
    memset(&description, 0, sizeof(description));
    description.componentType = kAudioUnitType_Output;
    description.componentSubType = kAudioUnitSubType_VoiceProcessingIO;
    description.componentManufacturer = kAudioUnitManufacturer_Apple;
    AudioComponent component = AudioComponentFindNext(NULL, &description);
    if (component == NULL) {
        set_error(err, err_len, "VoiceProcessingIO is unavailable");
        return NULL;
    }
    SaaaVpio *session = (SaaaVpio *)calloc(1, sizeof(SaaaVpio));
    if (session == NULL) {
        set_error(err, err_len, "Could not allocate VoiceProcessing session");
        return NULL;
    }
    session->playback_ring = playback_ring;
    session->capture_ring = capture_ring;
    session->write_capture = write_capture;
    session->read_playback = read_playback;
    session->on_route_change = on_route_change;
    session->route_ctx = route_ctx;
    session->sample_rate = config->sample_rate;
    session->ducking_level = config->ducking_level;
    session->advanced_ducking = config->enable_advanced_ducking;
    session->enable_agc = config->enable_agc;
    session->bypass = config->bypass_voice_processing;
    atomic_store(&session->rebuild, 0);
    if (AudioComponentInstanceNew(component, &session->unit) != noErr) {
        set_error(err, err_len, "Could not create VoiceProcessingIO");
        free(session);
        return NULL;
    }
    UInt32 enable = 1;
    if (set_uint32(session->unit, kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Input, 1,
            enable)
            != noErr
        || set_uint32(session->unit, kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Output, 0,
            enable)
            != noErr) {
        set_error(err, err_len, "Could not enable VoiceProcessing IO");
        AudioComponentInstanceDispose(session->unit);
        free(session);
        return NULL;
    }
    AudioStreamBasicDescription format = mono_float(config->sample_rate);
    if (AudioUnitSetProperty(session->unit, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input,
            0, &format, sizeof(format))
            != noErr
        || AudioUnitSetProperty(session->unit, kAudioUnitProperty_StreamFormat,
            kAudioUnitScope_Output, 1, &format, sizeof(format))
            != noErr) {
        set_error(err, err_len, "Could not set VoiceProcessing stream format");
        AudioComponentInstanceDispose(session->unit);
        free(session);
        return NULL;
    }
    UInt32 max_frames = SAAA_MAX_FRAMES;
    AudioUnitSetProperty(session->unit, kAudioUnitProperty_MaximumFramesPerSlice,
        kAudioUnitScope_Global, 0, &max_frames, sizeof(max_frames));
    AURenderCallbackStruct render;
    memset(&render, 0, sizeof(render));
    render.inputProc = render_cb;
    render.inputProcRefCon = session;
    AURenderCallbackStruct input;
    memset(&input, 0, sizeof(input));
    input.inputProc = input_cb;
    input.inputProcRefCon = session;
    if (AudioUnitSetProperty(session->unit, kAudioUnitProperty_SetRenderCallback,
            kAudioUnitScope_Input, 0, &render, sizeof(render))
            != noErr
        || AudioUnitSetProperty(session->unit, kAudioOutputUnitProperty_SetInputCallback,
            kAudioUnitScope_Global, 0, &input, sizeof(input))
            != noErr) {
        set_error(err, err_len, "Could not set VoiceProcessing callbacks");
        AudioComponentInstanceDispose(session->unit);
        free(session);
        return NULL;
    }
    if (apply_voice_properties(session, err, err_len) != SAAA_VPIO_OK) {
        AudioComponentInstanceDispose(session->unit);
        free(session);
        return NULL;
    }
    if (AudioUnitInitialize(session->unit) != noErr) {
        set_error(err, err_len, "Could not initialize VoiceProcessingIO");
        AudioComponentInstanceDispose(session->unit);
        free(session);
        return NULL;
    }
    if (apply_voice_properties(session, err, err_len) != SAAA_VPIO_OK) {
        AudioUnitUninitialize(session->unit);
        AudioComponentInstanceDispose(session->unit);
        free(session);
        return NULL;
    }
    add_device_listeners(session);
    return session;
}

int saaa_vpio_start(SaaaVpio *session, char *err, uint32_t err_len) {
    if (session == NULL) {
        set_error(err, err_len, "VoiceProcessing session is missing");
        return SAAA_VPIO_ERR;
    }
    if (AudioOutputUnitStart(session->unit) != noErr) {
        set_error(err, err_len, "Could not start VoiceProcessingIO");
        return SAAA_VPIO_ERR;
    }
    session->started = 1;
    return SAAA_VPIO_OK;
}

int saaa_vpio_stop(SaaaVpio *session) {
    if (session == NULL || !session->started) {
        return SAAA_VPIO_OK;
    }
    AudioOutputUnitStop(session->unit);
    session->started = 0;
    return SAAA_VPIO_OK;
}

int saaa_vpio_readback(SaaaVpio *session, SaaaVpioStatus *status) {
    if (session == NULL || status == NULL) {
        return SAAA_VPIO_ERR;
    }
    memset(status, 0, sizeof(*status));
    status->sample_rate = session->sample_rate;
    status->macos_major = saaa_macos_major_version();
    UInt32 bypass = 0;
    UInt32 agc = 0;
    UInt32 size = sizeof(UInt32);
    if (AudioUnitGetProperty(session->unit, kAUVoiceIOProperty_BypassVoiceProcessing,
            kAudioUnitScope_Global, 0, &bypass, &size)
        == noErr) {
        status->bypass_enabled = bypass ? 1 : 0;
    }
    size = sizeof(UInt32);
    if (AudioUnitGetProperty(session->unit, kAUVoiceIOProperty_VoiceProcessingEnableAGC,
            kAudioUnitScope_Global, 0, &agc, &size)
        == noErr) {
        status->agc_enabled = agc ? 1 : 0;
    }
    if (__builtin_available(macOS 14.0, *)) {
        AUVoiceIOOtherAudioDuckingConfiguration ducking;
        memset(&ducking, 0, sizeof(ducking));
        size = sizeof(ducking);
        if (AudioUnitGetProperty(session->unit, kAUVoiceIOProperty_OtherAudioDuckingConfiguration,
                kAudioUnitScope_Global, 0, &ducking, &size)
            == noErr) {
            status->advanced_ducking = ducking.mEnableAdvancedDucking ? 1 : 0;
            status->ducking_level = ducking.mDuckingLevel;
        }
    }
    saaa_audio_default_output_transport(&status->output_transport);
    return SAAA_VPIO_OK;
}

int saaa_vpio_rebuild_requested(const SaaaVpio *session) {
    if (session == NULL) {
        return 0;
    }
    return atomic_load(&session->rebuild) ? 1 : 0;
}

void saaa_vpio_clear_rebuild(SaaaVpio *session) {
    if (session != NULL) {
        atomic_store(&session->rebuild, 0);
    }
}

void saaa_vpio_destroy(SaaaVpio *session) {
    if (session == NULL) {
        return;
    }
    saaa_vpio_stop(session);
    remove_device_listeners(session);
    AudioUnitUninitialize(session->unit);
    AudioComponentInstanceDispose(session->unit);
    free(session);
}
