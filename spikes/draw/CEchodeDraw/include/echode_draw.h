#ifndef ECHODE_DRAW_H
#define ECHODE_DRAW_H

#include <stdint.h>

typedef struct EchodeInstance {
    float x;
    float y;
    float w;
    float h;
    uint32_t rgba;
    uint32_t _pad0;
    uint32_t _pad1;
    uint32_t _pad2;
} EchodeInstance;

typedef struct EchodeScene EchodeScene;

EchodeScene *echode_scene_new(void);
void echode_scene_free(EchodeScene *scene);
uint32_t echode_scene_track_count(const EchodeScene *scene);
uint32_t echode_scene_note_count(const EchodeScene *scene);
float echode_scene_timeline_beats(const EchodeScene *scene);

uint32_t echode_scene_visible_tracks(
    const EchodeScene *scene,
    float origin_x,
    float origin_y,
    float width,
    float height,
    float zoom_x,
    float zoom_y,
    EchodeInstance *out,
    uint32_t capacity,
    uint32_t *dropped
);

uint32_t echode_scene_visible_notes(
    const EchodeScene *scene,
    float origin_x,
    float origin_y,
    float width,
    float height,
    float zoom_x,
    float zoom_y,
    EchodeInstance *out,
    uint32_t capacity,
    uint32_t *dropped
);

#endif
