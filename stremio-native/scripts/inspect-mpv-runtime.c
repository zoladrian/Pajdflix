/* Inspect an explicitly selected DLL without loading media, scripts or user config. */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
int wmain(int argc, wchar_t **argv) {
    if (argc != 2) return 2;
    SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS);
    HMODULE dll = LoadLibraryExW(argv[1], NULL, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
    if (!dll) { printf("DLL load failed: %lu\n", GetLastError()); return 3; }
    unsigned long long (*api)(void) = (void *)GetProcAddress(dll, "mpv_client_api_version");
    void *(*create)(void) = (void *)GetProcAddress(dll, "mpv_create");
    int (*option)(void *, const char *, const char *) = (void *)GetProcAddress(dll, "mpv_set_option_string");
    int (*initialize)(void *) = (void *)GetProcAddress(dll, "mpv_initialize");
    char *(*property)(void *, const char *) = (void *)GetProcAddress(dll, "mpv_get_property_string");
    void (*release)(void *) = (void *)GetProcAddress(dll, "mpv_free");
    void (*destroy)(void *) = (void *)GetProcAddress(dll, "mpv_terminate_destroy");
    if (!api || !create || !option || !initialize || !property || !release || !destroy) return 4;
    unsigned long long version = api();
    printf("client-api=%llu.%llu\n", version >> 16, version & 65535);
    void *context = create(); if (!context) return 5;
    const char *options[] = {"config", "terminal", "load-scripts", "ytdl"};
    for (int i = 0; i < 4; ++i) if (option(context, options[i], "no") < 0) { destroy(context); return 6; }
    if (initialize(context) < 0) { destroy(context); return 7; }
    const char *names[] = {"mpv-version", "ffmpeg-version", "mpv-configuration"};
    for (int i = 0; i < 3; ++i) {
        char *value = property(context, names[i]);
        printf("%s=%s\n", names[i], value ? value : "UNAVAILABLE");
        if (value) release(value);
    }
    destroy(context); FreeLibrary(dll); return 0;
}
