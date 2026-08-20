// rv_host — headless validation host for Retrovert playback plugins.
//
// Speaks playback ABI v2 with injected services (file-backed RVIo, stderr
// RVLog, no-op metadata/settings). Used by the build harness for the
// load/unload smoke and the playback smoke; exit code 0 means the check
// passed. Built against the harness-pinned headers in ../abi, so the
// api_version comparison doubles as the runtime ABI cross-check.
//
// Commands:
//   rv_host load <plugin>                          load/unload + create/destroy cycles
//   rv_host play <plugin> <fixture> [seconds]      probe + decode, assert non-silent

#include <math.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <retrovert/io.h>
#include <retrovert/log.h>
#include <retrovert/metadata.h>
#include <retrovert/playback.h>
#include <retrovert/service.h>
#include <retrovert/settings.h>

#ifdef _WIN32
#include <windows.h>
typedef HMODULE PluginHandle;
#else
#include <dlfcn.h>
typedef void* PluginHandle;
#endif

#define LOAD_CYCLES 3
#define CREATE_CYCLES 5
#define MAX_REOPENS 50
#define MAX_STALLED_READS 1000
#define READ_FRAMES 4096
// Alternated with READ_FRAMES so the request cap binds strictly below any
// plugin's internal render-block size; a plugin ignoring the cap over-returns.
#define SMALL_READ_FRAMES 777
// 16-byte frames (F32 stereo) is the widest layout a plugin can produce.
#define READ_BUFFER_BYTES (READ_FRAMES * 16)
#define SILENCE_THRESHOLD 0.01

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
// Injected RVIo: plain local-file backend.

static bool io_exists(struct RVIoPrivate* self, const char* url) {
    (void)self;
    FILE* f = fopen(url, "rb");
    if (f) {
        fclose(f);
        return true;
    }
    return false;
}

static RVIoReadUrlResult io_read_url_to_memory(struct RVIoPrivate* self, const char* url) {
    (void)self;
    RVIoReadUrlResult res = { NULL, 0 };
    FILE* f = fopen(url, "rb");
    if (!f) {
        fprintf(stderr, "rv_host: io: cannot open %s\n", url);
        return res;
    }
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (size < 0) {
        fclose(f);
        return res;
    }
    uint8_t* data = (uint8_t*)malloc(size ? (size_t)size : 1);
    if (data && fread(data, 1, (size_t)size, f) == (size_t)size) {
        res.data = data;
        res.data_size = (uint64_t)size;
    } else {
        free(data);
    }
    fclose(f);
    return res;
}

static void io_free_url_to_memory(struct RVIoPrivate* self, void* memory) {
    (void)self;
    free(memory);
}

static const RVIo s_io = { NULL, io_exists, io_read_url_to_memory, io_free_url_to_memory };

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
// Injected RVLog: everything to stderr.

static void log_log(struct RVLogPrivate* self, uint32_t level, const char* file, int line, const char* fmt, ...) {
    (void)self;
    static const char* level_names[] = { "TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL" };
    const char* name = level < 6 ? level_names[level] : "?";
    if (file) {
        fprintf(stderr, "[plugin %s %s:%d] ", name, file, line);
    } else {
        fprintf(stderr, "[plugin %s] ", name);
    }
    va_list args;
    va_start(args, fmt);
    vfprintf(stderr, fmt, args);
    va_end(args);
    fputc('\n', stderr);
}

static const RVLog s_log = { NULL, log_log };

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
// Injected RVMetadata: accepts and discards everything.

