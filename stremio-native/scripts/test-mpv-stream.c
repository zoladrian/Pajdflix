/* Test-only, silent decoder process. No UI, scripts, configuration or yt-dlp. */
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef void *handle;
static char *(*property)(handle, const char *);
static void (*release)(void *);
static void *(*wait_event)(handle, double);
static int await_frame(handle context, double target, const char *subtitle) {
    ULONGLONG end = GetTickCount64() + 20000;
    while (GetTickCount64() < end) {
        wait_event(context, 0.05);
        char *time = property(context, "time-pos");
        char *width = property(context, "video-params/w");
        char *text = property(context, "sub-text");
        int ready = time && width && text && atof(time) >= target && atoi(width) == 320 && strstr(text, subtitle);
        if (time) release(time); if (width) release(width); if (text) release(text);
        if (ready) return 1;
    }
    return 0;
}
int main(int argc, char **argv) {
    if (argc != 4) return 2;
    SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS);
    HMODULE dll = LoadLibraryExA(argv[1], NULL, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
    if (!dll) return 3;
    handle (*create)(void) = (void *)GetProcAddress(dll, "mpv_create");
    int (*option)(handle,const char *,const char *) = (void *)GetProcAddress(dll, "mpv_set_option_string");
    int (*initialize)(handle) = (void *)GetProcAddress(dll, "mpv_initialize");
    int (*command)(handle,const char *const *) = (void *)GetProcAddress(dll, "mpv_command");
    void (*destroy)(handle) = (void *)GetProcAddress(dll, "mpv_terminate_destroy");
    property = (void *)GetProcAddress(dll, "mpv_get_property_string");
    release = (void *)GetProcAddress(dll, "mpv_free");
    wait_event = (void *)GetProcAddress(dll, "mpv_wait_event");
    if (!create || !option || !initialize || !command || !destroy || !property || !release || !wait_event) return 4;
    handle context = create(); if (!context) return 5;
    const char *settings[][2] = {{"config","no"},{"terminal","no"},{"load-scripts","no"},{"ytdl","no"},{"tls-verify","yes"},{"vo","null"},{"ao","null"},{"hwdec","no"},{"idle","yes"},{"cache","no"},{"demuxer-readahead-secs","1"},{"demuxer-max-bytes","131072"}};
    for (int i=0;i<sizeof(settings)/sizeof(settings[0]);++i) if (option(context,settings[i][0],settings[i][1])<0) return 6;
    if (initialize(context)<0) return 7;
    const char *load[] = {"loadfile",argv[2],NULL};
    if (command(context,load)<0) return 8;
    ULONGLONG end = GetTickCount64()+15000;
    int loaded=0;
    while(GetTickCount64()<end) { wait_event(context,0.05); char *width=property(context,"video-params/w"); if(width) {loaded=atoi(width)==320;release(width);} if(loaded)break; }
    const char *subs[] = {"sub-add",argv[3],"select",NULL};
    if (!loaded || command(context,subs)<0 || !await_frame(context,0.2,"Legalny")) { destroy(context); return 9; }
    puts("DECODER_READY"); fflush(stdout);
    char input[32]; if(!fgets(input,sizeof(input),stdin)) { destroy(context); return 10; }
    const char *seek[] = {"seek","16","absolute+exact",NULL};
    if(command(context,seek)<0 || !await_frame(context,16.2,"Druga")) { destroy(context); return 11; }
    puts("SEEK_AND_SUBTITLES_OK"); fflush(stdout);
    if(!fgets(input,sizeof(input),stdin)) { destroy(context); return 12; }
    destroy(context); FreeLibrary(dll); return 0;
}