static RVMetadataId meta_create_url(struct RVMetadataPrivate* self, const char* url) {
    (void)self;
    (void)url;
    return 1;
}
static void meta_set_tag(struct RVMetadataPrivate* self, RVMetadataId id, const char* tag, const char* data) {
    (void)self;
    (void)id;
    (void)tag;
    (void)data;
}
static void meta_set_tag_f64(struct RVMetadataPrivate* self, RVMetadataId id, const char* tag, double data) {
    (void)self;
    (void)id;
    (void)tag;
    (void)data;
}
static void meta_add_subsong(struct RVMetadataPrivate* self, RVMetadataId parent_id, uint32_t index, const char* name,
                             float length) {
    (void)self;
    (void)parent_id;
    (void)index;
    (void)name;
    (void)length;
}
static void meta_add_sample(struct RVMetadataPrivate* self, RVMetadataId parent_id, const char* text) {
    (void)self;
    (void)parent_id;
    (void)text;
}
static void meta_add_instrument(struct RVMetadataPrivate* self, RVMetadataId parent_id, const char* text) {
    (void)self;
    (void)parent_id;
    (void)text;
}

static const RVMetadata s_metadata = { NULL,          meta_create_url, meta_set_tag,
                                       meta_set_tag_f64, meta_add_subsong, meta_add_sample,
                                       meta_add_instrument };

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
// Injected RVSettings: registration succeeds, every lookup reports NotFound
// so plugins fall back to their built-in defaults.

static RVSettingsResult settings_reg(struct RVSettingsPrivate* self, const char* id, RVSetting* settings,
                                     uint64_t settings_size) {
    (void)self;
    (void)id;
    (void)settings;
    (void)settings_size;
    return RVSettingsResult_Ok;
}
static RVSStringResult settings_get_string(struct RVSettingsPrivate* self, const char* reg_id, const char* ext,
                                           const char* id) {
    (void)self;
    (void)reg_id;
    (void)ext;
    (void)id;
    RVSStringResult res = { RVSettingsResult_NotFound, NULL };
    return res;
}
static RVSIntResult settings_get_int(struct RVSettingsPrivate* self, const char* reg_id, const char* ext,
                                     const char* id) {
    (void)self;
    (void)reg_id;
    (void)ext;
    (void)id;
    RVSIntResult res = { RVSettingsResult_NotFound, 0 };
    return res;
}
static RVSFloatResult settings_get_float(struct RVSettingsPrivate* self, const char* reg_id, const char* ext,
                                         const char* id) {
    (void)self;
    (void)reg_id;
    (void)ext;
    (void)id;
    RVSFloatResult res = { RVSettingsResult_NotFound, 0.0f };
    return res;
}
static RVSBoolResult settings_get_bool(struct RVSettingsPrivate* self, const char* reg_id, const char* ext,
                                       const char* id) {
    (void)self;
    (void)reg_id;
    (void)ext;
    (void)id;
    RVSBoolResult res = { RVSettingsResult_NotFound, false };
    return res;
}

static const RVSettings s_settings = { NULL, settings_reg, settings_get_string, settings_get_int, settings_get_float,
                                       settings_get_bool };

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
// Service locator.

static const struct RVIo* service_get_io(struct RVServicePrivData* p, int api_version) {
    (void)p;
    return api_version == RV_IO_API_VERSION ? &s_io : NULL;
}
static const struct RVLog* service_get_log(struct RVServicePrivData* p, int api_version) {
    (void)p;
    return api_version == RV_LOG_API_VERSION ? &s_log : NULL;
}
static const struct RVMetadata* service_get_metadata(struct RVServicePrivData* p, int api_version) {
    (void)p;
    return api_version == RV_METADATA_API_VERSION ? &s_metadata : NULL;
}
static const struct RVSettings* service_get_settings(struct RVServicePrivData* p, int api_version) {
    (void)p;
    return api_version == RV_SETTINGS_API_VERSION ? &s_settings : NULL;
}

static const RVService s_service = { NULL, service_get_io, service_get_log, service_get_metadata,
                                     service_get_settings };

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////
// Plugin loading. On Windows the host mirrors the consumer loader policy:
// safe search flags only, no legacy fallback.

typedef RVPlaybackPlugin* (*PluginEntry)(void);

static PluginHandle plugin_open(const char* path) {
#ifdef _WIN32
    return LoadLibraryExA(path, NULL, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
#else
    return dlopen(path, RTLD_NOW | RTLD_LOCAL);
#endif
}

static void plugin_close(PluginHandle handle) {
#ifdef _WIN32
    FreeLibrary(handle);
#else
    dlclose(handle);
#endif
}

static PluginEntry plugin_entry(PluginHandle handle) {
#ifdef _WIN32
    return (PluginEntry)(void*)GetProcAddress(handle, "rv_playback_plugin");
#else
    // Object-to-function cast required by the dlsym API.
    union {
        void* obj;
        PluginEntry fn;
    } cast;
    cast.obj = dlsym(handle, "rv_playback_plugin");
    return cast.fn;
#endif
}

static void print_load_error(const char* path) {
#ifdef _WIN32
    fprintf(stderr, "rv_host: failed to load %s (error %lu)\n", path, GetLastError());
#else
    fprintf(stderr, "rv_host: failed to load %s: %s\n", path, dlerror());
#endif
}

// Load the plugin and validate its identity block; NULL on failure.
static RVPlaybackPlugin* plugin_get_checked(PluginHandle handle, const char* path) {
    PluginEntry entry = plugin_entry(handle);
    if (!entry) {
        fprintf(stderr, "rv_host: %s does not export rv_playback_plugin\n", path);
        return NULL;
    }
    RVPlaybackPlugin* plugin = entry();
    if (!plugin) {
        fprintf(stderr, "rv_host: rv_playback_plugin() returned NULL\n");
        return NULL;
    }
    if (plugin->api_version != RV_PLAYBACK_PLUGIN_API_VERSION) {
        fprintf(stderr, "rv_host: ABI mismatch: plugin reports api_version %llu, host expects %d\n",
                (unsigned long long)plugin->api_version, RV_PLAYBACK_PLUGIN_API_VERSION);
        return NULL;
    }
    if (!plugin->name || !plugin->probe_can_play || !plugin->create || !plugin->destroy || !plugin->open ||
        !plugin->close || !plugin->read_data) {
        fprintf(stderr, "rv_host: plugin is missing mandatory ABI entries\n");
        return NULL;
    }
    return plugin;
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

static int cmd_load(const char* path) {
    for (int cycle = 0; cycle < LOAD_CYCLES; ++cycle) {
        PluginHandle handle = plugin_open(path);
        if (!handle) {
            print_load_error(path);
            return 1;
        }
        RVPlaybackPlugin* plugin = plugin_get_checked(handle, path);
        if (!plugin) {
            plugin_close(handle);
            return 1;
        }
        if (cycle == 0) {
            fprintf(stderr, "rv_host: loaded '%s' version '%s' api %llu\n", plugin->name,
                    plugin->version ? plugin->version : "?", (unsigned long long)plugin->api_version);
        }
        if (plugin->static_init) {
            plugin->static_init(&s_service);
        }
        for (int i = 0; i < CREATE_CYCLES; ++i) {
            void* instance = plugin->create(&s_service);
            if (!instance) {
                fprintf(stderr, "rv_host: create() failed (cycle %d, instance %d)\n", cycle, i);
                return 1;
            }
            if (plugin->destroy(instance) != 0) {
                fprintf(stderr, "rv_host: destroy() failed (cycle %d, instance %d)\n", cycle, i);
                return 1;
            }
        }
        if (plugin->static_destroy) {
            plugin->static_destroy();
        }
        plugin_close(handle);
    }
    fprintf(stderr, "rv_host: load smoke passed (%d load cycles, %d create/destroy each)\n", LOAD_CYCLES,
            CREATE_CYCLES);
    return 0;
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

static double sample_peak(const void* buffer, const RVReadInfo* info) {
    uint32_t count = info->frame_count * info->format.channel_count;
    double peak = 0.0;
    switch (info->format.audio_format) {
        case RVAudioStreamFormat_U8: {
            const uint8_t* s = (const uint8_t*)buffer;
            for (uint32_t i = 0; i < count; ++i) {
                double v = fabs((s[i] - 128.0) / 128.0);
                if (v > peak) peak = v;
            }
            break;
        }
        case RVAudioStreamFormat_S16: {
            const int16_t* s = (const int16_t*)buffer;
            for (uint32_t i = 0; i < count; ++i) {
                double v = fabs(s[i] / 32768.0);
                if (v > peak) peak = v;
            }
            break;
        }
        case RVAudioStreamFormat_S24: {
            // Packed little-endian 3-byte samples.
            const uint8_t* s = (const uint8_t*)buffer;
            for (uint32_t i = 0; i < count; ++i) {
                int32_t v = (int32_t)((uint32_t)s[i * 3] << 8 | (uint32_t)s[i * 3 + 1] << 16 |
                                      (uint32_t)s[i * 3 + 2] << 24) >>
                            8;
                double a = fabs(v / 8388608.0);
                if (a > peak) peak = a;
            }
            break;
        }
        case RVAudioStreamFormat_S32: {
            const int32_t* s = (const int32_t*)buffer;
            for (uint32_t i = 0; i < count; ++i) {
                double v = fabs(s[i] / 2147483648.0);
                if (v > peak) peak = v;
            }
            break;
        }
        case RVAudioStreamFormat_F32: {
            const float* s = (const float*)buffer;
            for (uint32_t i = 0; i < count; ++i) {
                double v = fabs(s[i]);
                if (v > peak) peak = v;
            }
            break;
        }
        default:
            break;
    }
    return peak;
}

static uint32_t format_frame_bytes(const RVAudioFormat* format) {
    uint32_t sample_bytes;
    switch (format->audio_format) {
        case RVAudioStreamFormat_U8: sample_bytes = 1; break;
        case RVAudioStreamFormat_S16: sample_bytes = 2; break;
        case RVAudioStreamFormat_S24: sample_bytes = 3; break;
        case RVAudioStreamFormat_S32: sample_bytes = 4; break;
        case RVAudioStreamFormat_F32: sample_bytes = 4; break;
        default: return 0;
    }
    return sample_bytes * format->channel_count;
}

static int cmd_play(const char* path, const char* fixture, double target_seconds) {
    PluginHandle handle = plugin_open(path);
    if (!handle) {
        print_load_error(path);
        return 1;
    }
    RVPlaybackPlugin* plugin = plugin_get_checked(handle, path);
    if (!plugin) {
        return 1;
    }
    if (plugin->static_init) {
        plugin->static_init(&s_service);
    }

    RVIoReadUrlResult probe_data = io_read_url_to_memory(NULL, fixture);
    if (!probe_data.data) {
        fprintf(stderr, "rv_host: cannot read fixture %s\n", fixture);
        return 1;
    }
    RVProbeResult probe =
        plugin->probe_can_play(probe_data.data, probe_data.data_size, fixture, probe_data.data_size);
    free(probe_data.data);
    if (probe == RVProbeResult_Unsupported) {
        fprintf(stderr, "rv_host: plugin rejected fixture %s in probe\n", fixture);
        return 1;
    }
    fprintf(stderr, "rv_host: probe: %s\n", probe == RVProbeResult_Supported ? "supported" : "unsure");

    void* instance = plugin->create(&s_service);
    if (!instance) {
        fprintf(stderr, "rv_host: create() failed\n");
        return 1;
    }
    if (plugin->open(instance, fixture, 0, &s_service) != 0) {
        fprintf(stderr, "rv_host: open(%s) failed\n", fixture);
        return 1;
    }

    void* buffer = malloc(READ_BUFFER_BYTES);
    double decoded_seconds = 0.0;
    double peak = 0.0;
    int reopens = 0;
    int stalled_reads = 0;
    int failed = 0;
    uint64_t reads = 0;

    while (decoded_seconds < target_seconds) {
        RVReadData dest;
        memset(&dest, 0, sizeof(dest));
        dest.channels_output = buffer;
        dest.channels_output_max_bytes_size = READ_BUFFER_BYTES;
        dest.info.format.audio_format = RVAudioStreamFormat_S16;
        dest.info.format.channel_count = 2;
        dest.info.format.sample_rate = 48000;
        dest.info.frame_count = (reads++ & 1) ? SMALL_READ_FRAMES : READ_FRAMES;

        RVReadInfo info = plugin->read_data(instance, dest);

        if (info.frame_count > dest.info.frame_count) {
            fprintf(stderr, "rv_host: read_data returned %u frames for a %u-frame request\n", info.frame_count,
                    dest.info.frame_count);
            failed = 1;
            break;
        }
        if (info.status == RVReadStatus_Error) {
            fprintf(stderr, "rv_host: read_data reported an error after %.2fs\n", decoded_seconds);
            failed = 1;
            break;
        }
        if (info.status == RVReadStatus_DecodingRequest || info.frame_count == 0) {
            if (info.status == RVReadStatus_Finished) {
                // Fixture shorter than the target: rewind by reopening and keep decoding.
                plugin->close(instance);
                if (++reopens > MAX_REOPENS) {
                    fprintf(stderr, "rv_host: gave up after %d reopens at %.2fs\n", reopens, decoded_seconds);
                    failed = 1;
                    break;
                }
                if (plugin->open(instance, fixture, 0, &s_service) != 0) {
                    fprintf(stderr, "rv_host: reopen failed\n");
                    failed = 1;
                    break;
                }
                continue;
            }
            if (++stalled_reads > MAX_STALLED_READS) {
                fprintf(stderr, "rv_host: no audio after %d reads\n", stalled_reads);
                failed = 1;
                break;
            }
            continue;
        }
        stalled_reads = 0;

        uint32_t frame_bytes = format_frame_bytes(&info.format);
        if (frame_bytes == 0 || info.format.sample_rate == 0) {
            fprintf(stderr, "rv_host: read_data returned invalid format (fmt %d, rate %u)\n",
                    (int)info.format.audio_format, info.format.sample_rate);
            failed = 1;
            break;
        }
        if ((uint64_t)info.frame_count * frame_bytes > READ_BUFFER_BYTES) {
            fprintf(stderr, "rv_host: plugin overran the output buffer (%u frames of %u bytes)\n", info.frame_count,
                    frame_bytes);
            failed = 1;
            break;
        }

        double block_peak = sample_peak(buffer, &info);
        if (block_peak > peak) peak = block_peak;
        decoded_seconds += (double)info.frame_count / info.format.sample_rate;

        if (info.status == RVReadStatus_Finished) {
            plugin->close(instance);
            if (decoded_seconds >= target_seconds) {
                break;
            }
            if (++reopens > MAX_REOPENS) {
                fprintf(stderr, "rv_host: gave up after %d reopens at %.2fs\n", reopens, decoded_seconds);
                failed = 1;
                break;
            }
            if (plugin->open(instance, fixture, 0, &s_service) != 0) {
                fprintf(stderr, "rv_host: reopen failed\n");
                failed = 1;
                break;
            }
        }
    }

    if (!failed && decoded_seconds >= target_seconds) {
        plugin->close(instance);
    }
    free(buffer);
    if (plugin->destroy(instance) != 0) {
        fprintf(stderr, "rv_host: destroy() failed\n");
        failed = 1;
    }
    if (plugin->static_destroy) {
        plugin->static_destroy();
    }
    plugin_close(handle);

    if (failed) {
        return 1;
    }
    if (peak < SILENCE_THRESHOLD) {
        fprintf(stderr, "rv_host: output is silent (peak %.5f over %.2fs)\n", peak, decoded_seconds);
        return 1;
    }
    fprintf(stderr, "rv_host: playback smoke passed: %.2fs decoded, peak %.3f, %d reopen(s)\n", decoded_seconds, peak,
            reopens);
    return 0;
}

///////////////////////////////////////////////////////////////////////////////////////////////////////////////////////

int main(int argc, char** argv) {
    if (argc >= 3 && strcmp(argv[1], "load") == 0) {
        return cmd_load(argv[2]);
    }
    if (argc >= 4 && strcmp(argv[1], "play") == 0) {
        double seconds = argc >= 5 ? atof(argv[4]) : 10.0;
        return cmd_play(argv[2], argv[3], seconds);
    }
    fprintf(stderr,
            "usage:\n"
            "  rv_host load <plugin>\n"
            "  rv_host play <plugin> <fixture> [seconds]\n");
    return 2;
}
